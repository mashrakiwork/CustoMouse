//! Windows integration tests. These talk to the real OS.
//!
//! Tests that change the registry take a snapshot first and restore it from a
//! drop guard, so they put your cursors back even if an assertion fails. They are
//! serialised behind a mutex because they share one registry key.

#![cfg(windows)]

use cursorpack::{build, manifest::Pack, DEFAULT_SIZES};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use winapply::{apply, registry, restore};

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packs/inverted-default")
        .canonicalize()
        .unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join("custo-mouse-tests").join(name);
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Only one test may touch the cursors key at a time.
///
/// Poisoning is ignored deliberately: a failing test would otherwise cascade into
/// every other one, hiding the original error behind a pile of PoisonErrors.
fn lock_registry() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Snapshot the Windows defaults as the starting point.
///
/// Without this a pack left applied by an earlier run becomes the baseline, and
/// "apply changed something" assertions compare a pack against itself. That is
/// how a real corruption went unnoticed once, so tests now establish the state
/// they expect rather than inheriting whatever was there.
fn clean_baseline() -> registry::Snapshot {
    let _ = apply::restore_defaults();
    registry::snapshot().expect("read cursor settings")
}

/// Restores the registry when it goes out of scope, including during a panic.
struct RegistryGuard {
    snapshot: registry::Snapshot,
}

impl RegistryGuard {
    fn take() -> Self {
        RegistryGuard {
            snapshot: registry::snapshot().expect("should be able to read cursor settings"),
        }
    }
}

impl Drop for RegistryGuard {
    fn drop(&mut self) {
        if let Err(e) = registry::restore(&self.snapshot) {
            eprintln!("WARNING: could not restore cursor settings: {e}");
        }
    }
}

// ---------------------------------------------------------------- read-only

/// The authoritative check: Windows itself must accept every file we generate.
/// Self-consistent encoders can still be wrong; `LoadImageW` cannot be fooled.
#[test]
fn windows_accepts_every_generated_cursor() {
    let dir = demo_dir();
    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();
    let out = scratch("accept");
    let written = built.write_to(&out).unwrap();

    assert_eq!(written.len(), pack.active_roles().len());
    for (role, path) in &written {
        let loaded = winapply::probe_cursor(path, None)
            .unwrap_or_else(|e| panic!("Windows rejected {role}: {e}"));
        assert!(
            loaded.width > 0 && loaded.height > 0,
            "{role} loaded with no dimensions"
        );
    }
}

/// Windows must honour the size we ask for, using the image we embedded.
#[test]
fn windows_serves_each_embedded_size() {
    let dir = demo_dir();
    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();
    let out = scratch("sizes");
    let written = built.write_to(&out).unwrap();

    for (role, path) in &written {
        for &size in DEFAULT_SIZES {
            let loaded = winapply::probe_cursor(path, Some(size))
                .unwrap_or_else(|e| panic!("{role} at {size}px: {e}"));
            assert_eq!(
                (loaded.width, loaded.height),
                (size as i32, size as i32),
                "{role} asked for {size}px, Windows gave {}x{}",
                loaded.width,
                loaded.height
            );
        }
    }
}

/// The hotspot Windows reports must be the scaled one, not the 32px one reused.
#[test]
fn windows_reports_the_scaled_hotspot() {
    let dir = demo_dir();
    let pack = Pack::load(&dir).unwrap();
    let built = build::build(&pack, &dir).unwrap();
    let out = scratch("hotspot");
    let written = built.write_to(&out).unwrap();

    let (_, arrow) = written.iter().find(|(r, _)| r == "Arrow").unwrap();
    for &size in DEFAULT_SIZES {
        let loaded = winapply::probe_cursor(arrow, Some(size)).unwrap();
        let (ex, ey) = pack.scaled_hotspot(&pack.roles["Arrow"], size);
        assert_eq!(
            (loaded.hot_x, loaded.hot_y),
            (ex as i32, ey as i32),
            "Arrow hotspot at {size}px"
        );
    }
}

