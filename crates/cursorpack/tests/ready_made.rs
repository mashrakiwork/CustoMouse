//! Using existing `.cur` / `.ani` files as pack sources.
//!
//! Windows ships a folder full of real cursors, so those are the test material:
//! files produced by Microsoft, not by us.

#![cfg(windows)]

use cursorpack::{ani, build, ico, manifest::Pack};
use std::path::{Path, PathBuf};

fn stock(name: &str) -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()))
        .join("Cursors")
        .join(name)
}

/// Build a throwaway pack directory whose sources are copies of stock cursors.
fn make_pack(dir: &Path, roles: &[(&str, &str)], hotspot: Option<[u32; 2]>) -> Pack {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    let mut entries = Vec::new();
    for (role, file) in roles {
        let src = stock(file);
        if !src.exists() {
            continue;
        }
        std::fs::copy(&src, dir.join("art").join(file)).unwrap();
        let hs = match hotspot {
            Some([x, y]) => format!(r#", "hotspot": [{x}, {y}]"#),
            None => String::new(),
        };
        entries.push(format!(
            r#""{role}": {{ "source": "art/{file}"{hs} }}"#
        ));
    }
    assert!(!entries.is_empty(), "no stock cursors available to test with");

    let json = format!(
        r#"{{ "name": "Stock Passthrough", "roles": {{ {} }} }}"#,
        entries.join(", ")
    );
    std::fs::write(dir.join("pack.json"), json).unwrap();
    Pack::load(dir).unwrap()
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join("custo-readymade").join(name)
}

/// A `.cur`/`.ani` used as a source must come out byte-for-byte identical.
/// Re-encoding would be lossless anyway, but copying proves nothing is touched.
#[test]
fn ready_made_cursors_pass_through_untouched() {
    let dir = scratch("passthrough");
    let pack = make_pack(
        &dir,
        &[
            ("Arrow", "aero_arrow.cur"),
            ("Wait", "aero_busy.ani"),
            ("Hand", "aero_link.cur"),
        ],
        None,
    );

    let built = build::build(&pack, &dir).unwrap();
    assert!(!built.cursors.is_empty());

    for c in &built.cursors {
        let source = dir
            .join("art")
            .join(if c.animated {
                "aero_busy.ani"
            } else if c.role == "Arrow" {
                "aero_arrow.cur"
            } else {
                "aero_link.cur"
            });
        let original = std::fs::read(&source).unwrap();
        assert_eq!(
            c.data, original,
            "{} should be copied through unchanged",
            c.role
        );
        assert_eq!(
            c.file_name,
            format!("{}.{}", c.role, if c.animated { "ani" } else { "cur" })
        );
    }
}

/// The animated stock cursor must keep its own frame count and timing, not be
/// re-timed to the manifest default of 20 fps.
#[test]
fn animated_source_keeps_its_own_timing() {
    let dir = scratch("timing");
    let pack = make_pack(&dir, &[("Wait", "aero_busy.ani")], None);
    let built = build::build(&pack, &dir).unwrap();

    let wait = built.cursors.iter().find(|c| c.role == "Wait").unwrap();
    assert!(wait.animated);

    let original = ani::parse(&std::fs::read(stock("aero_busy.ani")).unwrap()).unwrap();
    let ours = ani::parse(&wait.data).unwrap();
    assert_eq!(ours.header.frames, original.header.frames);
    assert_eq!(ours.header.display_rate, original.header.display_rate);
    assert_eq!(ours.header.steps, original.header.steps);
}

/// Every size embedded in the source must survive, including the large ones.
#[test]
fn embedded_sizes_are_reported_and_preserved() {
    let dir = scratch("sizes");
    let pack = make_pack(&dir, &[("Arrow", "aero_arrow.cur")], None);
    let built = build::build(&pack, &dir).unwrap();

    let arrow = &built.cursors[0];
    let decoded = ico::decode(&arrow.data).unwrap();
    let mut actual: Vec<u32> = decoded.iter().map(|i| i.size).collect();
    actual.sort_unstable();

    assert_eq!(actual, arrow.sizes, "reported sizes must match the file");
    assert!(!actual.is_empty());
}

/// Omitting `hotspot` keeps the one baked into the file; supplying one overrides it.
#[test]
fn hotspot_is_kept_unless_explicitly_overridden() {
    let original = ico::decode(&std::fs::read(stock("aero_arrow.cur")).unwrap()).unwrap();
    let biggest = original.iter().max_by_key(|i| i.size).unwrap();

    // Without an override.
    let dir = scratch("hs-keep");
    let pack = make_pack(&dir, &[("Arrow", "aero_arrow.cur")], None);
    let built = build::build(&pack, &dir).unwrap();
    let kept = ico::decode(&built.cursors[0].data).unwrap();
    let kept_big = kept.iter().max_by_key(|i| i.size).unwrap();
    assert_eq!(
        (kept_big.hot_x, kept_big.hot_y),
        (biggest.hot_x, biggest.hot_y),
        "hotspot should come from the file"
    );

    // With one, expressed in 32px base coordinates and scaled per size.
    let dir = scratch("hs-override");
    let pack = make_pack(&dir, &[("Arrow", "aero_arrow.cur")], Some([4, 3]));
    let built = build::build(&pack, &dir).unwrap();
    for img in ico::decode(&built.cursors[0].data).unwrap() {
        let expect = pack.scaled_hotspot(&pack.roles["Arrow"], img.size);
        assert_eq!((img.hot_x, img.hot_y), expect, "at {}px", img.size);
    }
}

/// A source that only carries small images should say so, since nothing downstream
/// can recover detail that is not in the file.
#[test]
fn small_ready_made_sources_are_flagged() {
    use cursorpack::ico::Image;

    let dir = scratch("small");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    let img = Image::new(32, vec![180u8; 32 * 32 * 4], 0, 0).unwrap();
    std::fs::write(
        dir.join("art/tiny.cur"),
        ico::encode(&[img], ico::TYPE_CURSOR).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("pack.json"),
        r#"{ "name": "Tiny", "roles": { "Arrow": { "source": "art/tiny.cur" } } }"#,
    )
    .unwrap();

    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();

    assert!(
        built.warnings.iter().any(|w| matches!(
            w,
            cursorpack::raster::Warning::LowResolutionSource { largest: 32, .. }
        )),
        "a 32px-only source should be flagged, got {:?}",
        built.warnings
    );
}

/// Mixing ready-made files with drawn art in one pack has to work, since that is
/// how someone extends a downloaded pack.
#[test]
fn ready_made_and_drawn_art_mix_in_one_pack() {
    let dir = scratch("mixed");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    std::fs::copy(stock("aero_arrow.cur"), dir.join("art/aero_arrow.cur")).unwrap();
    std::fs::write(
        dir.join("art/beam.svg"),
        br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
             <rect x="14" y="5" width="4" height="22" fill="white" stroke="black"/></svg>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("pack.json"),
        r#"{ "name": "Mixed", "roles": {
              "Arrow": { "source": "art/aero_arrow.cur" },
              "IBeam": { "source": "art/beam.svg", "hotspot": [16, 16] } } }"#,
    )
    .unwrap();

    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();
    assert_eq!(built.cursors.len(), 2);

    let arrow = built.cursors.iter().find(|c| c.role == "Arrow").unwrap();
    let beam = built.cursors.iter().find(|c| c.role == "IBeam").unwrap();

    // The copied one keeps whatever the file had; the drawn one gets the full set.
    assert_eq!(
        arrow.data,
        std::fs::read(dir.join("art/aero_arrow.cur")).unwrap()
    );
    assert_eq!(beam.sizes, cursorpack::DEFAULT_SIZES.to_vec());
}

