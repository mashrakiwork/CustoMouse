//! Coverage for the drawn-art pipeline: SVG sources and numbered PNG sequences.
//!
//! The bundled pack is made of ready-made `.cur`/`.ani` files, which take the
//! pass-through path and never touch the rasteriser. These fixtures are built at
//! run time so the rendering path stays tested without shipping sample art.

use cursorpack::{ani, build, ico, manifest::Pack, DEFAULT_SIZES};
use image::RgbaImage;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join("custo-synthetic").join(name);
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(p.join("art")).unwrap();
    p
}

const ARROW_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
  <path d="M2 2 L2 24 L8 18 L12 28 L16 26 L12 16 L20 16 Z" fill="white" stroke="black"/>
</svg>"#;

/// Write `n` frames of a moving square, so consecutive frames genuinely differ.
fn write_sequence(dir: &Path, n: u32, size: u32) {
    std::fs::create_dir_all(dir).unwrap();
    for f in 0..n {
        let img = RgbaImage::from_fn(size, size, |x, y| {
            let cx = size / 4 + (f * size / (2 * n));
            let inside = x.abs_diff(cx) < size / 6 && y.abs_diff(size / 2) < size / 6;
            if inside {
                image::Rgba([240, 240, 240, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            }
        });
        img.save(dir.join(format!("f_{f:02}.png"))).unwrap();
    }
}

/// A vector source must render at every size, with the hotspot scaled into each.
#[test]
fn svg_sources_render_at_every_size_with_scaled_hotspots() {
    let dir = scratch("svg");
    std::fs::write(dir.join("art/arrow.svg"), ARROW_SVG).unwrap();
    std::fs::write(
        dir.join("pack.json"),
        r#"{ "name": "Synth SVG", "roles": {
              "Arrow": { "source": "art/arrow.svg", "hotspot": [2, 2] } } }"#,
    )
    .unwrap();

    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();
    assert_eq!(built.cursors.len(), 1);

    let arrow = &built.cursors[0];
    assert!(!arrow.animated);
    assert_eq!(arrow.sizes, DEFAULT_SIZES.to_vec());

    let images = ico::decode(&arrow.data).unwrap();
    assert_eq!(images.len(), DEFAULT_SIZES.len());
    for img in &images {
        let expect = pack.scaled_hotspot(&pack.roles["Arrow"], img.size);
        assert_eq!((img.hot_x, img.hot_y), expect, "hotspot at {}px", img.size);
        assert!(
            img.rgba.chunks_exact(4).any(|p| p[3] > 0),
            "{}px render came out empty",
            img.size
        );
    }

    // Authored at [2,2] on a 32px canvas, so 128px must be [8,8].
    let big = images.iter().find(|i| i.size == 128).unwrap();
    assert_eq!((big.hot_x, big.hot_y), (8, 8));
}

/// A numbered PNG folder must become an animation at the requested frame rate,
/// trimmed to the sizes an `.ani` frame may hold.
#[test]
fn png_sequences_become_animations_at_the_requested_rate() {
    let dir = scratch("seq");
    write_sequence(&dir.join("art/spin"), 12, 256);
    std::fs::write(
        dir.join("pack.json"),
        r#"{ "name": "Synth Seq", "roles": {
              "Wait": { "source": "art/spin", "hotspot": [16, 16], "fps": 20 } } }"#,
    )
    .unwrap();

    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();
    let wait = &built.cursors[0];

    assert!(wait.animated);
    assert_eq!(wait.frames, 12);
    assert_eq!(wait.file_name, "Wait.ani");

    let raw = ani::parse(&wait.data).unwrap();
    assert_eq!(raw.header.frames, 12);
    assert_eq!(raw.header.display_rate, 3, "20 fps is 3 jiffies");

    let decoded = ani::decode(&wait.data).unwrap();
    assert!((decoded.duration_ms() - 600.0).abs() < 1.0, "12 frames at 20fps is 600ms");

    // Animated roles are budgeted down; static ones are not.
    assert_eq!(decoded.sizes(), vec![32, 48, 64]);
    assert!(ico::frame_bytes(&wait.sizes) <= ani::MAX_FRAME_BYTES);
    assert!(built
        .warnings
        .iter()
        .any(|w| matches!(w, cursorpack::raster::Warning::AnimationSizesDropped { .. })));

    // Frames must actually differ, or it is a still image pretending to animate.
    let differing = decoded
        .frames
        .windows(2)
        .filter(|w| {
            let a = w[0].iter().find(|i| i.size == 64).unwrap();
            let b = w[1].iter().find(|i| i.size == 64).unwrap();
            a.rgba != b.rgba
        })
        .count();
    assert_eq!(differing, decoded.frames.len() - 1);
}

/// Gaps in frame numbering silently drop frames, so they must be an error.
#[test]
fn frame_number_gaps_are_reported() {
    let dir = scratch("gap");
    let art = dir.join("art/spin");
    write_sequence(&art, 6, 256);
    std::fs::remove_file(art.join("f_03.png")).unwrap();
    std::fs::write(
        dir.join("pack.json"),
        r#"{ "name": "Gap", "roles": { "Wait": { "source": "art/spin" } } }"#,
    )
    .unwrap();

    let pack = Pack::load(&dir).unwrap();
    let err = build::build(&pack, &dir).unwrap_err();
    assert!(
        matches!(err, cursorpack::Error::FrameGap { .. }),
        "expected a frame-gap error, got: {err}"
    );
    assert!(err.to_string().contains('3'), "the error should name the missing frame");
}

/// Art too small to make a sharp cursor is refused rather than silently upscaled.
#[test]
fn undersized_art_is_refused() {
    let dir = scratch("small");
    RgbaImage::from_pixel(48, 48, image::Rgba([255, 255, 255, 255]))
        .save(dir.join("art/tiny.png"))
        .unwrap();
    std::fs::write(
        dir.join("pack.json"),
        r#"{ "name": "Tiny", "roles": { "Arrow": { "source": "art/tiny.png" } } }"#,
    )
    .unwrap();

    let pack = Pack::load(&dir).unwrap();
    let err = build::build(&pack, &dir).unwrap_err();
    assert!(
        matches!(err, cursorpack::Error::SourceTooSmall { .. }),
        "expected a too-small error, got: {err}"
    );
}
