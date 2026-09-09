//! Build the "Inverted Default" pack from this machine's own Windows cursors.
//!
//! ```text
//! cargo run -p cursorpack --example gen_inverted -- packs/inverted-default
//! ```
//!
//! Windows 11 ships SVG sources for its pointers in `C:\Windows\Cursors`
//! alongside the `aero_*.cur/.ani` bitmaps. The SVGs are the better input: they
//! re-render cleanly at all seven sizes, so the result is sharper than the stock
//! bitmaps it is derived from. Hotspots are read from the matching `.cur`,
//! because rendering an SVG cannot tell you where the click point is.
//!
//! The two spinners are animated, so those are taken from the `.ani` files with
//! their pixels inverted, preserving frame count and timing exactly.

use cursorpack::manifest::{Pack, RoleSpec};
use cursorpack::{ani, ico, raster, DEFAULT_SIZES};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// role, SVG source, bitmap source (for the hotspot, or the art when animated),
/// fallback hotspot in 32px space when there is no bitmap to read one from.
const ROLES: &[(&str, Option<&str>, Option<&str>, (u32, u32))] = &[
    ("Arrow", Some("arrow.svg"), Some("aero_arrow.cur"), (0, 0)),
    ("Help", Some("helpsel.svg"), Some("aero_helpsel.cur"), (0, 0)),
    ("AppStarting", None, Some("aero_working.ani"), (0, 0)),
    ("Wait", None, Some("aero_busy.ani"), (16, 16)),
    ("Crosshair", Some("cross.svg"), None, (16, 16)),
    ("IBeam", Some("ibeam.svg"), None, (16, 16)),
    ("NWPen", Some("pen.svg"), Some("aero_pen.cur"), (0, 0)),
    ("No", Some("unavail.svg"), Some("aero_unavail.cur"), (16, 16)),
    ("SizeNS", Some("ns.svg"), Some("aero_ns.cur"), (16, 16)),
    ("SizeWE", Some("ew.svg"), Some("aero_ew.cur"), (16, 16)),
    ("SizeNWSE", Some("nwse.svg"), Some("aero_nwse.cur"), (16, 16)),
    ("SizeNESW", Some("nesw.svg"), Some("aero_nesw.cur"), (16, 16)),
    ("SizeAll", Some("move.svg"), Some("aero_move.cur"), (16, 16)),
    ("UpArrow", Some("up.svg"), Some("aero_up.cur"), (16, 3)),
    ("Hand", Some("link.svg"), Some("aero_link.cur"), (10, 3)),
    ("Pin", Some("pin.svg"), Some("aero_pin.cur"), (5, 3)),
    ("Person", Some("person.svg"), Some("aero_person.cur"), (5, 3)),
];

/// Convert to greyscale and invert: black where it was white, white where it was
/// black, and no colour anywhere.
///
/// A plain per-channel RGB inversion is *not* what is wanted here — it turns the
/// blue busy ring orange rather than grey. Inverting luminance instead keeps the
/// whole pack strictly black and white.
///
/// Fully transparent pixels are skipped: their colour channels are usually zero,
/// and turning them white would bleed a halo when the image is downscaled.
fn invert(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        if px[3] == 0 {
            continue;
        }
        // Rec. 709 luminance.
        let luma = 0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32;
        let v = (255.0 - luma).round().clamp(0.0, 255.0) as u8;
        px[0] = v;
        px[1] = v;
        px[2] = v;
    }
}

/// Mirror an RGBA buffer top to bottom, in place.
///
/// The SVGs Microsoft ships in `C:\Windows\Cursors` are stored upside down
/// relative to the matching `aero_*.cur` — they look exported from the bottom-up
/// DIB rows. Rendering them as-is produces cursors that point the wrong way.
fn flip_vertically(rgba: &mut [u8], size: u32) {
    let stride = (size * 4) as usize;
    for y in 0..(size as usize) / 2 {
        let (top, bottom) = rgba.split_at_mut((y + 1) * stride);
        let top_row = &mut top[y * stride..];
        let bottom_row = &mut bottom[(size as usize - 2 * y - 2) * stride..][..stride];
        top_row[..stride].swap_with_slice(bottom_row);
    }
}

fn cursors_dir() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()))
        .join("Cursors")
}

/// Every hotspot a `.cur`/`.ani` declares, keyed by image size.
///
/// Windows stores these per size and they are not always an exact multiple of the
/// 32px one (`aero_move` is 11 at 32px but 45 at 128px, not 44), so the real
/// values are used wherever they exist and only extrapolated beyond them.
fn hotspots(path: &Path) -> BTreeMap<u32, (u32, u32)> {
    let Ok(data) = std::fs::read(path) else {
        return BTreeMap::new();
    };
    let frame = match ani::parse(&data) {
        Ok(raw) => match raw.frames.into_iter().next() {
            Some(f) => f,
            None => return BTreeMap::new(),
        },
        Err(_) => data,
    };
    ico::read_dir(&frame)
        .map(|entries| {
            entries
                .iter()
                .map(|e| (e.width, (e.hot_x as u32, e.hot_y as u32)))
                .collect()
        })
        .unwrap_or_default()
}