/// The whole downloaded-pack workflow: a folder of cursor files plus an
/// install.inf, imported and compiled into something Windows will accept.
#[test]
fn a_downloaded_style_folder_imports_end_to_end() {
    use cursorpack::import;

    let dir = scratch("downloaded");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Mimic how cursor sets ship: arbitrary file names, mapped by install.inf.
    let files = [
        ("aero_arrow.cur", "pointer.cur"),
        ("aero_busy.ani", "wait.ani"),
        ("aero_link.cur", "clicky.cur"),
        ("aero_helpsel.cur", "whatsthis.cur"),
        ("aero_unavail.cur", "denied.cur"),
    ];
    let mut present = Vec::new();
    for (src, dst) in files {
        if stock(src).exists() {
            std::fs::copy(stock(src), dir.join(dst)).unwrap();
            present.push(dst);
        }
    }
    assert!(present.len() >= 3, "need stock cursors to build the fixture");

    // Names like "clicky.cur" and "denied.cur" would never be guessed correctly,
    // so this also proves the INF is genuinely being used.
    std::fs::write(
        dir.join("install.inf"),
        b"[Version]\r\nsignature=\"$CHICAGO$\"\r\n\r\n[Strings]\r\n\
          SCHEME_NAME = \"Downloaded\"\r\n\
          pointer     = \"pointer.cur\"\r\n\
          busy        = \"wait.ani\"\r\n\
          link        = \"clicky.cur\"\r\n\
          help        = \"whatsthis.cur\"\r\n\
          unavailiable= \"denied.cur\"\r\n",
    )
    .unwrap();

    let imported = import::from_folder(&dir, "Downloaded").unwrap();
    assert_eq!(imported.source, import::MapSource::Inf, "install.inf should have been used");
    assert!(
        imported.mappings.iter().all(|m| m.source == import::MapSource::Inf),
        "every mapping should come from the INF, not name guessing"
    );

    let by_role = |r: &str| {
        imported
            .mappings
            .iter()
            .find(|m| m.role == r)
            .map(|m| m.file.file_name().unwrap().to_string_lossy().to_string())
    };
    assert_eq!(by_role("Arrow").as_deref(), Some("pointer.cur"));
    assert_eq!(by_role("Hand").as_deref(), Some("clicky.cur"));
    assert_eq!(by_role("Wait").as_deref(), Some("wait.ani"));

    // Materialise into a self-contained pack, then load it back from disk.
    let dest = scratch("downloaded-out");
    let _ = std::fs::remove_dir_all(&dest);
    imported.materialize(&dest).unwrap();
    assert!(dest.join("pack.json").is_file());

    let reloaded = Pack::load(&dest).unwrap();
    let built = build::build(&reloaded, &dest).unwrap();
    assert_eq!(built.cursors.len(), imported.mappings.len());

    // And Windows must accept every file the pack produces.
    let out = scratch("downloaded-built");
    let _ = std::fs::remove_dir_all(&out);
    for (role, path) in built.write_to(&out).unwrap() {
        winapply_probe(&role, &path);
    }
}

