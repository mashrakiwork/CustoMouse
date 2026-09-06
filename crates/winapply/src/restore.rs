//! Restore points, and the offline script that works without us.

use crate::registry::{self, RawValue, Snapshot};
use crate::{Error, Result};
use std::path::{Path, PathBuf};

/// `%LOCALAPPDATA%\CustoMouse`. Everything we generate lives here.
pub fn data_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("CustoMouse")
}

/// The very first snapshot, taken before we ever changed anything. Never overwritten.
pub fn original_path() -> PathBuf {
    data_root().join("restore-point.json")
}

/// The state immediately before the most recent apply, for one-step undo.
pub fn previous_path() -> PathBuf {
    data_root().join("previous.json")
}

/// The script that restores the defaults without needing this program at all.
pub fn script_path() -> PathBuf {
    data_root().join("Restore-Default-Cursors.cmd")
}

fn read_snapshot(path: &Path) -> Result<Snapshot> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    serde_json::from_str(&text).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn write_snapshot(path: &Path, snap: &Snapshot) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    let text = serde_json::to_string_pretty(snap).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })?;
    std::fs::write(path, text).map_err(|e| Error::io(path, e))
}

/// True once a restore point exists.
pub fn has_restore_point() -> bool {
    original_path().is_file()
}

/// Capture the original cursor settings, if that has not happened yet, and
/// (re)write the offline restore script.
///
/// Called before the first change of any kind.
pub fn ensure_restore_point() -> Result<Snapshot> {
    let path = original_path();
    if path.is_file() {
        // Still make sure the script exists; the user may have deleted it.
        let snap = read_snapshot(&path)?;
        if !script_path().is_file() {
            write_script_from(&snap)?;
        }
        return Ok(snap);
    }

    let snap = registry::snapshot()?;
    write_snapshot(&path, &snap)?;
    write_script_from(&snap)?;
    Ok(snap)
}

/// Record the state we are about to overwrite, so one apply can be undone.
pub fn record_previous() -> Result<()> {
    let snap = registry::snapshot()?;
    write_snapshot(&previous_path(), &snap)
}

/// Put the original Windows cursors back.
pub fn restore_original() -> Result<()> {
    let path = original_path();
    if !path.is_file() {
        return Err(Error::NoRestorePoint);
    }
    registry::restore(&read_snapshot(&path)?)
}

/// Undo the most recent apply.
pub fn restore_previous() -> Result<()> {
    let path = previous_path();
    if !path.is_file() {
        return restore_original();
    }
    registry::restore(&read_snapshot(&path)?)
}

/// Decode a `REG_SZ` / `REG_EXPAND_SZ` payload (UTF-16LE, usually null terminated).
pub fn decode_sz(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

fn decode_dword(bytes: &[u8]) -> Option<u32> {
    (bytes.len() >= 4).then(|| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Escape a value for use inside `reg add ... /d "..."` in a batch file.
fn cmd_escape(s: &str) -> String {
    // Percent signs would be treated as variable expansion by cmd.exe, and quotes
    // would end the argument early.
    s.replace('%', "%%").replace('"', "\"\"")
}

/// Write a plain `.cmd` that restores the original cursors using only `reg.exe`
/// and `rundll32`.
///
/// This is the safety net: it has no dependency on our executable, so it still
/// works if the app is broken or uninstalled.
///
/// It deliberately takes no snapshot argument and always reads the original
/// restore point itself. An earlier version accepted one, and a test handed it
/// the *current* registry while a pack was applied — silently rewriting the
/// user's escape hatch to restore that pack instead of Windows. There is only one
/// correct input, so the API no longer allows a wrong one.
pub fn write_script() -> Result<()> {
    let path = original_path();
    if !path.is_file() {
        return Err(Error::NoRestorePoint);
    }
    write_script_from(&read_snapshot(&path)?)
}

fn write_script_from(snap: &Snapshot) -> Result<()> {
    let path = script_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }

    let key = format!(r"HKCU\{}", registry::CURSORS_KEY);
    let mut s = String::new();
    s.push_str("@echo off\r\n");
    s.push_str("REM  Restores the Windows mouse cursors that were in place before\r\n");
    s.push_str("REM  CustoMouse was first used. Safe to run at any time, and it does\r\n");
    s.push_str("REM  not need CustoMouse to be installed or working.\r\n");
    s.push_str("REM  Only HKEY_CURRENT_USER is touched; no admin rights are required.\r\n\r\n");
    s.push_str("echo Restoring the original Windows cursors...\r\n\r\n");

    for v in &snap.values {
        let name = &v.name;
        let arg = if name.is_empty() {
            "/ve".to_string()
        } else {
            format!("/v \"{}\"", cmd_escape(name))
        };
        match v.vtype {
            1 | 2 => {
                let t = if v.vtype == 2 { "REG_EXPAND_SZ" } else { "REG_SZ" };
                let data = cmd_escape(&decode_sz(&v.bytes));
                s.push_str(&format!(
                    "reg add \"{key}\" {arg} /t {t} /d \"{data}\" /f >nul\r\n"
                ));
            }
            4 => {
                if let Some(n) = decode_dword(&v.bytes) {
                    s.push_str(&format!(
                        "reg add \"{key}\" {arg} /t REG_DWORD /d {n} /f >nul\r\n"
                    ));
                }
            }
            _ => {
                // Anything unusual is preserved as hex binary.
                let hex: String = v.bytes.iter().map(|b| format!("{b:02x}")).collect();
                s.push_str(&format!(
                    "reg add \"{key}\" {arg} /t REG_BINARY /d {hex} /f >nul\r\n"
                ));
            }
        }
    }

    // Remove any role we may have added that was not in the original snapshot.
    let had = snap.names();
    for role in cursorpack::manifest::ROLES.iter().map(|r| r.win) {
        if !had.contains(&role.to_lowercase()) {
            s.push_str(&format!(
                "reg delete \"{key}\" /v \"{role}\" /f >nul 2>nul\r\n"
            ));
        }
    }

    s.push_str("\r\nREM  Make the change take effect immediately.\r\n");
    s.push_str("rundll32.exe user32.dll,UpdatePerUserSystemParameters 1, True\r\n\r\n");
    s.push_str("echo Done. Your original cursors are back.\r\n");
    s.push_str("pause\r\n");

    std::fs::write(&path, s).map_err(|e| Error::io(&path, e))
}

/// Roles the original snapshot did not define, i.e. ones we would be adding.
pub fn roles_added_by_us(snap: &Snapshot) -> Vec<String> {
    let had = snap.names();
    cursorpack::manifest::ROLES
        .iter()
        .map(|r| r.win)
        .filter(|r| !had.contains(&r.to_lowercase()))
        .map(|r| r.to_string())
        .collect()
}

/// Values in the snapshot, for display in the UI.
pub fn snapshot_summary(snap: &Snapshot) -> Vec<(String, String)> {
    snap.values
        .iter()
        .map(|v: &RawValue| {
            let shown = match v.vtype {
                1 | 2 => {
                    let s = decode_sz(&v.bytes);
                    if s.is_empty() {
                        "(Windows default)".to_string()
                    } else {
                        s
                    }
                }
                4 => decode_dword(&v.bytes).map(|n| n.to_string()).unwrap_or_default(),
                _ => format!("{} bytes", v.bytes.len()),
            };
            let name = if v.name.is_empty() {
                "(scheme name)".to_string()
            } else {
                v.name.clone()
            };
            (name, shown)
        })
        .collect()
}
