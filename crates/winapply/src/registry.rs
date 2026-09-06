//! Reading and writing `HKCU\Control Panel\Cursors`.
//!
//! Everything here is per-user. No admin rights, and nothing under
//! `C:\Windows` is ever touched, so a restore is purely a registry operation
//! and cannot be broken by a missing or deleted art file.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_SZ};
use winreg::{RegKey, RegValue};

pub const CURSORS_KEY: &str = r"Control Panel\Cursors";
pub const SCHEMES_KEY: &str = r"Control Panel\Cursors\Schemes";

/// `Scheme Source` values Windows understands.
pub mod scheme_source {
    /// No scheme; Windows defaults.
    pub const NONE: u32 = 0;
    /// A user-defined scheme, which is what we install.
    pub const USER: u32 = 1;
    /// A scheme that shipped with Windows.
    pub const SYSTEM: u32 = 2;
}

/// The order role paths appear in a scheme string, which is fixed by Windows.
/// `Pin` and `Person` come last and only exist on some installs.
pub const SCHEME_ORDER: &[&str] = &[
    "Arrow",
    "Help",
    "AppStarting",
    "Wait",
    "Crosshair",
    "IBeam",
    "NWPen",
    "No",
    "SizeNS",
    "SizeWE",
    "SizeNWSE",
    "SizeNESW",
    "SizeAll",
    "UpArrow",
    "Hand",
    "Pin",
    "Person",
];

/// One registry value captured exactly as stored, so a restore is byte-identical.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawValue {
    pub name: String,
    /// The `REG_*` type code.
    pub vtype: u32,
    pub bytes: Vec<u8>,
}

/// A complete capture of the cursors key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// RFC3339-ish local timestamp, for the UI.
    pub taken: String,
    pub values: Vec<RawValue>,
}

impl Snapshot {
    pub fn get(&self, name: &str) -> Option<&RawValue> {
        self.values.iter().find(|v| v.name.eq_ignore_ascii_case(name))
    }

    /// Value names, lowercased, for set comparisons.
    pub fn names(&self) -> BTreeSet<String> {
        self.values.iter().map(|v| v.name.to_lowercase()).collect()
    }
}

fn open(write: bool) -> Result<RegKey> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let access = if write { KEY_READ | KEY_WRITE } else { KEY_READ };
    hkcu.open_subkey_with_flags(CURSORS_KEY, access)
        .map_err(|e| Error::Registry {
            key: CURSORS_KEY.into(),
            source: e,
        })
}

fn now_stamp() -> String {
    // Avoiding a date crate for one string: seconds since the epoch is enough to
    // order restore points, and the UI formats it.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Capture every value under the cursors key.
pub fn snapshot() -> Result<Snapshot> {
    let key = open(false)?;
    let mut values = Vec::new();
    for item in key.enum_values() {
        let (name, val) = item.map_err(|e| Error::Registry {
            key: CURSORS_KEY.into(),
            source: e,
        })?;
        values.push(RawValue {
            name,
            vtype: val.vtype as u32,
            bytes: val.bytes,
        });
    }
    values.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Snapshot {
        taken: now_stamp(),
        values,
    })
}

/// Put the cursors key back exactly as the snapshot found it.
///
/// Values added since the snapshot are deleted, so the result is byte-identical
/// rather than merely "close".
pub fn restore(snap: &Snapshot) -> Result<()> {
    let key = open(true)?;

    let current = snapshot()?;
    let wanted = snap.names();
    for v in &current.values {
        if !wanted.contains(&v.name.to_lowercase()) {
            key.delete_value(&v.name).map_err(|e| Error::Registry {
                key: format!(r"{CURSORS_KEY}\{}", v.name),
                source: e,
            })?;
        }
    }

    for v in &snap.values {
        let raw = RegValue {
            vtype: enum_from_u32(v.vtype),
            bytes: v.bytes.clone(),
        };
        key.set_raw_value(&v.name, &raw).map_err(|e| Error::Registry {
            key: format!(r"{CURSORS_KEY}\{}", v.name),
            source: e,
        })?;
    }

    crate::broadcast()?;
    Ok(())
}

