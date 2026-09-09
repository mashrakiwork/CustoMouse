//! Compiling a pack into finished `.cur` / `.ani` files.

use crate::ani::{self, Anim};
use crate::ico::{self, Image};
use crate::manifest::{Pack, RoleSpec, UpscaleMode};
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
/// hotspot or turns on `upscale`, either of which needs the file re-encoded.
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
    let (animated, frame_count, sizes) = match ani::parse(&data) {
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
    let native = sizes.iter().copied().max().unwrap_or(32);

    // A plain pass-through needs no decoding at all. Overriding the hotspot or
    // synthesizing larger sizes both need real pixels to work with.
    let upscaling = spec.upscale != UpscaleMode::Off;
    let needs_decode = spec.hotspot.is_some() || upscaling;

    let (out_data, final_sizes) = if !needs_decode {
        (data, sizes.clone())
    } else if animated {
        let mut anim = ani::decode(&data)?;

        // Folding in the pack's larger sizes is capped to what a single frame
        // may hold — the same budget drawn animations respect. Without
        // `upscale` the original sizes are kept exactly as they came, since a
        // real ready-made file was already something Windows could load.
        let mut wanted = sizes.clone();
        if upscaling {
            wanted.extend(pack.sizes.iter().copied());
            wanted.sort_unstable();
            wanted.dedup();
        }
        let (kept, dropped) = ani::fit_sizes(&wanted);
        if upscaling && !dropped.is_empty() {
            warnings.push(Warning::AnimationSizesDropped {
                role: role.to_string(),
                kept: kept.clone(),
                dropped,
            });
        }

        for frame in &mut anim.frames {
            let existing = std::mem::take(frame);
            *frame = build_sized_frame(pack, spec, existing, &kept)?;
        }

        (ani::encode(&anim)?, kept)
    } else {
        let existing = ico::decode(&data)?;
        let mut wanted = sizes.clone();
        if upscaling {
            wanted.extend(pack.sizes.iter().copied());
            wanted.sort_unstable();
            wanted.dedup();
        }
        let images = build_sized_frame(pack, spec, existing, &wanted)?;
        (ico::encode(&images, ico::TYPE_CURSOR)?, wanted)
    };

    // Report what actually happened: sizes synthesized by upscaling, or (if
    // upscaling was off, or on but added nothing — e.g. the animation budget
    // left no room) that the source is still stuck at its native resolution.
    let added: Vec<u32> = final_sizes
        .iter()
        .copied()
        .filter(|s| !sizes.contains(s))
        .collect();
    if upscaling && !added.is_empty() {
        warnings.push(Warning::ReadyMadeUpscaled {
            role: role.to_string(),
            native,
            added,
            mode: spec.upscale,
        });
    } else if native < 64 {
        warnings.push(Warning::LowResolutionSource {
            role: role.to_string(),
            largest: native,
        });
    }

    let ext = if animated { "ani" } else { "cur" };
    Ok((
        BuiltCursor {
            role: role.to_string(),
            file_name: format!("{role}.{ext}"),
            data: out_data,
            animated,
            frames: frame_count,
            sizes: final_sizes,
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

/// Scale a hotspot recorded at `from_size` to `to_size` by simple proportion,
/// clamped inside the target canvas.
fn scale_hotspot(from_size: u32, hot: (u16, u16), to_size: u32) -> (u16, u16) {
    let ratio = to_size as f64 / from_size as f64;
    let x = ((hot.0 as f64 * ratio).round() as u32).min(to_size.saturating_sub(1));
    let y = ((hot.1 as f64 * ratio).round() as u32).min(to_size.saturating_sub(1));
    (x as u16, y as u16)
}

/// Build one frame's image set for `target_sizes` out of a ready-made source's
/// `existing` images.
///
/// A target size already present is kept exactly (pixels untouched); any other
/// is synthesized by upscaling the largest existing image, which is the best
/// single source available — resampling once from the highest-resolution
/// original beats resampling from whichever size happens to be closest.
///
/// Hotspot: an explicit override (`spec.hotspot`) is applied uniformly via
/// `pack.scaled_hotspot`. Otherwise a size that already existed keeps its own
/// recorded hotspot, and a synthesized size scales the hotspot proportionally
/// from the largest existing image.
fn build_sized_frame(
    pack: &Pack,
    spec: &RoleSpec,
    existing: Vec<Image>,
    target_sizes: &[u32],
) -> Result<Vec<Image>> {
    let mut by_size: std::collections::BTreeMap<u32, Image> =
        existing.into_iter().map(|img| (img.size, img)).collect();
    let &largest_size = by_size
        .keys()
        .max()
        .ok_or_else(|| Error::malformed("cursor", "frame has no images to build from"))?;
    let largest_rgba = by_size[&largest_size].rgba.clone();
    let largest_hot = (by_size[&largest_size].hot_x, by_size[&largest_size].hot_y);

    // Vector mode traces the largest image once and renders every missing
    // size from that single trace, rather than resampling per size.
    let needs_synthesis = target_sizes.iter().any(|s| !by_size.contains_key(s));
    let traced = if spec.upscale == UpscaleMode::Vector && needs_synthesis {
        Some(crate::raster::trace_to_vector(&largest_rgba, largest_size)?)
    } else {
        None
    };

    let mut out = Vec::with_capacity(target_sizes.len());
    for &size in target_sizes {
        let img = match by_size.remove(&size) {
            Some(existing_img) => existing_img,
            None => {
                let rgba = match &traced {
                    Some(tree) => crate::raster::render_vector(tree, size)?,
                    None => crate::raster::resize_rgba(&largest_rgba, largest_size, size)?,
                };
                let (hx, hy) = scale_hotspot(largest_size, largest_hot, size);
                Image::new(size, rgba, hx, hy)?
            }
        };
        out.push(img);
    }

    if spec.hotspot.is_some() {
        for img in &mut out {
            let (hx, hy) = pack.scaled_hotspot(spec, img.size);
            img.hot_x = hx;
            img.hot_y = hy;
        }
    }
    Ok(out)
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
