//! Golden tests against the cursors Windows itself ships.
//!
//! These are the tests that can prove our format model wrong, because the reference
//! files were produced by Microsoft rather than by us. If our parser and writer only
//! agreed with each other, the unit tests would pass with a completely wrong format.
//!
//! Skipped automatically on non-Windows and if the directory is missing.

#![cfg(windows)]

use cursorpack::{ani, ico};
use std::path::{Path, PathBuf};

fn cursor_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()))
        .join("Cursors");
    dir.is_dir().then_some(dir)
}

fn files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        })
        .collect();
    v.sort();
    v
}

/// Every stock `.ani` must parse, and the header must agree with the frames found.
#[test]
fn parses_every_stock_ani() {
    let Some(dir) = cursor_dir() else {
        eprintln!("no Cursors dir; skipping");
        return;
    };
    let files = files_with_ext(&dir, "ani");
    assert!(!files.is_empty(), "expected stock .ani files in {dir:?}");

    let mut checked = 0;
    for path in &files {
        let data = std::fs::read(path).unwrap();
        let raw = ani::parse(&data).unwrap_or_else(|e| panic!("{}: {e}", path.display()));

        assert_eq!(
            raw.header.frames as usize,
            raw.frames.len(),
            "{}: anih says {} frames, found {}",
            path.display(),
            raw.header.frames,
            raw.frames.len()
        );
        assert_eq!(raw.header.cb_size, 36, "{}", path.display());
        assert!(
            raw.header.flags & ani::AF_ICON != 0,
            "{}: stock cursors use icon-format frames",
            path.display()
        );
        assert!(raw.header.display_rate >= 1, "{}", path.display());
        if let Some(r) = &raw.rates {
            assert_eq!(r.len(), raw.header.steps as usize, "{}", path.display());
        }

        // Every frame must be a readable multi-size cursor with a sane hotspot.
        for (n, frame) in raw.frames.iter().enumerate() {
            let entries = ico::read_dir(frame)
                .unwrap_or_else(|e| panic!("{} frame {n}: {e}", path.display()));
            assert!(!entries.is_empty());
            for e in &entries {
                assert_eq!(e.width, e.height, "{} frame {n}: not square", path.display());
                assert!(
                    e.hot_x as u32 <= e.width && e.hot_y as u32 <= e.height,
                    "{} frame {n}: hotspot {:?} outside {}px",
                    path.display(),
                    (e.hot_x, e.hot_y),
                    e.width
                );
            }
        }
        checked += 1;
    }
    eprintln!("parsed {checked} stock .ani files");
}

/// The claim the whole design rests on: stock animated cursors really do carry
/// several sizes inside a single file, so we do not need one .ani per size.
#[test]
fn stock_ani_frames_carry_multiple_sizes() {
    let Some(dir) = cursor_dir() else { return };
    let path = dir.join("aero_busy.ani");
    if !path.exists() {
        eprintln!("aero_busy.ani missing; skipping");
        return;
    }

    let raw = ani::parse(&std::fs::read(&path).unwrap()).unwrap();
    let sizes: Vec<u32> = ico::read_dir(&raw.frames[0])
        .unwrap()
        .iter()
        .map(|e| e.width)
        .collect();

    assert!(
        sizes.len() > 1,
        "aero_busy.ani frame 0 had only {sizes:?} — the multi-size premise is wrong"
    );
    assert!(sizes.contains(&32) && sizes.contains(&48) && sizes.contains(&64), "{sizes:?}");

    // Hotspots must be scaled per size, not copied.
    for e in ico::read_dir(&raw.frames[0]).unwrap() {
        assert_eq!(
            (e.hot_x as u32, e.hot_y as u32),
            (e.width / 2, e.height / 2),
            "aero_busy hotspot should sit at the centre of each size"
        );
    }
}

