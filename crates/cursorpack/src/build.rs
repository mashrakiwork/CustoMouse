//! Compiling a pack into finished `.cur` / `.ani` files.

use crate::ani::{self, Anim};
use crate::ico::{self, Image};
use crate::manifest::{Pack, RoleSpec};
use crate::raster::{Art, Warning};
use crate::{Error, Result};
use std::path::Path;

/// One finished cursor file.
pub struct BuiltCursor {
    /// Windows role name, e.g. `Arrow`.
    pub role: String,
    /// File name to write, e.g. `Arrow.cur` or `Wait.ani`.
    pub file_name: String,
    pub data: Vec<u8>,
    pub animated: bool,
    pub frames: usize,
    pub sizes: Vec<u32>,
}

/// Everything a build produced.
#[derive(Debug)]
pub struct Built {
    pub cursors: Vec<BuiltCursor>,
    pub warnings: Vec<Warning>,
}

/// Prints metadata only: `data` is megabytes of pixels and would drown any
/// assertion message that included it.
impl std::fmt::Debug for BuiltCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltCursor")
            .field("role", &self.role)
            .field("file_name", &self.file_name)
            .field("animated", &self.animated)
            .field("frames", &self.frames)
            .field("sizes", &self.sizes)
            .field("bytes", &self.data.len())
            .finish()
    }
}

impl Built {
    /// Write every cursor into `out_dir`, creating it if needed.
    /// Returns each role paired with the path written.
    pub fn write_to(&self, out_dir: &Path) -> Result<Vec<(String, std::path::PathBuf)>> {
        std::fs::create_dir_all(out_dir).map_err(|e| Error::io(out_dir, e))?;
        let mut written = Vec::with_capacity(self.cursors.len());
        for c in &self.cursors {
            let path = out_dir.join(&c.file_name);
            std::fs::write(&path, &c.data).map_err(|e| Error::io(&path, e))?;
            written.push((c.role.clone(), path));
        }
        Ok(written)
    }
}

/// Render one small still image per role, for gallery thumbnails.
///
/// Much cheaper than a full build: a single size, first frame only, no encoding.
/// Returns `(role, size, rgba)` per role, in canonical role order.
pub fn thumbnails(pack: &Pack, pack_dir: &Path, size: u32) -> Result<Vec<(String, u32, Vec<u8>)>> {
    let mut out = Vec::new();
    for (role, spec) in pack.active_roles() {
        let Some(rel) = spec.source.as_ref() else {
            continue;
        };
        let path = pack_dir.join(rel);

        // Ready-made cursors are not rasterisable art; decode them instead.
        // Missing this is what made imported packs show "cannot build".
        if is_ready_made(&path) {
            if let Some((px, rgba)) = ready_made_thumbnail(&path, size) {
                out.push((role.to_string(), px, rgba));
            }
            continue;
        }

        let (art, _) = Art::load(&path, spec.grid)?;
        let mut frames = art.render(size)?;
        if frames.is_empty() {
            continue;
        }
        out.push((role.to_string(), size, frames.remove(0)));
    }
    Ok(out)
}

/// First frame of a `.cur`/`.ani`, at whichever embedded size is closest to
/// `want`. Returns `None` rather than failing the whole gallery on one bad file.
fn ready_made_thumbnail(path: &Path, want: u32) -> Option<(u32, Vec<u8>)> {
    let data = std::fs::read(path).ok()?;
    let images = match ani::decode(&data) {
        Ok(anim) => anim.frames.into_iter().next()?,
        Err(_) => ico::decode(&data).ok()?,
    };
    let best = images
        .into_iter()
        .min_by_key(|i| i.size.abs_diff(want))?;
    Some((best.size, best.rgba))
}

/// Compile every active role in `pack`, resolving art relative to `pack_dir`.
pub fn build(pack: &Pack, pack_dir: &Path) -> Result<Built> {
    pack.validate()?;

    let mut cursors = Vec::new();
    let mut warnings = Vec::new();

    for (role, spec) in pack.active_roles() {
        let rel = spec.source.as_ref().expect("active roles have a source");
        let path = pack_dir.join(rel);

        // A ready-made .cur/.ani is already in the format Windows wants. Copy it
        // through rather than rasterising it, which keeps its exact pixels, hotspot
        // and frame timing.
        if is_ready_made(&path) {
            let (built, mut warn) = build_ready_made(pack, role, spec, &path)?;
            warnings.append(&mut warn);
            cursors.push(built);
            continue;
        }

        let (art, mut warn) = Art::load(&path, spec.grid)?;
        warnings.append(&mut warn);
        warnings.extend(art.size_warnings(&path, &pack.sizes));

        let (built, mut role_warnings) = build_role(pack, role, spec, &art)?;
        warnings.append(&mut role_warnings);
        cursors.push(built);
    }

    warnings.dedup();
    Ok(Built { cursors, warnings })
}

/// True for source files that are already Windows cursors.
pub fn is_ready_made(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "cur" | "ani" | "ico"
    )
}

