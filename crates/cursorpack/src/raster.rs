//! Turning whatever art you dropped in into square RGBA frames at any size.
//!
//! Which shape you gave is detected from the path:
//!
//! | You supply                           | Detected as        |
//! |--------------------------------------|--------------------|
//! | `arrow.svg`                          | vector, static     |
//! | `arrow.png` (>= 128px)               | raster, static     |
//! | `spin/` holding `f_00.png..f_11.png` | numbered sequence  |
//! | `spin.png` + a `grid`                | sprite sheet       |
//! | `spin.gif`                           | animated GIF       |
//!
//! `.cur` and `.ani` sources never reach this module: they are already Windows
//! cursors and are copied through by [`crate::build`] without being rasterised.
//!
//! Vector art is re-rendered at each output size, so it is exact at every DPI.
//! Raster art is resampled in premultiplied space with Lanczos3, which avoids the
//! dark halo that straight-alpha resizing produces around soft cursor edges.

use crate::manifest::Grid;
use crate::{Error, Result, MIN_SOURCE_PX};
use image::imageops::FilterType;
use image::{GenericImageView, RgbaImage};
use std::path::{Path, PathBuf};

/// Something worth telling the user about that is not fatal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// Source was not square, so it was padded. Cursors must be square.
    Padded {
        path: PathBuf,
        from: (u32, u32),
        to: u32,
    },
    /// Source is smaller than the largest requested size, so that size is an upscale.
    Upscaled {
        path: PathBuf,
        native: u32,
        requested: u32,
    },
    /// A ready-made `.cur`/`.ani` was used as-is, and the sizes it happens to
    /// contain are all Windows will ever have for that pointer.
    LowResolutionSource {
        role: String,
        largest: u32,
    },
    /// A ready-made source's larger sizes were synthesized from its largest
    /// embedded image (role's `upscale` mode), rather than left for Windows
    /// to stretch on the fly.
    ReadyMadeUpscaled {
        role: String,
        native: u32,
        added: Vec<u32>,
        mode: crate::manifest::UpscaleMode,
    },
    /// Sizes were left out of an animated cursor to stay under the Windows
    /// per-frame limit. See [`crate::ani::MAX_FRAME_BYTES`].
    AnimationSizesDropped {
        role: String,
        kept: Vec<u32>,
        dropped: Vec<u32>,
    },
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Warning::Padded { path, from, to } => write!(
                f,
                "{} is {}x{}, not square — padded to {to}x{to} with the art at the top-left. \
                 Square art avoids surprises.",
                path.display(),
                from.0,
                from.1
            ),
            Warning::Upscaled {
                path,
                native,
                requested,
            } => write!(
                f,
                "{} is only {native}px, so the {requested}px version is an upscale and will \
                 look soft. An SVG or larger PNG would fix this.",
                path.display()
            ),
            Warning::LowResolutionSource { role, largest } => write!(
                f,
                "{role} uses a ready-made cursor file whose largest image is {largest}px. \
                 It is copied through untouched, but Windows must scale it up on high-DPI \
                 screens or at large pointer sizes. Set \"upscale\": true on this role in \
                 pack.json to resample it up yourself instead — still soft above {largest}px \
                 since no upscale invents real detail, but sharper and more consistent than \
                 whatever Windows does on the fly. Only redrawing it from vector or \
                 high-resolution art gets a genuinely sharp result."
            ),
            Warning::ReadyMadeUpscaled { role, native, added, mode } => {
                let how = match mode {
                    crate::manifest::UpscaleMode::Vector => {
                        "by tracing it into vector shapes and rendering each size from that trace"
                    }
                    _ => "by resampling it",
                };
                write!(
                    f,
                    "{role} is a ready-made cursor only {native}px native; {added:?} px were \
                     synthesized {how}. They will look different from a size drawn from real \
                     art at that resolution."
                )
            }
            Warning::AnimationSizesDropped {
                role,
                kept,
                dropped,
            } => write!(
                f,
                "{role} is animated, so it keeps {kept:?} px and leaves out {dropped:?}. \
                 Windows limits how much each animation frame may hold, and its own \
                 animated cursors stop at 64px for the same reason. Static cursors are \
                 unaffected and still get every size."
            ),
        }
    }
}

/// Loaded art, ready to render at any size.
pub enum Art {
    Vector(Box<resvg::usvg::Tree>),
    Raster {
        frames: Vec<RgbaImage>,
        /// Per-frame delays, when the source carried its own timing (GIF).
        delays_ms: Option<Vec<u32>>,
    },
}