/// Kept separate so the assertion message names the role that failed.
fn winapply_probe(role: &str, path: &Path) {
    // The winapply crate is not a dependency of cursorpack's tests, so validate
    // structurally instead: parse it back the way Windows would read it.
    let data = std::fs::read(path).unwrap();
    if path.extension().and_then(|e| e.to_str()) == Some("ani") {
        let a = ani::parse(&data).unwrap_or_else(|e| panic!("{role}: {e}"));
        assert!(a.header.frames > 0, "{role} has no frames");
        for (n, f) in a.frames.iter().enumerate() {
            ico::read_dir(f).unwrap_or_else(|e| panic!("{role} frame {n}: {e}"));
        }
    } else {
        let d = ico::decode(&data).unwrap_or_else(|e| panic!("{role}: {e}"));
        assert!(!d.is_empty(), "{role} decoded to nothing");
    }
}

/// Gallery thumbnails must work for packs made of ready-made cursor files.
/// Missing this made imported packs render as "cannot build".
#[test]
fn thumbnails_work_for_ready_made_packs() {
    let dir = scratch("thumbs");
    let pack = make_pack(
        &dir,
        &[("Arrow", "aero_arrow.cur"), ("Wait", "aero_busy.ani")],
        None,
    );

    let thumbs = build::thumbnails(&pack, &dir, 64).expect("thumbnails should build");
    assert_eq!(thumbs.len(), 2, "every role needs a thumbnail");

    for (role, size, rgba) in &thumbs {
        assert_eq!(
            rgba.len(),
            (*size as usize) * (*size as usize) * 4,
            "{role}: buffer must match its reported size"
        );
        let visible = rgba.chunks_exact(4).filter(|p| p[3] > 16).count();
        assert!(visible > 0, "{role} thumbnail is blank");
    }
}