/// Decode a Microsoft file, re-encode it with our writer, decode again, and require
/// the pixels, sizes and hotspots to survive. This validates the writer against a
/// reference we did not produce.
#[test]
fn reencoding_stock_cursors_preserves_everything() {
    let Some(dir) = cursor_dir() else { return };

    let mut checked = 0;
    for path in files_with_ext(&dir, "ani") {
        let data = std::fs::read(&path).unwrap();

        // Some stock files use formats we intentionally do not write (paletted,
        // PNG payloads). Skip those rather than failing; the parse test above
        // already covers their structure.
        let Ok(original) = ani::decode(&data) else {
            continue;
        };

        let reencoded = ani::encode(&original)
            .unwrap_or_else(|e| panic!("re-encoding {}: {e}", path.display()));
        let back = ani::decode(&reencoded)
            .unwrap_or_else(|e| panic!("re-decoding {}: {e}", path.display()));

        assert_eq!(back.frames.len(), original.frames.len(), "{}", path.display());
        assert_eq!(back.default_rate, original.default_rate, "{}", path.display());
        assert_eq!(back.sizes(), original.sizes(), "{}", path.display());

        for (n, (o, g)) in original.frames.iter().zip(&back.frames).enumerate() {
            for oi in o {
                let gi = g
                    .iter()
                    .find(|g| g.size == oi.size)
                    .unwrap_or_else(|| panic!("{} frame {n}: lost size {}", path.display(), oi.size));
                assert_eq!(gi.rgba, oi.rgba, "{} frame {n} @{}px pixels", path.display(), oi.size);
                assert_eq!(
                    (gi.hot_x, gi.hot_y),
                    (oi.hot_x, oi.hot_y),
                    "{} frame {n} @{}px hotspot",
                    path.display(),
                    oi.size
                );
            }
        }
        checked += 1;
    }

    assert!(checked > 0, "no stock .ani files were re-encoded");
    eprintln!("re-encoded {checked} stock .ani files losslessly");
}

/// Same round trip for static `.cur` files.
#[test]
fn reencoding_stock_cur_preserves_everything() {
    let Some(dir) = cursor_dir() else { return };

    let mut checked = 0;
    for path in files_with_ext(&dir, "cur") {
        let data = std::fs::read(&path).unwrap();
        let Ok(original) = ico::decode(&data) else {
            continue;
        };

        let reencoded = ico::encode(&original, ico::TYPE_CURSOR).unwrap();
        let back = ico::decode(&reencoded).unwrap();

        assert_eq!(back.len(), original.len(), "{}", path.display());
        for oi in &original {
            let gi = back.iter().find(|g| g.size == oi.size).expect("size kept");
            assert_eq!(gi.rgba, oi.rgba, "{} @{}px", path.display(), oi.size);
            assert_eq!((gi.hot_x, gi.hot_y), (oi.hot_x, oi.hot_y), "{}", path.display());
        }
        checked += 1;
    }
    assert!(checked > 0, "no stock .cur files were re-encoded");
    eprintln!("re-encoded {checked} stock .cur files losslessly");
}

/// Our own output must be structurally indistinguishable from Microsoft's:
/// same header fields, same frame layout, same directory shape.
#[test]
fn our_output_matches_the_stock_file_shape() {
    let Some(dir) = cursor_dir() else { return };
    let path = dir.join("aero_busy.ani");
    if !path.exists() {
        return;
    }
    let stock = ani::parse(&std::fs::read(&path).unwrap()).unwrap();

    let decoded = ani::decode(&std::fs::read(&path).unwrap()).unwrap();
    let ours = ani::parse(&ani::encode(&decoded).unwrap()).unwrap();

    assert_eq!(ours.header.cb_size, stock.header.cb_size);
    assert_eq!(ours.header.frames, stock.header.frames);
    assert_eq!(ours.header.steps, stock.header.steps);
    assert_eq!(ours.header.bit_count, stock.header.bit_count);
    assert_eq!(ours.header.planes, stock.header.planes);
    assert_eq!(ours.header.display_rate, stock.header.display_rate);
    assert_eq!(ours.header.flags, stock.header.flags);

    let stock_dir = ico::read_dir(&stock.frames[0]).unwrap();
    let our_dir = ico::read_dir(&ours.frames[0]).unwrap();
    assert_eq!(our_dir.len(), stock_dir.len());
    for (o, s) in our_dir.iter().zip(&stock_dir) {
        assert_eq!((o.width, o.height), (s.width, s.height));
        assert_eq!((o.hot_x, o.hot_y), (s.hot_x, s.hot_y));
        assert_eq!(o.bytes, s.bytes, "image payload size must match exactly");
    }
}