/// Hotspot for one output size: exact when Windows declares it, scaled from the
/// smallest known one otherwise.
fn hotspot_for(table: &BTreeMap<u32, (u32, u32)>, size: u32, fallback: (u32, u32)) -> (u32, u32) {
    if let Some(&hs) = table.get(&size) {
        return hs;
    }
    let (base_size, base) = table
        .iter()
        .next()
        .map(|(k, v)| (*k, *v))
        .unwrap_or((32, fallback));
    let scale = size as f32 / base_size as f32;
    (
        (base.0 as f32 * scale).round() as u32,
        (base.1 as f32 * scale).round() as u32,
    )
}

fn main() {
    let dest = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: gen_inverted <output pack dir>"),
    );
    let art = dest.join("art");
    std::fs::create_dir_all(&art).expect("create art dir");
    let src = cursors_dir();

    let mut roles: BTreeMap<String, RoleSpec> = BTreeMap::new();

    for (role, svg, bitmap, fallback_hs) in ROLES {
        let bitmap_path = bitmap.map(|b| src.join(b));
        let hs_table = bitmap_path
            .as_ref()
            .filter(|p| p.exists())
            .map(|p| hotspots(p))
            .unwrap_or_default();
        let hs = hotspot_for(&hs_table, 32, *fallback_hs);

        // Animated roles keep their frames: invert the .ani in place.
        let animated = bitmap.is_some_and(|b| b.ends_with(".ani"));
        let file_name;

        if animated {
            let Some(path) = bitmap_path.filter(|p| p.exists()) else {
                println!("  skip {role}: {} missing", bitmap.unwrap_or("?"));
                continue;
            };
            let mut anim = match ani::decode(&std::fs::read(&path).unwrap()) {
                Ok(a) => a,
                Err(e) => {
                    println!("  skip {role}: {e}");
                    continue;
                }
            };
            for frame in &mut anim.frames {
                for img in frame.iter_mut() {
                    invert(&mut img.rgba);
                }
            }
            file_name = format!("{role}.ani");
            std::fs::write(art.join(&file_name), ani::encode(&anim).unwrap()).unwrap();
            println!(
                "  {role:12} <- {} (inverted, {} frames, sizes {:?})",
                bitmap.unwrap(),
                anim.frames.len(),
                anim.sizes()
            );
        } else if let Some(svg_name) = svg.filter(|s| src.join(s).exists()) {
            // Vector source: render every size, then invert.
            let data = std::fs::read(src.join(svg_name)).unwrap();
            let mut images = Vec::new();
            for &size in DEFAULT_SIZES {
                let mut rgba = match raster::render_svg_bytes(&data, size) {
                    Ok(r) => r,
                    Err(e) => {
                        println!("  skip {role}: {e}");
                        images.clear();
                        break;
                    }
                };
                flip_vertically(&mut rgba, size);
                invert(&mut rgba);
                let (hx, hy) = hotspot_for(&hs_table, size, *fallback_hs);
                let hx = hx.min(size - 1) as u16;
                let hy = hy.min(size - 1) as u16;
                images.push(ico::Image::new(size, rgba, hx, hy).unwrap());
            }
            if images.is_empty() {
                continue;
            }
            file_name = format!("{role}.cur");
            std::fs::write(
                art.join(&file_name),
                ico::encode(&images, ico::TYPE_CURSOR).unwrap(),
            )
            .unwrap();
            println!(
                "  {role:12} <- {svg_name} (vector, inverted, sizes {:?}, hotspot {hs:?})",
                DEFAULT_SIZES
            );
        } else if let Some(path) = bitmap_path.filter(|p| p.exists()) {
            // No vector source: invert the bitmap and keep whatever sizes it had.
            let mut images = match ico::decode(&std::fs::read(&path).unwrap()) {
                Ok(i) => i,
                Err(e) => {
                    println!("  skip {role}: {e}");
                    continue;
                }
            };
            for img in &mut images {
                invert(&mut img.rgba);
            }
            file_name = format!("{role}.cur");
            std::fs::write(
                art.join(&file_name),
                ico::encode(&images, ico::TYPE_CURSOR).unwrap(),
            )
            .unwrap();
            println!("  {role:12} <- {} (bitmap, inverted)", bitmap.unwrap());
        } else {
            println!("  skip {role}: no source on this machine");
            continue;
        };

        roles.insert(
            role.to_string(),
            RoleSpec {
                source: Some(PathBuf::from("art").join(&file_name)),
                // Omitted deliberately: the generated file already carries the
                // correct hotspot for every size it contains.
                hotspot: None,
                ..Default::default()
            },
        );
    }

    let pack = Pack {
        format: 1,
        name: "Inverted Default".into(),
        author: Some("generated from Windows".into()),
        version: Some("1.0.0".into()),
        description: Some(
            "The Windows default pointers with their colours inverted: black where \
             they were white, white where they were black. Static pointers are \
             re-rendered from the Windows SVG sources at every size, so they are \
             sharper than the stock bitmaps."
                .into(),
        ),
        base_size: 32,
        sizes: DEFAULT_SIZES.to_vec(),
        roles,
    };
    pack.save(&dest).expect("write pack.json");
    println!("\n{} roles -> {}", pack.roles.len(), dest.display());

    if let Some(icon_path) = std::env::args().nth(2) {
        write_icon(&src, Path::new(&icon_path));
    }
}