impl std::fmt::Debug for Art {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Art::Vector(_) => write!(f, "Art::Vector"),
            Art::Raster { frames, delays_ms } => f
                .debug_struct("Art::Raster")
                .field("frames", &frames.len())
                .field("has_delays", &delays_ms.is_some())
                .finish(),
        }
    }
}

impl Art {
    pub fn frame_count(&self) -> usize {
        match self {
            Art::Vector(_) => 1,
            Art::Raster { frames, .. } => frames.len(),
        }
    }

    pub fn is_animated(&self) -> bool {
        self.frame_count() > 1
    }

    /// Native pixel size, or `None` for vector art which has no fixed size.
    pub fn native_size(&self) -> Option<u32> {
        match self {
            Art::Vector(_) => None,
            Art::Raster { frames, .. } => frames.first().map(|f| f.width()),
        }
    }

    pub fn delays_ms(&self) -> Option<&[u32]> {
        match self {
            Art::Raster { delays_ms, .. } => delays_ms.as_deref(),
            _ => None,
        }
    }

    /// Load art from a path. `grid` marks the source as a sprite sheet.
    pub fn load(path: &Path, grid: Option<Grid>) -> Result<(Art, Vec<Warning>)> {
        if path.is_dir() {
            return load_sequence(path);
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        match ext.as_str() {
            "svg" | "svgz" => load_svg(path).map(|a| (a, Vec::new())),
            "gif" => load_gif(path),
            _ => match grid {
                Some(g) => load_sheet(path, g),
                None => load_static(path),
            },
        }
    }

    /// Render every frame at `size`, as straight-alpha RGBA.
    pub fn render(&self, size: u32) -> Result<Vec<Vec<u8>>> {
        match self {
            Art::Vector(tree) => Ok(vec![render_svg(tree, size)?]),
            Art::Raster { frames, .. } => frames
                .iter()
                .map(|f| Ok(resize_square(f, size).into_raw()))
                .collect(),
        }
    }

    /// Warnings that depend on the sizes being requested.
    pub fn size_warnings(&self, path: &Path, sizes: &[u32]) -> Vec<Warning> {
        let Some(native) = self.native_size() else {
            return Vec::new(); // vector art is exact at any size
        };
        sizes
            .iter()
            .filter(|&&s| s > native)
            .map(|&s| Warning::Upscaled {
                path: path.to_path_buf(),
                native,
                requested: s,
            })
            .collect()
    }
}

// ---------------------------------------------------------------- vector

/// Resize a square RGBA buffer, resampling in premultiplied space.
///
/// Public because the app icon needs the same halo-free downscale the cursors get.
pub fn resize_rgba(rgba: &[u8], from: u32, to: u32) -> Result<Vec<u8>> {
    let img = RgbaImage::from_raw(from, from, rgba.to_vec())
        .ok_or_else(|| Error::Other(format!("buffer is not {from}x{from} RGBA")))?;
    Ok(resize_square(&img, to).into_raw())
}

/// Rasterize SVG bytes to a square RGBA buffer. Used for the app and tray icons,
/// so they come from the same renderer as the cursors themselves.
pub fn render_svg_bytes(data: &[u8], size: u32) -> Result<Vec<u8>> {
    let tree =
        resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).map_err(|e| {
            Error::Svg {
                path: PathBuf::from("<embedded>"),
                msg: e.to_string(),
            }
        })?;
    render_svg(&tree, size)
}

fn load_svg(path: &Path) -> Result<Art> {
    let data = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(&data, &opt).map_err(|e| Error::Svg {
        path: path.to_path_buf(),
        msg: e.to_string(),
    })?;
    Ok(Art::Vector(Box::new(tree)))
}

/// Trace a square straight-alpha RGBA buffer into vector shapes, for the
/// `UpscaleMode::Vector` path: a ready-made source's larger sizes are then
/// rendered from this trace instead of resampled, so their edges stay crisp
/// no matter how far past the source's native size they go.
///
/// Speckle filtering and spline-fit corners (vtracer's defaults) are the
/// "cleanup" pass — they discard stray anti-aliasing noise from the source
/// bitmap and smooth pixel-staircase edges into curves, rather than tracing
/// every jagged pixel boundary literally.
pub fn trace_to_vector(rgba: &[u8], size: u32) -> Result<resvg::usvg::Tree> {
    let img = vtracer::ColorImage {
        pixels: rgba.to_vec(),
        width: size as usize,
        height: size as usize,
    };
    let svg = vtracer::convert(img, vtracer::Config::default())
        .map_err(|e| Error::Other(format!("vectorizing failed: {e}")))?;
    let data = svg.to_string();
    resvg::usvg::Tree::from_data(data.as_bytes(), &resvg::usvg::Options::default()).map_err(|e| {
        Error::Svg {
            path: PathBuf::from("<traced>"),
            msg: e.to_string(),
        }
    })
}