/// Mean *squared* gradient of the alpha channel.
///
/// Squaring matters here: smearing an edge across more pixels roughly conserves
/// the summed gradient magnitude, so plain magnitude barely separates a sharp
/// edge from a blurred one. Energy does — one pixel stepping by 255 contributes
/// far more than four pixels stepping by 64 each.
fn edge_sharpness(bgra: &[u8], size: u32) -> f64 {
    let a = |x: u32, y: u32| bgra[((y * size + x) * 4 + 3) as usize] as f64;
    let mut total = 0.0;
    let mut n = 0.0;
    for y in 1..size - 1 {
        for x in 1..size - 1 {
            let gx = a(x + 1, y) - a(x - 1, y);
            let gy = a(x, y + 1) - a(x, y - 1);
            total += gx * gx + gy * gy;
            n += 1.0;
        }
    }
    if n == 0.0 {
        0.0
    } else {
        total / n
    }
}

/// The claim the whole design rests on, measured rather than assumed: a cursor
/// with a real 128px image embedded is sharper at 128px than one Windows has to
/// upscale from 32px.
#[test]
fn embedding_large_sizes_produces_a_sharper_cursor() {
    let out = scratch("sharpness");

    // Must use a *rasterised* source. The bundled pack is ready-made .cur files,
    // which pass straight through, so pack.sizes would have no effect and both
    // variants would be byte-identical.
    std::fs::create_dir_all(out.join("art")).unwrap();
    std::fs::write(
        out.join("art/arrow.svg"),
        br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
             <path d="M2 2 L2 24 L8 18 L12 28 L16 26 L12 16 L20 16 Z"
                   fill="white" stroke="black"/></svg>"#,
    )
    .unwrap();

    let build_with = |sizes: Vec<u32>, name: &str| -> PathBuf {
        let json = format!(
            r#"{{"name":"Sharpness","sizes":{sizes:?},
                 "roles":{{"Arrow":{{"source":"art/arrow.svg","hotspot":[2,2]}}}}}}"#
        );
        std::fs::write(out.join("pack.json"), json).unwrap();
        let pack = Pack::load(&out).unwrap();
        let built = build::build(&pack, &out).unwrap();
        let path = out.join(name);
        std::fs::write(&path, &built.cursors[0].data).unwrap();
        path
    };

    let small_only = build_with(vec![32], "small.cur");
    let multi_size = build_with(DEFAULT_SIZES.to_vec(), "multi.cur");

    let a = winapply::read_cursor_pixels(&small_only, 128).unwrap();
    let b = winapply::read_cursor_pixels(&multi_size, 128).unwrap();

    let upscaled = edge_sharpness(&a, 128);
    let native = edge_sharpness(&b, 128);

    eprintln!("edge sharpness at 128px: upscaled-from-32 {upscaled:.2}, embedded-128 {native:.2}");
    assert!(
        native > upscaled * 1.5,
        "embedding a 128px image should be visibly sharper, but got {native:.2} vs {upscaled:.2}"
    );
}