/// Legacy cursors are frequently 1/4/8bpp paletted. We deliberately do not decode
/// those pixel formats, but Windows loads them fine — so copying one through must
/// still work. Insisting on a full decode would reject a perfectly good file.
#[test]
fn paletted_legacy_cursors_still_pass_through() {
    let dir = scratch("paletted");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    // Hand-build a 32x32 8bpp cursor: ICONDIR + BITMAPINFOHEADER + 256-colour
    // palette + 8bpp pixels + AND mask.
    let (w, h) = (32u32, 32u32);
    let palette = 256 * 4;
    let xor = (w * h) as usize; // 1 byte per pixel
    let and = (((w + 31) / 32) * 4 * h) as usize;
    let img_len = 40 + palette + xor + and;

    let mut img = Vec::with_capacity(img_len);
    img.extend_from_slice(&40u32.to_le_bytes());
    img.extend_from_slice(&(w as i32).to_le_bytes());
    img.extend_from_slice(&((h as i32) * 2).to_le_bytes());
    img.extend_from_slice(&1u16.to_le_bytes()); // planes
    img.extend_from_slice(&8u16.to_le_bytes()); // 8 bits per pixel
    img.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    img.extend_from_slice(&0u32.to_le_bytes()); // size image
    img.extend_from_slice(&0i32.to_le_bytes());
    img.extend_from_slice(&0i32.to_le_bytes());
    img.extend_from_slice(&256u32.to_le_bytes()); // colours used
    img.extend_from_slice(&0u32.to_le_bytes());
    for i in 0..256u32 {
        // BGRA palette entries
        img.extend_from_slice(&[(i & 0xff) as u8, 0x40, 0x80, 0]);
    }
    img.extend(std::iter::repeat(7u8).take(xor)); // all palette index 7
    img.extend(std::iter::repeat(0u8).take(and)); // fully opaque mask
    assert_eq!(img.len(), img_len);

    let mut cur = Vec::new();
    cur.extend_from_slice(&0u16.to_le_bytes());
    cur.extend_from_slice(&2u16.to_le_bytes()); // cursor
    cur.extend_from_slice(&1u16.to_le_bytes()); // one image
    cur.push(w as u8);
    cur.push(h as u8);
    cur.push(0);
    cur.push(0);
    cur.extend_from_slice(&4u16.to_le_bytes()); // hotspot x
    cur.extend_from_slice(&3u16.to_le_bytes()); // hotspot y
    cur.extend_from_slice(&(img_len as u32).to_le_bytes());
    cur.extend_from_slice(&22u32.to_le_bytes()); // offset
    cur.extend_from_slice(&img);

    std::fs::write(dir.join("art/legacy.cur"), &cur).unwrap();
    std::fs::write(
        dir.join("pack.json"),
        r#"{ "name": "Legacy", "roles": { "Arrow": { "source": "art/legacy.cur" } } }"#,
    )
    .unwrap();

    // Our full decoder refuses 8bpp, which is exactly why the build must not use it.
    assert!(ico::decode(&cur).is_err(), "8bpp decode is intentionally unsupported");

    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).expect("an 8bpp cursor must still build");
    assert_eq!(built.cursors.len(), 1);
    assert_eq!(built.cursors[0].data, cur, "copied through byte for byte");
    assert_eq!(built.cursors[0].sizes, vec![32], "size read from the directory");
}

// ---------------------------------------------------------------- upscale

/// Build a standalone single-size `.cur` out of one image cropped from a stock
/// multi-size file, mimicking the low-resolution downloads (32/48px only) that
/// prompted the `upscale` option.
fn single_size_cur(source: &Path, size: u32) -> Vec<u8> {
    let images = ico::decode(&std::fs::read(source).unwrap()).unwrap();
    let img = images
        .into_iter()
        .find(|i| i.size == size)
        .unwrap_or_else(|| panic!("{} has no {size}px image", source.display()));
    ico::encode(&[img], ico::TYPE_CURSOR).unwrap()
}