/// Render an already-loaded vector tree to a square RGBA buffer. Exposed for
/// [`crate::build`], which renders one trace at several sizes.
pub fn render_vector(tree: &resvg::usvg::Tree, size: u32) -> Result<Vec<u8>> {
    render_svg(tree, size)
}

fn render_svg(tree: &resvg::usvg::Tree, size: u32) -> Result<Vec<u8>> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)
        .ok_or_else(|| Error::Other(format!("could not allocate a {size}x{size} canvas")))?;

    // Fit the drawing into the square canvas, preserving aspect ratio.
    let s = tree.size();
    let scale = (size as f32 / s.width().max(1.0)).min(size as f32 / s.height().max(1.0));
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);

    resvg::render(tree, transform, &mut pixmap.as_mut());
    Ok(unpremultiply(pixmap.take()))
}

// ---------------------------------------------------------------- raster

fn load_static(path: &Path) -> Result<(Art, Vec<Warning>)> {
    let img = image::open(path).map_err(|e| Error::Image {
        path: path.to_path_buf(),
        msg: e.to_string(),
    })?;
    let (w, h) = img.dimensions();
    if w.min(h) < MIN_SOURCE_PX {
        return Err(Error::SourceTooSmall {
            path: path.to_path_buf(),
            got: w.min(h),
            want: MIN_SOURCE_PX,
        });
    }
    let (square, warn) = to_square(img.to_rgba8(), path);
    Ok((
        Art::Raster {
            frames: vec![square],
            delays_ms: None,
        },
        warn,
    ))
}

fn load_sequence(dir: &Path) -> Result<(Art, Vec<Warning>)> {
    let mut numbered: Vec<(usize, PathBuf)> = std::fs::read_dir(dir)
        .map_err(|e| Error::io(dir, e))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "webp" | "bmp"))
        })
        .filter_map(|p| trailing_number(&p).map(|n| (n, p)))
        .collect();

    if numbered.is_empty() {
        return Err(Error::NoFrames {
            dir: dir.to_path_buf(),
        });
    }
    numbered.sort_by_key(|(n, _)| *n);

    // Frames must be contiguous or the animation silently skips.
    let first = numbered[0].0;
    let last = numbered[numbered.len() - 1].0;
    let expected = last - first + 1;
    if numbered.len() != expected {
        let present: std::collections::HashSet<usize> = numbered.iter().map(|(n, _)| *n).collect();
        let missing: Vec<String> = (first..=last)
            .filter(|n| !present.contains(n))
            .take(10)
            .map(|n| n.to_string())
            .collect();
        return Err(Error::FrameGap {
            dir: dir.to_path_buf(),
            found: numbered.len(),
            max: last,
            first,
            missing: missing.join(", "),
        });
    }

    let mut frames = Vec::with_capacity(numbered.len());
    let mut warnings = Vec::new();
    let mut dims: Option<(u32, u32)> = None;

    for (_, path) in &numbered {
        let img = image::open(path).map_err(|e| Error::Image {
            path: path.clone(),
            msg: e.to_string(),
        })?;
        let (w, h) = img.dimensions();
        match dims {
            None => {
                if w.min(h) < MIN_SOURCE_PX {
                    return Err(Error::SourceTooSmall {
                        path: path.clone(),
                        got: w.min(h),
                        want: MIN_SOURCE_PX,
                    });
                }
                dims = Some((w, h));
            }
            Some((fw, fh)) if (w, h) != (fw, fh) => {
                return Err(Error::FrameSizeMismatch {
                    path: path.clone(),
                    w,
                    h,
                    fw,
                    fh,
                });
            }
            _ => {}
        }
        let (square, mut warn) = to_square(img.to_rgba8(), path);
        frames.push(square);
        warnings.append(&mut warn);
    }

    warnings.dedup();
    Ok((
        Art::Raster {
            frames,
            delays_ms: None,
        },
        warnings,
    ))
}

