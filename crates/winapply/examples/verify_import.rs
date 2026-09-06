//! Import a real cursor folder, compile it, and hand every file to Windows.
use std::path::PathBuf;

fn main() {
    let src = PathBuf::from(std::env::args().nth(1).expect("usage: verify_import <folder>"));
    let import = cursorpack::import::from_folder(&src, "Verify").expect("import");
    println!("mapped {} pointer(s) from {}", import.mappings.len(), import.source.label());

    let staged = std::env::temp_dir().join("custo-verify-pack");
    let _ = std::fs::remove_dir_all(&staged);
    import.materialize(&staged).expect("materialize");

    let pack = cursorpack::manifest::Pack::load(&staged).expect("reload pack.json");
    let built = cursorpack::build::build(&pack, &staged).expect("build");

    for w in &built.warnings {
        println!("  warning: {w}");
    }

    let out = std::env::temp_dir().join("custo-verify-out");
    let _ = std::fs::remove_dir_all(&out);
    let written = built.write_to(&out).expect("write");

    let mut ok = 0;
    for (role, path) in &written {
        match winapply::probe_cursor(path, None) {
            Ok(c) => {
                ok += 1;
                let sizes = built.cursors.iter().find(|b| &b.role == role).unwrap();
                println!(
                    "  OK  {role:12} {:>3}x{:<3} sizes={:?}{}",
                    c.width, c.height, sizes.sizes,
                    if sizes.animated { format!(" animated {} frames", sizes.frames) } else { String::new() }
                );
            }
            Err(e) => println!("  REJECTED {role}: {e}"),
        }
    }
    println!("\n{ok}/{} accepted by Windows", written.len());
}