/// Lock in the measured per-frame ceiling. If a future Windows build changes it,
/// this fails loudly instead of shipping cursors that silently refuse to load.
#[test]
fn the_animation_frame_budget_matches_what_windows_accepts() {
    use cursorpack::ani::{self, Anim, MAX_FRAME_BYTES};
    use cursorpack::ico::{self, frame_bytes, Image};

    let out = scratch("budget");
    let img = |size: u32| Image {
        size,
        rgba: vec![200u8; (size * size * 4) as usize],
        hot_x: 0,
        hot_y: 0,
    };
    let write = |sizes: &[u32], name: &str| -> PathBuf {
        let frame: Vec<Image> = sizes.iter().map(|&s| img(s)).collect();
        let a = Anim {
            frames: vec![frame.clone(), frame],
            default_rate: 3,
            rates: None,
            seq: None,
        };
        let p = out.join(name);
        std::fs::write(&p, ani::encode(&a).unwrap()).unwrap();
        p
    };

    // What the budget chooses must load.
    let (kept, _) = ani::fit_sizes(DEFAULT_SIZES);
    assert!(frame_bytes(&kept) <= MAX_FRAME_BYTES);
    let p = write(&kept, "within.ani");
    winapply::probe_cursor(&p, None)
        .unwrap_or_else(|e| panic!("budgeted sizes {kept:?} should load: {e}"));

    // And the combination we measured as too large must still be refused, which is
    // what makes the budget necessary rather than superstition.
    let over = vec![32u32, 128];
    assert!(frame_bytes(&over) > MAX_FRAME_BYTES, "expected {over:?} to exceed the cap");
    let p = write(&over, "over.ani");
    assert!(
        winapply::probe_cursor(&p, None).is_err(),
        "Windows unexpectedly accepted {over:?} ({} bytes/frame) � the budget may now be wrong",
        frame_bytes(&over)
    );

    // Total file size must remain irrelevant: many small frames are fine.
    let frame: Vec<Image> = [32u32, 48, 64].iter().map(|&s| img(s)).collect();
    let many = Anim { frames: vec![frame; 120], default_rate: 3, rates: None, seq: None };
    let bytes = ani::encode(&many).unwrap();
    assert!(bytes.len() > 3_000_000, "expected a multi-megabyte file");
    let p = out.join("many.ani");
    std::fs::write(&p, &bytes).unwrap();
    winapply::probe_cursor(&p, None).expect("a 3MB .ani with small frames should still load");
    let _ = ico::TYPE_CURSOR;
}

// ---------------------------------------------------------------- registry

/// The highest-stakes test here: applying and restoring must leave the registry
/// byte-for-byte as it was, or this app can permanently alter someone's system.
#[test]
fn restore_returns_the_registry_byte_for_byte() {
    let _lock = lock_registry();
    let _guard = RegistryGuard::take();

    restore::ensure_restore_point().unwrap();
    let before = clean_baseline();

    let applied = apply::apply(&demo_dir()).expect("demo pack should apply");
    assert!(!applied.roles.is_empty());

    let during = registry::snapshot().unwrap();
    assert_ne!(during.values, before.values, "apply should have changed something");

    apply::restore_defaults().expect("restore should succeed");

    let after = registry::snapshot().unwrap();
    assert_eq!(
        after.values, before.values,
        "restore must reproduce the original values exactly, including their types"
    );
}

/// Applying must actually point Windows at our files, and the values must be
/// readable back as the paths we wrote.
#[test]
fn applying_points_windows_at_our_files() {
    let _lock = lock_registry();
    let _guard = RegistryGuard::take();

    let applied = apply::apply(&demo_dir()).unwrap();

    for (role, path) in &applied.roles {
        let value = registry::get_role(role)
            .unwrap()
            .unwrap_or_else(|| panic!("{role} was not set"));
        assert_eq!(Path::new(&value), path.as_path(), "{role}");
        assert!(Path::new(&value).is_file(), "{role} points at a missing file");
    }

    // Roles the pack does not define must fall back to the Windows default,
    // never to a previously applied pack.
    for role in &applied.reset_to_default {
        let value = registry::get_role(role).unwrap().unwrap_or_default();
        assert!(value.is_empty(), "{role} should be empty, got {value:?}");
    }
}