fn load_sheet(path: &Path, grid: Grid) -> Result<(Art, Vec<Warning>)> {
    let img = image::open(path)
        .map_err(|e| Error::Image {
            path: path.to_path_buf(),
            msg: e.to_string(),
        })?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());

    if w % grid.cols != 0 || h % grid.rows != 0 {
        return Err(Error::SheetNotDivisible {
            path: path.to_path_buf(),
            w,
            h,
            cols: grid.cols,
            rows: grid.rows,
        });
    }
    let capacity = grid.cols * grid.rows;
    let count = grid.frames.unwrap_or(capacity);
    if count == 0 || count > capacity {
        return Err(Error::SheetFrameCount {
            path: path.to_path_buf(),
            frames: count,
            cols: grid.cols,
            rows: grid.rows,
            capacity,
        });
    }

    let (cw, ch) = (w / grid.cols, h / grid.rows);
    if cw.min(ch) < MIN_SOURCE_PX {
        return Err(Error::SourceTooSmall {
            path: path.to_path_buf(),
            got: cw.min(ch),
            want: MIN_SOURCE_PX,
        });
    }

    let mut frames = Vec::with_capacity(count as usize);
    let mut warnings = Vec::new();
    for i in 0..count {
        let (cx, cy) = (i % grid.cols, i / grid.cols);
        let cell = image::imageops::crop_imm(&img, cx * cw, cy * ch, cw, ch).to_image();
        let (square, mut warn) = to_square(cell, path);
        frames.push(square);
        warnings.append(&mut warn);
    }
    warnings.dedup();
    Ok((
        Art::Raster {
            frames,
            delays_ms: None,
        },
        warnings,
    ))
}

fn load_gif(path: &Path) -> Result<(Art, Vec<Warning>)> {
    use image::AnimationDecoder;

    let file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file)).map_err(|e| {
        Error::Image {
            path: path.to_path_buf(),
            msg: e.to_string(),
        }
    })?;
    let raw = decoder.into_frames().collect_frames().map_err(|e| Error::Image {
        path: path.to_path_buf(),
        msg: e.to_string(),
    })?;

    if raw.is_empty() {
        return Err(Error::NoFrames {
            dir: path.to_path_buf(),
        });
    }

    let mut frames = Vec::with_capacity(raw.len());
    let mut delays = Vec::with_capacity(raw.len());
    let mut warnings = Vec::new();

    for f in raw {
        let (num, den) = f.delay().numer_denom_ms();
        delays.push(if den == 0 { 100 } else { num / den.max(1) });
        let (square, mut warn) = to_square(f.into_buffer(), path);
        frames.push(square);
        warnings.append(&mut warn);
    }

    let native = frames[0].width();
    if native < MIN_SOURCE_PX {
        return Err(Error::SourceTooSmall {
            path: path.to_path_buf(),
            got: native,
            want: MIN_SOURCE_PX,
        });
    }

    warnings.dedup();
    Ok((
        Art::Raster {
            frames,
            delays_ms: Some(delays),
        },
        warnings,
    ))
}

// ---------------------------------------------------------------- helpers

/// Extract the trailing integer from a file stem, e.g. `walk_007` -> 7.
fn trailing_number(path: &Path) -> Option<usize> {
    let stem = path.file_stem()?.to_str()?;
    let digits: String = stem
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}

/// Pad a possibly-rectangular image into a square canvas, anchored top-left so the
/// hotspot coordinate system stays predictable.
fn to_square(img: RgbaImage, path: &Path) -> (RgbaImage, Vec<Warning>) {
    let (w, h) = (img.width(), img.height());
    if w == h {
        return (img, Vec::new());
    }
    let side = w.max(h);
    let mut canvas = RgbaImage::new(side, side);
    image::imageops::replace(&mut canvas, &img, 0, 0);
    (
        canvas,
        vec![Warning::Padded {
            path: path.to_path_buf(),
            from: (w, h),
            to: side,
        }],
    )
}

/// Resize to a square, resampling in premultiplied space so soft edges do not
/// pick up a dark fringe.
fn resize_square(img: &RgbaImage, size: u32) -> RgbaImage {
    if img.width() == size && img.height() == size {
        return img.clone();
    }
    let mut pm = img.clone();
    premultiply_in_place(&mut pm);
    let mut out = image::imageops::resize(&pm, size, size, FilterType::Lanczos3);
    unpremultiply_in_place(&mut out);
    out
}

fn premultiply_in_place(img: &mut RgbaImage) {
    for p in img.pixels_mut() {
        let a = p[3] as u32;
        p[0] = ((p[0] as u32 * a + 127) / 255) as u8;
        p[1] = ((p[1] as u32 * a + 127) / 255) as u8;
        p[2] = ((p[2] as u32 * a + 127) / 255) as u8;
    }
}

fn unpremultiply_in_place(img: &mut RgbaImage) {
    for p in img.pixels_mut() {
        let a = p[3] as u32;
        if a == 0 {
            p[0] = 0;
            p[1] = 0;
            p[2] = 0;
        } else {
            p[0] = ((p[0] as u32 * 255 + a / 2) / a).min(255) as u8;
            p[1] = ((p[1] as u32 * 255 + a / 2) / a).min(255) as u8;
            p[2] = ((p[2] as u32 * 255 + a / 2) / a).min(255) as u8;
        }
    }
}