fn enum_from_u32(v: u32) -> winreg::enums::RegType {
    use winreg::enums::RegType::*;
    match v {
        0 => REG_NONE,
        1 => REG_SZ,
        2 => REG_EXPAND_SZ,
        3 => REG_BINARY,
        4 => REG_DWORD,
        5 => REG_DWORD_BIG_ENDIAN,
        6 => REG_LINK,
        7 => REG_MULTI_SZ,
        11 => REG_QWORD,
        _ => REG_BINARY,
    }
}

/// Role names this Windows install actually has, intersected with the roles we know.
///
/// `Pin` and `Person` are absent on many installs, so hardcoding the list would
/// create values Windows never reads.
pub fn roles_present() -> Result<Vec<String>> {
    let snap = snapshot()?;
    let have = snap.names();
    Ok(cursorpack::manifest::ROLES
        .iter()
        .map(|r| r.win)
        .filter(|r| have.contains(&r.to_lowercase()))
        .map(|r| r.to_string())
        .collect())
}

/// Point a role at a cursor file. An empty path means "use the Windows default".
pub fn set_role(role: &str, path: &str) -> Result<()> {
    let key = open(true)?;
    key.set_value(role, &path.to_string())
        .map_err(|e| Error::Registry {
            key: format!(r"{CURSORS_KEY}\{role}"),
            source: e,
        })
}

pub fn get_role(role: &str) -> Result<Option<String>> {
    let key = open(false)?;
    match key.get_value::<String, _>(role) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Registry {
            key: format!(r"{CURSORS_KEY}\{role}"),
            source: e,
        }),
    }
}

/// The name shown in Settings, stored as the key's default value.
pub fn set_scheme_name(name: &str) -> Result<()> {
    let key = open(true)?;
    key.set_value("", &name.to_string()).map_err(|e| Error::Registry {
        key: CURSORS_KEY.into(),
        source: e,
    })
}

pub fn set_scheme_source(source: u32) -> Result<()> {
    let key = open(true)?;
    key.set_value("Scheme Source", &source).map_err(|e| Error::Registry {
        key: format!(r"{CURSORS_KEY}\Scheme Source"),
        source: e,
    })
}

pub fn get_base_size() -> Result<u32> {
    let key = open(false)?;
    Ok(key.get_value::<u32, _>("CursorBaseSize").unwrap_or(32))
}

pub fn set_base_size(px: u32) -> Result<()> {
    let key = open(true)?;
    key.set_value("CursorBaseSize", &px).map_err(|e| Error::Registry {
        key: format!(r"{CURSORS_KEY}\CursorBaseSize"),
        source: e,
    })
}

/// Register the scheme under `Cursors\Schemes` so it also appears in the
/// Windows mouse settings dropdown, like any other installed scheme.
pub fn register_scheme(name: &str, paths: &[(String, String)]) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(SCHEMES_KEY)
        .map_err(|e| Error::Registry {
            key: SCHEMES_KEY.into(),
            source: e,
        })?;

    let present = roles_present().unwrap_or_default();
    let value: Vec<String> = SCHEME_ORDER
        .iter()
        .filter(|r| present.iter().any(|p| p.eq_ignore_ascii_case(r)))
        .map(|role| {
            paths
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(role))
                .map(|(_, p)| p.clone())
                .unwrap_or_default()
        })
        .collect();

    key.set_value(name, &value.join(",")).map_err(|e| Error::Registry {
        key: format!(r"{SCHEMES_KEY}\{name}"),
        source: e,
    })
}

pub fn unregister_scheme(name: &str) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey_with_flags(SCHEMES_KEY, KEY_READ | KEY_WRITE) else {
        return Ok(());
    };
    match key.delete_value(name) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Registry {
            key: format!(r"{SCHEMES_KEY}\{name}"),
            source: e,
        }),
    }
}

/// Value types we write, exposed for tests that assert we did not change a type.
pub const OUR_PATH_TYPE: u32 = REG_SZ as u32;
pub const OUR_DWORD_TYPE: u32 = REG_DWORD as u32;