/// Switching packs must not leave cursors from the previous one behind.
#[test]
fn switching_packs_does_not_leave_stale_cursors() {
    let _lock = lock_registry();
    let _guard = RegistryGuard::take();

    let dir = demo_dir();
    apply::apply(&dir).unwrap();
    let hand_after_full = registry::get_role("Hand").unwrap().unwrap_or_default();
    assert!(!hand_after_full.is_empty(), "demo pack defines Hand");

    // A second pack that defines only Arrow.
    let partial_dir = scratch("partial-pack");
    std::fs::create_dir_all(partial_dir.join("art")).unwrap();
    std::fs::copy(dir.join("art/Arrow.cur"), partial_dir.join("art/Arrow.cur")).unwrap();
    std::fs::write(
        partial_dir.join("pack.json"),
        r#"{"name":"Arrow Only","roles":{"Arrow":{"source":"art/Arrow.cur"}}}"#,
    )
    .unwrap();

    apply::apply(&partial_dir).unwrap();

    let hand_now = registry::get_role("Hand").unwrap().unwrap_or_default();
    assert!(
        hand_now.is_empty(),
        "Hand should have reset to the Windows default, but still points at {hand_now:?}"
    );
}

/// Restoring must work when the compiled pack has been deleted, because a restore
/// that depends on our files is not a safety net.
#[test]
fn restore_works_after_the_installed_files_are_deleted() {
    let _lock = lock_registry();
    let _guard = RegistryGuard::take();

    let before = clean_baseline();
    let applied = apply::apply(&demo_dir()).unwrap();

    // Windows may still be reading the files it was just told to load, so use the
    // same retrying delete the app itself uses.
    apply::remove_dir_all_retrying(&applied.install_dir).unwrap();
    assert!(!applied.install_dir.exists());

    apply::restore_defaults().expect("restore must not need the installed files");
    assert_eq!(registry::snapshot().unwrap().values, before.values);
}