/// tiny-skia hands back premultiplied RGBA; cursors want straight alpha.
fn unpremultiply(mut data: Vec<u8>) -> Vec<u8> {
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else {
            px[0] = ((px[0] as u32 * 255 + a / 2) / a).min(255) as u8;
            px[1] = ((px[1] as u32 * 255 + a / 2) / a).min(255) as u8;
            px[2] = ((px[2] as u32 * 255 + a / 2) / a).min(255) as u8;
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_trailing_frame_numbers() {
        assert_eq!(trailing_number(Path::new("walk_007.png")), Some(7));
        assert_eq!(trailing_number(Path::new("f0.png")), Some(0));
        assert_eq!(trailing_number(Path::new("spin-12.png")), Some(12));
        assert_eq!(trailing_number(Path::new("frame.png")), None);
        // A leading number must not be mistaken for a frame index.
        assert_eq!(trailing_number(Path::new("3d_arrow.png")), None);
    }

    #[test]
    fn premultiply_round_trips_within_rounding_error() {
        let mut img = RgbaImage::from_fn(4, 4, |x, y| {
            image::Rgba([200, 100, 50, ((x * 4 + y) * 16).min(255) as u8])
        });
        let original = img.clone();
        premultiply_in_place(&mut img);
        unpremultiply_in_place(&mut img);

        for (a, b) in original.pixels().zip(img.pixels()) {
            assert_eq!(a[3], b[3], "alpha must be exact");
            if a[3] > 32 {
                for c in 0..3 {
                    let diff = (a[c] as i32 - b[c] as i32).abs();
                    assert!(diff <= 8, "channel drifted by {diff} at alpha {}", a[3]);
                }
            }
        }
    }

    /// Resizing straight-alpha RGBA bleeds black into soft edges. Resizing in
    /// premultiplied space must not, so a white shape stays white where visible.
    #[test]
    fn downscaling_does_not_darken_soft_edges() {
        // White, with alpha falling to zero at the right edge.
        let img = RgbaImage::from_fn(256, 256, |x, _| {
            image::Rgba([255, 255, 255, 255 - (x as f32 / 255.0 * 255.0) as u8])
        });
        let small = resize_square(&img, 32);
        for p in small.pixels() {
            if p[3] > 16 {
                assert!(
                    p[0] > 230 && p[1] > 230 && p[2] > 230,
                    "edge pixel darkened to {:?} — premultiply step is wrong",
                    p
                );
            }
        }
    }

    #[test]
    fn non_square_art_is_padded_and_reported() {
        let img = RgbaImage::new(200, 128);
        let (sq, warn) = to_square(img, Path::new("a.png"));
        assert_eq!((sq.width(), sq.height()), (200, 200));
        assert_eq!(
            warn,
            vec![Warning::Padded {
                path: "a.png".into(),
                from: (200, 128),
                to: 200
            }]
        );
    }

    #[test]
    fn square_art_is_left_alone() {
        let (sq, warn) = to_square(RgbaImage::new(128, 128), Path::new("a.png"));
        assert_eq!((sq.width(), sq.height()), (128, 128));
        assert!(warn.is_empty());
    }

    #[test]
    fn svg_renders_at_every_size_with_correct_buffer_length() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
            <path d="M2 2 L2 24 L8 18 L12 28 L16 26 L12 16 L20 16 Z" fill="white" stroke="black"/>
        </svg>"#;
        let tree =
            resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()).unwrap();
        let art = Art::Vector(Box::new(tree));

        for &size in crate::DEFAULT_SIZES {
            let frames = art.render(size).unwrap();
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0].len(), (size * size * 4) as usize);
            assert!(
                frames[0].chunks_exact(4).any(|p| p[3] > 0),
                "{size}px render came out empty"
            );
        }
        // Vector art never triggers an upscale warning.
        assert!(art.size_warnings(Path::new("a.svg"), &[256]).is_empty());
    }

    #[test]
    fn raster_art_warns_when_a_requested_size_would_upscale() {
        let art = Art::Raster {
            frames: vec![RgbaImage::new(128, 128)],
            delays_ms: None,
        };
        let w = art.size_warnings(Path::new("a.png"), &[32, 64, 128, 256]);
        assert_eq!(w.len(), 1);
        assert!(matches!(
            w[0],
            Warning::Upscaled {
                native: 128,
                requested: 256,
                ..
            }
        ));
    }
}