/// Draw the app icon: the inverted pointer, large and at its natural
/// proportions, with a bold white plus badge overlapping its bottom-right
/// corner — the same layout as a typical "add" badge overlaid on an icon.
///
/// Built from the same inverted art as the pack, rather than hand-drawn, so the
/// icon and the cursors can never drift apart.
fn write_icon(src: &Path, out: &Path) {
    const N: u32 = 256;

    let svg = std::fs::read(src.join("arrow.svg")).expect("arrow.svg");
    let mut rgba = raster::render_svg_bytes(&svg, N).expect("render arrow");
    flip_vertically(&mut rgba, N);
    invert(&mut rgba);

    // The Windows artwork sits in a corner of its viewBox with a lot of dead
    // space. Crop to the drawn pixels, keeping the cursor's real (non-square)
    // proportions rather than padding it out to a square.
    let src_img = image::RgbaImage::from_raw(N, N, rgba).expect("square buffer");
    let (mut x0, mut y0, mut x1, mut y1) = (N, N, 0u32, 0u32);
    for (x, y, px) in src_img.enumerate_pixels() {
        if px[3] > 8 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
    let cropped = image::imageops::crop_imm(&src_img, x0, y0, bw, bh).to_image();

    // Scale to fill almost the whole canvas, at the cursor's own aspect ratio.
    let margin = (N as f32 * 0.05) as u32;
    let avail = N - 2 * margin;
    let scale = (avail as f32 / bw as f32).min(avail as f32 / bh as f32);
    let (cw, ch) = (
        (bw as f32 * scale).round() as u32,
        (bh as f32 * scale).round() as u32,
    );
    let big = image::imageops::resize(&cropped, cw, ch, image::imageops::FilterType::Lanczos3);

    // Center the (non-square) cursor within the canvas on both axes. Only one
    // of cw/ch equals `avail` — the other is smaller, since the aspect ratio
    // is preserved — so anchoring at (margin, margin) on both axes leaves the
    // shorter axis flush against one edge instead of centered.
    let ox = margin as i64 + (avail as i64 - cw as i64) / 2;
    let oy = margin as i64 + (avail as i64 - ch as i64) / 2;
    let mut canvas = image::RgbaImage::new(N, N);
    image::imageops::overlay(&mut canvas, &big, ox, oy);

    // Plus badge, overlapping the bottom-right corner of the cursor itself (not
    // floating off in empty canvas space) — a dark keyline behind a white fill,
    // so it reads whether it sits over the black cursor or the transparent
    // background behind it.
    let outer_half = N as f32 * 0.20;
    let fit = |v: f32| v.clamp(outer_half + 2.0, N as f32 - outer_half - 2.0);
    let cx = fit(ox as f32 + cw as f32 * 0.92);
    let cy = fit(oy as f32 + ch as f32 * 0.92);
    let cross = |x: u32, y: u32, half: f32, thick: f32| {
        let (dx, dy) = ((x as f32 - cx).abs(), (y as f32 - cy).abs());
        (dx <= thick && dy <= half) || (dy <= thick && dx <= half)
    };
    for y in 0..N {
        for x in 0..N {
            if cross(x, y, outer_half, N as f32 * 0.078) {
                canvas.put_pixel(x, y, image::Rgba([0x14, 0x13, 0x1a, 255]));
            }
        }
    }
    for y in 0..N {
        for x in 0..N {
            if cross(x, y, N as f32 * 0.160, N as f32 * 0.054) {
                canvas.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
    }

    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    canvas.save(out).expect("write icon");
    println!("icon -> {}", out.display());
}