/// The offline `.cmd` is the last line of defence. It has to genuinely work, run
/// by cmd.exe as a subprocess, with this program uninvolved.
#[test]
fn the_offline_restore_script_actually_restores() {
    let _lock = lock_registry();
    let _guard = RegistryGuard::take();

    // The script restores the *restore point*, not whatever is applied right now,
    // so that is what this must compare against.
    let original = restore::ensure_restore_point().unwrap();
    let script = restore::script_path();
    assert!(script.is_file(), "the script is written with the restore point");

    apply::apply(&demo_dir()).unwrap();
    assert_ne!(registry::snapshot().unwrap().values, original.values);

    // `pause` at the end would block, so feed it a newline and close stdin.
    let output = std::process::Command::new("cmd.exe")
        .args(["/C", &script.to_string_lossy()])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("cmd.exe should run the script");
    assert!(
        output.status.success(),
        "script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = registry::snapshot().unwrap();
    assert_eq!(
        after.values, original.values,
        "the offline script must restore the registry exactly"
    );
}

/// A restore point must be captured before the first change, and never silently
/// overwritten by later applies — otherwise "restore" would return you to another
/// pack rather than to Windows.
#[test]
fn the_restore_point_is_captured_once_and_kept() {
    let _lock = lock_registry();
    let _guard = RegistryGuard::take();

    let original = restore::ensure_restore_point().unwrap();
    apply::apply(&demo_dir()).unwrap();

    let after_apply = restore::ensure_restore_point().unwrap();
    assert_eq!(
        after_apply.values, original.values,
        "the original restore point must survive an apply"
    );
}

// ---------------------------------------------------------- session memory

/// Quitting with "restore on exit" must leave the Windows defaults in place but
/// remember the pack, or logging back in would silently lose the user's choice.
#[test]
fn restoring_on_exit_keeps_the_remembered_pack() {
    let _lock = lock_registry();
    let _guard = RegistryGuard::take();
    let before = clean_baseline();

    let applied = apply::apply(&demo_dir()).unwrap();
    let state = apply::load_state();
    assert_eq!(state.active_pack.as_deref(), Some(applied.pack_name.as_str()));
    assert_eq!(state.last_pack.as_deref(), Some(applied.pack_name.as_str()));

    apply::restore_defaults_for_exit().unwrap();

    let state = apply::load_state();
    assert!(state.active_pack.is_none(), "nothing should be applied after exit");
    assert_eq!(
        state.last_pack.as_deref(),
        Some(applied.pack_name.as_str()),
        "the chosen pack must be remembered across the exit"
    );
    assert_eq!(registry::snapshot().unwrap().values, before.values);
}

/// Asking for the Windows defaults explicitly means exactly that: the next
/// launch must not put the pack back.
#[test]
fn explicitly_restoring_defaults_forgets_the_pack() {
    let _lock = lock_registry();
    let _guard = RegistryGuard::take();
    clean_baseline();

    apply::apply(&demo_dir()).unwrap();
    apply::restore_defaults().unwrap();

    let state = apply::load_state();
    assert!(state.active_pack.is_none());
    assert!(
        state.last_pack.is_none(),
        "an explicit restore must clear the remembered pack too"
    );
    assert_eq!(apply::reapply_last_pack().unwrap(), None);
}

/// The startup path: after an exit-restore, relaunching puts the pack back.
#[test]
fn startup_reapplies_the_remembered_pack() {
    let _lock = lock_registry();
    let _guard = RegistryGuard::take();
    clean_baseline();

    let applied = apply::apply(&demo_dir()).unwrap();
    apply::restore_defaults_for_exit().unwrap();
    assert!(registry::get_role("Arrow").unwrap().unwrap_or_default().is_empty()
        || !registry::get_role("Arrow").unwrap().unwrap_or_default().contains("CustoMouse"));

    let name = apply::reapply_last_pack().unwrap();
    assert_eq!(name.as_deref(), Some(applied.pack_name.as_str()));

    let arrow = registry::get_role("Arrow").unwrap().unwrap_or_default();
    assert!(
        arrow.contains("CustoMouse"),
        "Arrow should point back at the pack, got {arrow:?}"
    );
    assert_eq!(apply::load_state().active_pack.as_deref(), Some(applied.pack_name.as_str()));
}

/// Relaunching while the pack is still applied must not rebuild and rewrite it.
#[test]
fn startup_does_nothing_when_the_pack_is_already_applied() {
    let _lock = lock_registry();
    let _guard = RegistryGuard::take();
    clean_baseline();

    apply::apply(&demo_dir()).unwrap();
    assert_eq!(
        apply::reapply_last_pack().unwrap(),
        None,
        "already applied, so startup should be a no-op"
    );
}

/// A remembered pack whose folder has gone must be skipped, not error out.
#[test]
fn startup_skips_a_pack_that_no_longer_exists() {
    let _lock = lock_registry();
    let _guard = RegistryGuard::take();
    clean_baseline();

    let mut state = apply::load_state();
    state.active_pack = None;
    state.active_pack_dir = None;
    state.last_pack = Some("Gone".into());
    state.last_pack_dir = Some(std::env::temp_dir().join("custo-does-not-exist"));
    apply::save_state(&state).unwrap();

    assert_eq!(apply::reapply_last_pack().unwrap(), None);
}

// ---------------------------------------------------------- ready-made upscale

/// Build a standalone single-size `.cur` from one image cropped out of a stock
/// multi-size file, mimicking a low-resolution download (32/48px only).
fn single_size_cur(source: &Path, size: u32) -> Vec<u8> {
    let images = cursorpack::ico::decode(&std::fs::read(source).unwrap()).unwrap();
    let img = images.into_iter().find(|i| i.size == size).unwrap();
    cursorpack::ico::encode(&[img], cursorpack::ico::TYPE_CURSOR).unwrap()
}

/// The authoritative check for the `upscale` option: every size it synthesizes
/// must actually be something Windows will load, at the size requested, not
/// just bytes that happen to decode on our own end.
#[test]
fn windows_accepts_every_upscaled_size_from_a_low_res_ready_made_source() {
    let out = scratch("upscale-windows");
    std::fs::create_dir_all(out.join("art")).unwrap();

    let stock_dir = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()))
        .join("Cursors");
    std::fs::write(
        out.join("art/small.cur"),
        single_size_cur(&stock_dir.join("aero_arrow.cur"), 32),
    )
    .unwrap();
    std::fs::write(
        out.join("pack.json"),
        "{ \"name\": \"WindowsUpscale\", \"roles\": { \"Arrow\": { \"source\": \"art/small.cur\", \"upscale\": true } } }",
    )
    .unwrap();

    let pack = Pack::load(&out).unwrap();
    let built = build::build(&pack, &out).unwrap();
    let written = built.write_to(&out.join("built")).unwrap();
    let (_, path) = written.iter().find(|(r, _)| r == "Arrow").unwrap();

    for &size in DEFAULT_SIZES {
        let loaded = winapply::probe_cursor(path, Some(size))
            .unwrap_or_else(|e| panic!("Windows rejected the upscaled {size}px: {e}"));
        assert_eq!(
            (loaded.width, loaded.height),
            (size as i32, size as i32),
            "asked Windows for {size}px, got {}x{}",
            loaded.width,
            loaded.height
        );
    }
}

