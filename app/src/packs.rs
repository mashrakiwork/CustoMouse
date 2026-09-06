//! Finding cursor packs on disk.

use cursorpack::manifest::Pack;
use std::path::{Path, PathBuf};

/// A pack we found, with its manifest already read.
#[derive(Clone)]
pub struct FoundPack {
    pub dir: PathBuf,
    pub pack: Pack,
    /// True for packs that shipped with the app, which are read-only.
    pub builtin: bool,
}

/// Where the user's own packs live. Shown in the UI so it can be opened.
pub fn user_packs_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("CustoMouse")
        .join("packs")
}

/// Directories searched for packs, in order.
fn search_dirs() -> Vec<(PathBuf, bool)> {
    let mut dirs = Vec::new();

    // Packs shipped next to the executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push((parent.join("packs"), true));
            // Running from `target/debug`, so also check the repository root.
            if let Some(root) = parent.ancestors().nth(2) {
                dirs.push((root.join("packs"), true));
            }
        }
    }
    // During development the target directory can live outside the project (for
    // example when CARGO_TARGET_DIR is set globally), so the walk above finds
    // nothing. Fall back to the source tree.
    #[cfg(debug_assertions)]
    dirs.push((
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../packs"),
        true,
    ));

    dirs.push((user_packs_dir(), false));
    dirs
}

/// Scan for packs. Unreadable ones are reported rather than silently skipped, so a
/// broken `pack.json` is visible instead of the pack just not appearing.
pub fn discover() -> (Vec<FoundPack>, Vec<String>) {
    let mut found: Vec<FoundPack> = Vec::new();
    let mut problems = Vec::new();

    for (dir, builtin) in search_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || !path.join("pack.json").is_file() {
                continue;
            }
            // A pack already found under an earlier directory wins.
            if found.iter().any(|f| same_dir(&f.dir, &path)) {
                continue;
            }
            match Pack::load(&path) {
                Ok(pack) => found.push(FoundPack {
                    dir: path,
                    pack,
                    builtin,
                }),
                Err(e) => problems.push(format!("{}: {e}", path.display())),
            }
        }
    }

    found.sort_by(|a, b| a.pack.name.to_lowercase().cmp(&b.pack.name.to_lowercase()));
    (found, problems)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Open a folder in Explorer, creating it first if needed.
pub fn open_folder(path: &Path) {
    let _ = std::fs::create_dir_all(path);
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}
