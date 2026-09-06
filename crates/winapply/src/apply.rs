//! Installing a pack and pointing Windows at it.

use crate::registry::{self, scheme_source};
use crate::{restore, Error, Result};
use cursorpack::{build, manifest::Pack, raster::Warning};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

fn enabled_by_default() -> bool {
    true
}

/// What is currently applied, so the tray can show it across restarts.
///
/// Both behaviour switches default to **on** for a fresh install: the app is
/// meant to be there from boot, and to leave nothing behind when it is closed.
/// Once the user changes either one, `state.json` records the explicit value and
/// that choice is what is honoured from then on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    /// `None` means the Windows defaults are in place *right now*.
    #[serde(default)]
    pub active_pack: Option<String>,
    #[serde(default)]
    pub active_pack_dir: Option<PathBuf>,

    /// The pack the user last chose, remembered across restarts.
    ///
    /// Distinct from `active_pack` on purpose. With `restore_on_exit` on, quitting
    /// puts the Windows defaults back, which clears `active_pack` — but the user
    /// still wants that pack next time they log in. Only an explicit "Restore
    /// Windows Default" forgets it.
    #[serde(default)]
    pub last_pack: Option<String>,
    #[serde(default)]
    pub last_pack_dir: Option<PathBuf>,
    /// Restore the Windows defaults when the app exits.
    #[serde(default = "enabled_by_default")]
    pub restore_on_exit: bool,
    #[serde(default = "enabled_by_default")]
    pub start_with_windows: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            active_pack: None,
            active_pack_dir: None,
            last_pack: None,
            last_pack_dir: None,
            restore_on_exit: true,
            start_with_windows: true,
        }
    }
}

/// True once settings have been saved at least once, i.e. this is not a fresh install.
pub fn state_exists() -> bool {
    state_path().is_file()
}

pub fn state_path() -> PathBuf {
    restore::data_root().join("state.json")
}

pub fn load_state() -> State {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_state(state: &State) -> Result<()> {
    let path = state_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    let text = serde_json::to_string_pretty(state).map_err(|source| Error::Json {
        path: path.clone(),
        source,
    })?;
    std::fs::write(&path, text).map_err(|e| Error::io(&path, e))
}

/// Delete a directory tree, one entry at a time, retrying briefly.
///
/// Right after `SPI_SETCURSORS`, Windows is still reading the cursor files it was
/// just pointed at, and deletion fails transiently. It surfaces as
/// `ERROR_REPARSE_POINT_ENCOUNTERED` (4395) rather than a plain sharing
/// violation, because `std::fs::remove_dir_all` opens the directory with
/// reparse-point semantics and reports the busy child that way.
///
/// Removing files individually side-steps that entirely, and a short backoff
/// covers the window where Windows still has a handle open. Genuine permission
/// errors still surface after the last attempt.
pub fn remove_dir_all_retrying(dir: &Path) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    let mut last: Option<std::io::Error> = None;
    for attempt in 0..12u32 {
        match try_remove_tree(dir) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(40 * (attempt + 1) as u64));
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("could not remove directory")))
}

/// One pass: delete every file, then every directory, depth first.
fn try_remove_tree(dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // `file_type` does not follow links, so a symlinked directory is unlinked
        // rather than recursed into.
        if entry.file_type()?.is_dir() {
            try_remove_tree(&path)?;
        } else {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
    }
    match std::fs::remove_dir(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Where a pack's compiled cursors are installed.
pub fn install_dir_for(pack_name: &str) -> PathBuf {
    restore::data_root().join("installed").join(slug(pack_name))
}

fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "pack".into()
    } else {
        s
    }
}

/// The result of applying a pack.
#[derive(Debug, Clone)]
pub struct Applied {
    pub pack_name: String,
    pub install_dir: PathBuf,
    /// Roles we set, with the file each points at.
    pub roles: Vec<(String, PathBuf)>,
    /// Roles reset to the Windows default because this pack does not define them.
    pub reset_to_default: Vec<String>,
    pub warnings: Vec<Warning>,
}