/// The scenario the option exists for: a ready-made source that only has small
/// images gets the pack's larger sizes filled in, rather than staying stuck at
/// its native resolution for Windows to stretch on the fly.
#[test]
fn upscale_fills_in_larger_sizes_for_a_low_res_static_source() {
    let dir = scratch("upscale-static");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    std::fs::write(dir.join("art/small.cur"), single_size_cur(&stock("aero_arrow.cur"), 32)).unwrap();
    std::fs::write(
        dir.join("pack.json"),
        "{ \"name\": \"Upscale\", \"roles\": { \"Arrow\": { \"source\": \"art/small.cur\", \"upscale\": true } } }",
    )
    .unwrap();

    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();
    let arrow = &built.cursors[0];

    assert_eq!(
        arrow.sizes,
        cursorpack::DEFAULT_SIZES.to_vec(),
        "upscale should fill in every size the pack asks for"
    );

    let images = ico::decode(&arrow.data).unwrap();
    assert_eq!(images.len(), cursorpack::DEFAULT_SIZES.len());
    for img in &images {
        assert_eq!(img.rgba.len(), (img.size * img.size * 4) as usize);
        // Every synthesized size must still have real content, not a blank canvas.
        let opaque = img.rgba.chunks_exact(4).filter(|p| p[3] > 16).count();
        assert!(opaque > 0, "{}px is blank after upscaling", img.size);
    }

    assert!(
        built.warnings.iter().any(|w| matches!(
            w,
            cursorpack::raster::Warning::ReadyMadeUpscaled { native: 32, .. }
        )),
        "should report what was synthesized, got {:?}",
        built.warnings
    );
    assert!(
        !built.warnings.iter().any(|w| matches!(
            w,
            cursorpack::raster::Warning::LowResolutionSource { .. }
        )),
        "should not also claim it is stuck at low resolution once upscale fixed it"
    );
}

/// The vector-trace alternative to plain resampling: same scenario, but the
/// larger sizes come from tracing the source into vector shapes and
/// rendering each size from that, rather than a Lanczos resample.
#[test]
fn upscale_vector_traces_larger_sizes_for_a_low_res_static_source() {
    let dir = scratch("upscale-vector-static");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    std::fs::write(dir.join("art/small.cur"), single_size_cur(&stock("aero_arrow.cur"), 32)).unwrap();
    std::fs::write(
        dir.join("pack.json"),
        "{ \"name\": \"UpscaleVector\", \"roles\": { \"Arrow\": { \"source\": \"art/small.cur\", \"upscale\": \"vector\" } } }",
    )
    .unwrap();

    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();
    let arrow = &built.cursors[0];

    assert_eq!(
        arrow.sizes,
        cursorpack::DEFAULT_SIZES.to_vec(),
        "vector upscale should fill in every size the pack asks for"
    );

    let images = ico::decode(&arrow.data).unwrap();
    assert_eq!(images.len(), cursorpack::DEFAULT_SIZES.len());
    for img in &images {
        assert_eq!(img.rgba.len(), (img.size * img.size * 4) as usize);
        let opaque = img.rgba.chunks_exact(4).filter(|p| p[3] > 16).count();
        assert!(opaque > 0, "{}px is blank after vector upscaling", img.size);
    }

    assert!(
        built.warnings.iter().any(|w| matches!(
            w,
            cursorpack::raster::Warning::ReadyMadeUpscaled {
                native: 32,
                mode: cursorpack::manifest::UpscaleMode::Vector,
                ..
            }
        )),
        "should report the vector mode that was used, got {:?}",
        built.warnings
    );
}

/// Without `upscale` the old behaviour is unchanged: pass through untouched and
/// flag it as low resolution. This is what confirms the option is opt-in.
#[test]
fn without_upscale_a_low_res_source_is_still_just_flagged() {
    let dir = scratch("no-upscale-static");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    let small = single_size_cur(&stock("aero_arrow.cur"), 32);
    std::fs::write(dir.join("art/small.cur"), &small).unwrap();
    std::fs::write(
        dir.join("pack.json"),
        "{ \"name\": \"NoUpscale\", \"roles\": { \"Arrow\": { \"source\": \"art/small.cur\" } } }",
    )
    .unwrap();

    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();
    assert_eq!(built.cursors[0].data, small, "still a byte-for-byte copy");
    assert_eq!(built.cursors[0].sizes, vec![32]);
    assert!(built.warnings.iter().any(|w| matches!(
        w,
        cursorpack::raster::Warning::LowResolutionSource { largest: 32, .. }
    )));
}

/// A size that already existed must keep its own exact pixels and hotspot; only
/// sizes that did not exist are synthesized.
#[test]
fn upscale_leaves_the_original_size_untouched() {
    let dir = scratch("upscale-preserve");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    let original = single_size_cur(&stock("aero_arrow.cur"), 32);
    std::fs::write(dir.join("art/small.cur"), &original).unwrap();
    std::fs::write(
        dir.join("pack.json"),
        "{ \"name\": \"Preserve\", \"roles\": { \"Arrow\": { \"source\": \"art/small.cur\", \"upscale\": true } } }",
    )
    .unwrap();

    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();

    let orig_img = &ico::decode(&original).unwrap()[0];
    let built_32 = ico::decode(&built.cursors[0].data)
        .unwrap()
        .into_iter()
        .find(|i| i.size == 32)
        .unwrap();

    assert_eq!(built_32.rgba, orig_img.rgba, "original pixels must survive exactly");
    assert_eq!((built_32.hot_x, built_32.hot_y), (orig_img.hot_x, orig_img.hot_y));
}

