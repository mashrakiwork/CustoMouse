//! End-to-end checks on the bundled "Inverted Default" pack.
//!
//! That pack is generated from this machine's own Windows cursors
//! (`cargo run -p cursorpack --example gen_inverted -- packs/inverted-default`),
//! so these tests also confirm the generator produced something coherent.

use cursorpack::{ani, build, ico, manifest::Pack, DEFAULT_SIZES};
use std::path::PathBuf;

fn pack_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packs/inverted-default")
        .canonicalize()
        .expect("bundled pack should exist")
}

fn load() -> (Pack, build::Built) {
    let dir = pack_dir();
    let pack = Pack::load(&dir).expect("pack.json should load");
    let built = build::build(&pack, &dir).expect("pack should build");
    (pack, built)
}

#[test]
fn bundled_pack_covers_every_pointer_role() {
    let (pack, built) = load();
    let expected = pack.active_roles().len();
    assert!(
        expected >= 15,
        "the bundled pack should cover the Windows pointer set, got {expected}"
    );
    assert_eq!(built.cursors.len(), expected);

    for c in &built.cursors {
        let ext = if c.animated { "ani" } else { "cur" };
        assert_eq!(c.file_name, format!("{}.{ext}", c.role));
    }
}

/// Static pointers are rendered from the Windows SVG sources, so they should
/// carry the full size range — sharper than the stock bitmaps they replace.
#[test]
fn static_pointers_carry_every_size() {
    let (_, built) = load();
    let mut checked = 0;
    for c in built.cursors.iter().filter(|c| !c.animated) {
        assert_eq!(
            c.sizes,
            DEFAULT_SIZES.to_vec(),
            "{} should be rendered at every size",
            c.role
        );
        assert_eq!(ico::decode(&c.data).unwrap().len(), DEFAULT_SIZES.len());
        checked += 1;
    }
    assert!(checked >= 13, "expected most roles to be static, got {checked}");
}

/// The spinners are inverted copies of the Windows `.ani` files, so their frame
/// count and timing must survive untouched.
#[test]
fn animated_pointers_keep_the_windows_timing() {
    let (_, built) = load();
    let animated: Vec<_> = built.cursors.iter().filter(|c| c.animated).collect();
    assert_eq!(animated.len(), 2, "Wait and AppStarting are the animated roles");

    for c in animated {
        let raw = ani::parse(&c.data).unwrap();
        assert_eq!(raw.header.frames as usize, c.frames);
        assert!(raw.header.frames >= 8, "{} has too few frames", c.role);
        assert_eq!(
            raw.header.display_rate, 3,
            "{} should keep the stock 50ms per frame",
            c.role
        );

        // And must still fit the loader's per-frame ceiling.
        let per_frame = ico::frame_bytes(&c.sizes);
        assert!(
            per_frame <= ani::MAX_FRAME_BYTES,
            "{} uses {per_frame} bytes per frame",
            c.role
        );
    }
}

/// Every pointer must actually be drawn, at every size it claims.
#[test]
fn every_pointer_has_visible_pixels_at_every_size() {
    let (_, built) = load();
    for c in &built.cursors {
        let frames = if c.animated {
            ani::decode(&c.data).unwrap().frames
        } else {
            vec![ico::decode(&c.data).unwrap()]
        };
        for (n, frame) in frames.iter().enumerate() {
            for img in frame {
                let opaque = img.rgba.chunks_exact(4).filter(|p| p[3] > 32).count();
                let total = (img.size * img.size) as usize;
                assert!(
                    opaque > total / 500,
                    "{} frame {n} at {}px is effectively blank",
                    c.role,
                    img.size
                );
            }
        }
    }
}

/// Hotspots must be present, inside the canvas, and grow with the image. One that
/// stayed at its 32px value would click in the wrong place at large cursor sizes.
#[test]
fn hotspots_are_present_and_scale_with_size() {
    let (_, built) = load();

    let move_cursor = built
        .cursors
        .iter()
        .find(|c| c.role == "SizeAll")
        .expect("SizeAll should be in the pack");
    let images = ico::decode(&move_cursor.data).unwrap();

    for img in &images {
        assert!(
            (img.hot_x as u32) < img.size && (img.hot_y as u32) < img.size,
            "SizeAll hotspot {:?} outside {}px canvas",
            (img.hot_x, img.hot_y),
            img.size
        );
    }

    // The move pointer is centred on its artwork, so its hotspot must scale.
    let small = images.iter().find(|i| i.size == 32).unwrap();
    let large = images.iter().find(|i| i.size == 128).unwrap();
    assert!(
        large.hot_x > small.hot_x * 3,
        "hotspot did not scale: {} at 32px vs {} at 128px",
        small.hot_x,
        large.hot_x
    );
}