/// Compile `pack_dir`, install it, and make Windows use it immediately.
///
/// Roles the pack does not define are reset to the Windows default rather than
/// left pointing at a previously applied pack, so switching packs never leaves a
/// mixture of the two.
pub fn apply(pack_dir: &Path) -> Result<Applied> {
    let pack = Pack::load(pack_dir)?;
    let built = build::build(&pack, pack_dir)?;

    // Capture the originals before touching anything, and refresh the escape hatch.
    restore::ensure_restore_point()?;
    restore::record_previous()?;

    let install_dir = install_dir_for(&pack.name);
    // Clear any previous build so a pack that drops a role cannot leave its old
    // cursor file behind.
    let _ = remove_dir_all_retrying(&install_dir);
    let written = built.write_to(&install_dir)?;

    // Windows is the authority on whether these bytes are a cursor. Check before
    // writing them into the registry, so a bad build cannot leave a broken pointer.
    for (role, path) in &written {
        crate::probe_cursor(path, None).map_err(|e| match e {
            Error::CursorRejected { path, msg } => Error::CursorRejected {
                path,
                msg: format!("role {role}: {msg}"),
            },
            other => other,
        })?;
    }

    let present = registry::roles_present()?;
    let mut roles = Vec::new();
    let mut reset = Vec::new();

    for role in &present {
        match written.iter().find(|(r, _)| r.eq_ignore_ascii_case(role)) {
            Some((_, path)) => {
                registry::set_role(role, &path.to_string_lossy())?;
                roles.push((role.clone(), path.clone()));
            }
            None => {
                registry::set_role(role, "")?;
                reset.push(role.clone());
            }
        }
    }

    let scheme_paths: Vec<(String, String)> = roles
        .iter()
        .map(|(r, p)| (r.clone(), p.to_string_lossy().to_string()))
        .collect();
    registry::register_scheme(&pack.name, &scheme_paths)?;
    registry::set_scheme_name(&pack.name)?;
    registry::set_scheme_source(scheme_source::USER)?;

    crate::broadcast()?;

    let mut state = load_state();
    state.active_pack = Some(pack.name.clone());
    state.active_pack_dir = Some(pack_dir.to_path_buf());
    state.last_pack = Some(pack.name.clone());
    state.last_pack_dir = Some(pack_dir.to_path_buf());
    save_state(&state)?;

    Ok(Applied {
        pack_name: pack.name,
        install_dir,
        roles,
        reset_to_default: reset,
        warnings: built.warnings,
    })
}

/// Put the Windows defaults back because the user asked for them.
///
/// This forgets the remembered pack too: the user wants Windows cursors, so the
/// next launch must not quietly put the pack back.
pub fn restore_defaults() -> Result<()> {
    restore::restore_original()?;
    let mut state = load_state();
    state.active_pack = None;
    state.active_pack_dir = None;
    state.last_pack = None;
    state.last_pack_dir = None;
    save_state(&state)?;
    Ok(())
}

/// Put the Windows defaults back on the way out, keeping the user's choice.
///
/// Used by "restore on exit": the point is to leave no trace while the app is not
/// running, not to undo the pack the user picked.
pub fn restore_defaults_for_exit() -> Result<()> {
    restore::restore_original()?;
    let mut state = load_state();
    state.active_pack = None;
    state.active_pack_dir = None;
    save_state(&state)?;
    Ok(())
}

/// Re-apply the remembered pack when nothing is currently applied.
///
/// Returns the pack name if it applied one. Called at startup so that logging in
/// restores the pointer the user chose, which is what "restore on exit" would
/// otherwise take away.
pub fn reapply_last_pack() -> Result<Option<String>> {
    let state = reconcile_state();
    if state.active_pack.is_some() {
        return Ok(None); // already applied; nothing to do
    }
    let Some(dir) = state.last_pack_dir.clone() else {
        return Ok(None);
    };
    if !dir.join("pack.json").is_file() {
        return Ok(None); // the pack was deleted or moved
    }
    let applied = apply(&dir)?;
    Ok(Some(applied.pack_name))
}

/// Check the recorded active pack against what the registry actually says, and
/// forget it if they disagree.
///
/// The two can drift: the user may change cursors in Windows Settings, run the
/// offline restore script, or apply a scheme from another tool. Trusting our own
/// state file alone would then show an active pack that is not really applied.
pub fn reconcile_state() -> State {
    let mut state = load_state();
    let Some(name) = state.active_pack.clone() else {
        return state;
    };

    let expected_dir = install_dir_for(&name);
    let still_applied = registry::roles_present()
        .unwrap_or_default()
        .iter()
        .filter_map(|r| registry::get_role(r).ok().flatten())
        .any(|value| {
            !value.is_empty() && Path::new(&value).starts_with(&expected_dir)
        });

    if !still_applied {
        state.active_pack = None;
        state.active_pack_dir = None;
        let _ = save_state(&state);
    }
    state
}

/// Remove an installed pack: its compiled cursors and its source folder.
///
/// If the pack is the one currently applied, the Windows defaults are restored
/// first — deleting the files out from under an active scheme would otherwise
/// leave the registry pointing at nothing.
pub fn delete_pack(pack_dir: &Path, pack_name: &str) -> Result<()> {
    let state = load_state();
    if state.active_pack_dir.as_deref() == Some(pack_dir) {
        restore_defaults()?;
    }

    remove_dir_all_retrying(&install_dir_for(pack_name))
        .map_err(|e| Error::io(install_dir_for(pack_name), e))?;
    remove_dir_all_retrying(pack_dir).map_err(|e| Error::io(pack_dir, e))?;

    // The scheme entry in the Windows mouse settings dropdown goes too.
    let _ = registry::unregister_scheme(pack_name);
    Ok(())
}

/// Compile a pack without installing it, for the preview pane and validation.
pub fn dry_run(pack_dir: &Path) -> Result<(Pack, build::Built)> {
    let pack = Pack::load(pack_dir)?;
    let built = build::build(&pack, pack_dir)?;
    Ok((pack, built))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_safe_for_paths() {
        assert_eq!(slug("Inverted Default"), "inverted-default");
        assert_eq!(slug("Silk/Song: v2"), "silk-song--v2");
        assert_eq!(slug("  "), "pack");
        assert_eq!(slug("!!!"), "pack");
        assert!(!slug("../../etc").contains('.'), "must not escape the install dir");
    }
}