/// A synthesized size's hotspot must scale proportionally from the native one,
/// the same way a drawn source's hotspot scales from `base_size`.
#[test]
fn upscale_scales_the_hotspot_for_synthesized_sizes() {
    let dir = scratch("upscale-hotspot");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    let small = single_size_cur(&stock("aero_arrow.cur"), 32);
    let native_hot = ico::decode(&small).unwrap()[0].clone();
    let (nx, ny) = (native_hot.hot_x, native_hot.hot_y);

    std::fs::write(dir.join("art/small.cur"), &small).unwrap();
    std::fs::write(
        dir.join("pack.json"),
        "{ \"name\": \"HotspotScale\", \"roles\": { \"Arrow\": { \"source\": \"art/small.cur\", \"upscale\": true } } }",
    )
    .unwrap();

    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();
    for img in ico::decode(&built.cursors[0].data).unwrap() {
        let expect_x = ((nx as f64 * img.size as f64 / 32.0).round() as u32).min(img.size - 1);
        let expect_y = ((ny as f64 * img.size as f64 / 32.0).round() as u32).min(img.size - 1);
        assert_eq!(
            (img.hot_x as u32, img.hot_y as u32),
            (expect_x, expect_y),
            "hotspot at {}px",
            img.size
        );
    }
}

/// An explicit hotspot override still wins over upscaling's own proportional one.
#[test]
fn upscale_still_honours_an_explicit_hotspot_override() {
    let dir = scratch("upscale-override");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    std::fs::write(dir.join("art/small.cur"), single_size_cur(&stock("aero_arrow.cur"), 32)).unwrap();
    std::fs::write(
        dir.join("pack.json"),
        "{ \"name\": \"OverrideUpscale\", \"roles\": { \"Arrow\": { \"source\": \"art/small.cur\", \"upscale\": true, \"hotspot\": [4, 3] } } }",
    )
    .unwrap();

    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();
    for img in ico::decode(&built.cursors[0].data).unwrap() {
        let expect = pack.scaled_hotspot(&pack.roles["Arrow"], img.size);
        assert_eq!((img.hot_x, img.hot_y), expect, "at {}px", img.size);
    }
}

/// Upscaling an animated ready-made source must still respect the Windows
/// per-frame byte ceiling - the same budget drawn animations are trimmed to.
#[test]
fn upscale_on_animated_ready_made_respects_the_frame_budget() {
    let dir = scratch("upscale-animated");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("art")).unwrap();

    // aero_busy.ani already carries 32/48/64px in every frame; asking to
    // upscale to the full DEFAULT_SIZES range must be capped by the budget
    // exactly like a drawn animation is.
    std::fs::copy(stock("aero_busy.ani"), dir.join("art/busy.ani")).unwrap();
    std::fs::write(
        dir.join("pack.json"),
        "{ \"name\": \"UpscaleAnimated\", \"roles\": { \"Wait\": { \"source\": \"art/busy.ani\", \"upscale\": true } } }",
    )
    .unwrap();

    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();
    let wait = &built.cursors[0];

    let (expected_kept, expected_dropped) =
        cursorpack::ani::fit_sizes(&cursorpack::DEFAULT_SIZES.to_vec());
    assert_eq!(wait.sizes, expected_kept);
    assert!(!expected_dropped.is_empty(), "the test should exercise an actual cap");

    assert!(
        cursorpack::ico::frame_bytes(&wait.sizes) <= cursorpack::ani::MAX_FRAME_BYTES,
        "must not exceed the per-frame ceiling"
    );
    assert!(built.warnings.iter().any(|w| matches!(
        w,
        cursorpack::raster::Warning::AnimationSizesDropped { .. }
    )));

    let decoded = ani::decode(&wait.data).unwrap();
    assert_eq!(decoded.frames.len(), 18, "frame count must be unchanged");
}