/// The pack claims to be the Windows pointers, inverted, in black and white.
/// Check all three claims against `aero_arrow.cur` — the actual shipped bitmap.
///
/// An earlier version of this test compared against `arrow.svg`, the same file the
/// pack is generated from. That is self-referential: it reported a perfect match
/// while every pointer was upside down, because Microsoft stores those SVGs
/// mirrored relative to the bitmaps. Comparing against the bitmap catches it.
#[test]
fn the_art_matches_windows_inverted_and_monochrome() {
    let stock = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()))
        .join("Cursors")
        .join("aero_arrow.cur");
    if !stock.exists() {
        eprintln!("no aero_arrow.cur on this machine; skipping");
        return;
    }

    let theirs = ico::decode(&std::fs::read(&stock).unwrap()).unwrap();
    let theirs = theirs.iter().find(|i| i.size == 64).expect("64px stock arrow");

    let (_, built) = load();
    let arrow = built.cursors.iter().find(|c| c.role == "Arrow").unwrap();
    let decoded = ico::decode(&arrow.data).unwrap();
    let ours = decoded.iter().find(|i| i.size == 64).expect("64px arrow");

    // 1. Strictly black and white: no channel may differ from the others.
    let coloured = ours
        .rgba
        .chunks_exact(4)
        .filter(|p| p[3] > 8 && (p[0] != p[1] || p[1] != p[2]))
        .count();
    assert_eq!(coloured, 0, "{coloured} pixels still carry colour; the pack must be greyscale");

    // 2. Same way up. Coarse 8x8 alpha grids: the upright comparison must match
    //    better than the vertically mirrored one, which is what a flip would do.
    let grid = |img: &ico::Image, flip: bool| -> Vec<f32> {
        let n = 8usize;
        let cell = img.size as usize / n;
        let mut out = vec![0.0; n * n];
        for gy in 0..n {
            for gx in 0..n {
                let mut sum = 0.0;
                for y in 0..cell {
                    for x in 0..cell {
                        let sy = if flip { img.size as usize - 1 - (gy * cell + y) } else { gy * cell + y };
                        let sx = gx * cell + x;
                        sum += img.rgba[(sy * img.size as usize + sx) * 4 + 3] as f32;
                    }
                }
                out[gy * n + gx] = sum / (cell * cell) as f32;
            }
        }
        out
    };

    let ours_g = grid(ours, false);
    let upright = grid(theirs, false);
    let mirrored = grid(theirs, true);
    let diff = |a: &[f32], b: &[f32]| -> f32 {
        a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len() as f32
    };

    let d_upright = diff(&ours_g, &upright);
    let d_mirrored = diff(&ours_g, &mirrored);
    eprintln!("alpha grid difference: upright {d_upright:.1}, mirrored {d_mirrored:.1}");
    assert!(
        d_upright < d_mirrored * 0.6,
        "the pointer looks vertically flipped: upright difference {d_upright:.1}          is not clearly better than mirrored {d_mirrored:.1}"
    );

    // 3. Actually inverted. Measured per pixel over solid area, not per cell:
    //    a coarse grid lets sparse edge cells outweigh dense ones and washes the
    //    difference out. Ours is rendered from vector while theirs is a bitmap,
    //    so their edges never align — polarity is the claim worth testing.
    let stats = |img: &ico::Image| -> (f64, u32, u32) {
        let (mut sum, mut n, mut dark, mut light) = (0.0f64, 0.0f64, 0u32, 0u32);
        for p in img.rgba.chunks_exact(4) {
            if p[3] < 200 {
                continue;
            }
            let l = 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
            sum += l;
            n += 1.0;
            if l < 64.0 {
                dark += 1;
            } else if l > 192.0 {
                light += 1;
            }
        }
        (sum / n.max(1.0), dark, light)
    };

    let (their_mean, their_dark, their_light) = stats(theirs);
    let (our_mean, our_dark, our_light) = stats(ours);
    eprintln!(
        "mean luminance: windows {their_mean:.0} ({their_light} light / {their_dark} dark),          ours {our_mean:.0} ({our_light} light / {our_dark} dark)"
    );

    assert!(
        their_light > their_dark * 2,
        "expected the stock arrow to be mostly white, got {their_light} light vs {their_dark} dark"
    );
    assert!(
        our_dark > our_light,
        "the inverted arrow should be mostly black, got {our_light} light vs {our_dark} dark"
    );
    assert!(
        their_mean - our_mean > 70.0,
        "not enough polarity change: windows {their_mean:.0} vs ours {our_mean:.0}"
    );
}

#[test]
fn built_files_can_be_written_to_disk() {
    let (pack, built) = load();
    let out = std::env::temp_dir().join("custo-mouse-test-build");
    let _ = std::fs::remove_dir_all(&out);
    let written = built.write_to(&out).unwrap();

    assert_eq!(written.len(), pack.active_roles().len());
    for (role, path) in &written {
        let meta = std::fs::metadata(path).unwrap_or_else(|e| panic!("{role}: {e}"));
        assert!(meta.len() > 1000, "{role} is suspiciously small");
    }
    std::fs::remove_dir_all(&out).ok();
}