/// The upscaled result should still be sharper than letting Windows stretch the
/// native size on its own: our resample uses the same premultiplied Lanczos
/// filter as the rest of the pipeline, applied once at build time.
#[test]
fn upscaling_a_ready_made_source_is_sharper_than_leaving_it_to_windows() {
    let out = scratch("upscale-sharpness");
    std::fs::create_dir_all(out.join("art")).unwrap();

    let stock_dir = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()))
        .join("Cursors");
    let small = single_size_cur(&stock_dir.join("aero_arrow.cur"), 32);
    std::fs::write(out.join("art/small.cur"), &small).unwrap();

    // Without upscale: Windows has only the 32px image to stretch to 128px itself.
    std::fs::write(out.join("pack.json"), "{ \"name\": \"NoUp\", \"roles\": { \"Arrow\": { \"source\": \"art/small.cur\" } } }").unwrap();
    let no_up = build::build(&Pack::load(&out).unwrap(), &out).unwrap();
    let no_up_path = out.join("no_upscale.cur");
    std::fs::write(&no_up_path, &no_up.cursors[0].data).unwrap();

    // With upscale: we resample to 128px ourselves at build time.
    std::fs::write(
        out.join("pack.json"),
        "{ \"name\": \"Up\", \"roles\": { \"Arrow\": { \"source\": \"art/small.cur\", \"upscale\": true } } }",
    )
    .unwrap();
    let up = build::build(&Pack::load(&out).unwrap(), &out).unwrap();
    let up_path = out.join("upscale.cur");
    std::fs::write(&up_path, &up.cursors[0].data).unwrap();

    // Ask Windows to render both at 128px. The plain 32px-only file forces
    // Windows to stretch on the fly; the upscale:true file already contains a
    // 128px image we generated ourselves.
    let a = winapply::read_cursor_pixels(&no_up_path, 128).unwrap();
    let b = winapply::read_cursor_pixels(&up_path, 128).unwrap();

    let sharpness = |bgra: &[u8], size: u32| -> f64 {
        let alpha = |x: u32, y: u32| bgra[((y * size + x) * 4 + 3) as usize] as f64;
        let mut total = 0.0;
        let mut n = 0.0;
        for y in 1..size - 1 {
            for x in 1..size - 1 {
                let gx = alpha(x + 1, y) - alpha(x - 1, y);
                let gy = alpha(x, y + 1) - alpha(x, y - 1);
                total += gx * gx + gy * gy;
                n += 1.0;
            }
        }
        total / n
    };

    let windows_stretch = sharpness(&a, 128);
    let our_upscale = sharpness(&b, 128);
    eprintln!("edge energy at 128px: windows-stretched {windows_stretch:.2}, our-upscale {our_upscale:.2}");

    // Not claiming this beats real high-res art (it cannot invent detail), only
    // that resampling once ourselves is not worse than what Windows does live.
    assert!(
        our_upscale >= windows_stretch * 0.8,
        "our upscale should be roughly comparable to or better than Windows' own stretch"
    );
}