/// Use an existing `.cur`/`.ani` directly.
///
/// The file is parsed to prove it is valid and to read back its sizes, then its
/// original bytes are written out unchanged — unless the manifest overrides the
/// hotspot, in which case it is re-encoded with the new one.
fn build_ready_made(
    pack: &Pack,
    role: &str,
    spec: &RoleSpec,
    path: &Path,
) -> Result<(BuiltCursor, Vec<Warning>)> {
    let data = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    let mut warnings = Vec::new();

    // Only the *directory* is parsed for metadata, never the pixels. Legacy
    // cursors are often 1/4/8bpp paletted, which we deliberately do not decode —
    // but Windows loads them fine, and copying them through needs no decoding at
    // all. Insisting on a full decode here would reject perfectly good files.
    let (animated, frames, sizes) = match ani::parse(&data) {
        Ok(raw) => {
            let sizes = frame_sizes(raw.frames.first().map(|f| f.as_slice()).unwrap_or(&[]));
            (true, raw.frames.len(), sizes)
        }
        Err(_) => {
            let sizes = frame_sizes(&data);
            if sizes.is_empty() {
                return Err(Error::malformed(
                    "cursor",
                    format!("{} is neither a .cur nor a .ani we can read", path.display()),
                ));
            }
            (false, 1, sizes)
        }
    };

    // Rewriting the hotspot is the one case that needs real pixels.
    let out_data = match spec.hotspot {
        None => data,
        Some(_) if animated => {
            let mut anim = ani::decode(&data)?;
            for frame in &mut anim.frames {
                for img in frame.iter_mut() {
                    let (hx, hy) = pack.scaled_hotspot(spec, img.size);
                    img.hot_x = hx;
                    img.hot_y = hy;
                }
            }
            ani::encode(&anim)?
        }
        Some(_) => {
            let mut images = ico::decode(&data)?;
            for img in &mut images {
                let (hx, hy) = pack.scaled_hotspot(spec, img.size);
                img.hot_x = hx;
                img.hot_y = hy;
            }
            ico::encode(&images, ico::TYPE_CURSOR)?
        }
    };

    if let Some(&largest) = sizes.iter().max() {
        if largest < 64 {
            warnings.push(Warning::LowResolutionSource {
                role: role.to_string(),
                largest,
            });
        }
    }

    let ext = if animated { "ani" } else { "cur" };
    Ok((
        BuiltCursor {
            role: role.to_string(),
            file_name: format!("{role}.{ext}"),
            data: out_data,
            animated,
            frames,
            sizes,
        },
        warnings,
    ))
}

/// Sizes listed in a cursor's directory, without touching pixel data.
fn frame_sizes(data: &[u8]) -> Vec<u32> {
    let mut sizes: Vec<u32> = ico::read_dir(data)
        .map(|entries| entries.iter().map(|e| e.width).collect())
        .unwrap_or_default();
    sizes.sort_unstable();
    sizes.dedup();
    sizes
}

fn build_role(
    pack: &Pack,
    role: &str,
    spec: &RoleSpec,
    art: &Art,
) -> Result<(BuiltCursor, Vec<Warning>)> {
    let mut sizes = pack.sizes.clone();
    sizes.sort_unstable();
    sizes.dedup();

    // Animated frames have a per-frame ceiling in the Windows loader; static
    // cursors do not, so only trim when there is more than one frame.
    let mut warnings = Vec::new();
    if art.is_animated() {
        let (kept, dropped) = ani::fit_sizes(&sizes);
        if !dropped.is_empty() {
            warnings.push(Warning::AnimationSizesDropped {
                role: role.to_string(),
                kept: kept.clone(),
                dropped,
            });
        }
        sizes = kept;
    }

    // Render every size, then regroup so each animation frame carries all its sizes.
    // frames_by_size[size_index][frame_index]
    let mut frames_by_size = Vec::with_capacity(sizes.len());
    for &size in &sizes {
        frames_by_size.push(art.render(size)?);
    }

    let frame_count = art.frame_count();
    let mut frames: Vec<Vec<Image>> = Vec::with_capacity(frame_count);
    for f in 0..frame_count {
        let mut images = Vec::with_capacity(sizes.len());
        for (si, &size) in sizes.iter().enumerate() {
            let (hot_x, hot_y) = pack.scaled_hotspot(spec, size);
            let rgba = frames_by_size[si]
                .get(f)
                .ok_or_else(|| {
                    Error::malformed("build", format!("role '{role}' lost frame {f} at {size}px"))
                })?
                .clone();
            images.push(Image::new(size, rgba, hot_x, hot_y)?);
        }
        frames.push(images);
    }

    if frame_count > 1 {
        let mut anim = Anim::from_frames(frames, spec.fps)?;

        // A GIF carries its own per-frame timing; honour it instead of a flat rate.
        if let Some(delays) = art.delays_ms() {
            let rates: Vec<u32> = delays
                .iter()
                .map(|&ms| ((ms as f32 * ani::JIFFIES_PER_SEC / 1000.0).round() as u32).max(1))
                .collect();
            if rates.len() == anim.frames.len() && rates.iter().any(|&r| r != rates[0]) {
                anim.rates = Some(rates);
            } else if let Some(&first) = rates.first() {
                anim.default_rate = first;
            }
        }

        Ok((
            BuiltCursor {
                role: role.to_string(),
                file_name: format!("{role}.ani"),
                data: ani::encode(&anim)?,
                animated: true,
                frames: frame_count,
                sizes,
            },
            warnings,
        ))
    } else {
        let images = frames.into_iter().next().expect("one frame");
        Ok((
            BuiltCursor {
                role: role.to_string(),
                file_name: format!("{role}.cur"),
                data: ico::encode(&images, ico::TYPE_CURSOR)?,
                animated: false,
                frames: 1,
                sizes,
            },
            warnings,
        ))
    }
}
