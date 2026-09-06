//! Turning a folder of `.cur` / `.ani` files into a pack.
//!
//! Downloaded cursor sets ship their role mapping in one of three ways, and they
//! are tried in descending order of reliability:
//!
//! 1. **A `.crs` scheme file** — INI sections named after the Windows roles
//!    exactly (`[Arrow]`, `[SizeNWSE]`, …) each with a `Path=`. Unambiguous.
//! 2. **`install.inf`** — a `[Strings]` section with standardised keys
//!    (`pointer`, `busy`, `link`, …).
//! 3. **File names** — a last resort, and genuinely unreliable: real packs use
//!    names like `SilksongBackBusyCursor.ani` (which is *AppStarting*, not Wait)
//!    and `SilksongUnavalibleCursor.cur` (misspelled). Guessing gets these wrong,
//!    so a scheme file always wins where one exists.

use crate::manifest::{Pack, RoleSpec, ROLES};
use crate::{Error, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The `[Strings]` keys Windows cursor scheme INFs use, and the role each means.
///
/// Two of these are counter-intuitive and get mixed up constantly:
/// `hand` is the *handwriting pen*, and `link` is the pointing hand.
const INF_KEYS: &[(&str, &str)] = &[
    ("pointer", "Arrow"),
    ("help", "Help"),
    ("work", "AppStarting"),
    ("busy", "Wait"),
    ("cross", "Crosshair"),
    ("text", "IBeam"),
    ("hand", "NWPen"),
    ("unavailiable", "No"), // Microsoft's own misspelling, used verbatim in real INFs
    ("unavailable", "No"),
    ("vert", "SizeNS"),
    ("horz", "SizeWE"),
    ("dgn1", "SizeNWSE"),
    ("dgn2", "SizeNESW"),
    ("move", "SizeAll"),
    ("alternate", "UpArrow"),
    ("link", "Hand"),
    ("person", "Person"),
    ("pin", "Pin"),
];

/// Filename fragments that suggest a role, longest match wins.
///
/// Ordered so that specific patterns beat generic ones; `no` would otherwise
/// swallow `normal`.
const NAME_HINTS: &[(&str, &str)] = &[
    ("appstarting", "AppStarting"),
    ("app_starting", "AppStarting"),
    ("working", "AppStarting"),
    ("progress", "AppStarting"),
    ("startup", "AppStarting"),
    ("unavailiable", "No"),
    ("unavailable", "No"),
    ("unavalible", "No"), // seen in the wild; misspellings are common here
    ("unavailible", "No"),
    ("forbidden", "No"),
    ("nodrop", "No"),
    ("no_drop", "No"),
    ("notallowed", "No"),
    ("crosshair", "Crosshair"),
    ("precision", "Crosshair"),
    ("cross", "Crosshair"),
    ("handwriting", "NWPen"),
    ("writing", "NWPen"),
    ("nwpen", "NWPen"),
    ("pencil", "NWPen"),
    ("pen", "NWPen"),
    ("alternate", "UpArrow"),
    ("altcursor", "UpArrow"),
    ("uparrow", "UpArrow"),
    ("up_arrow", "UpArrow"),
    ("diagonal1", "SizeNWSE"),
    ("nwse", "SizeNWSE"),
    ("fdiag", "SizeNWSE"),
    ("diag1", "SizeNWSE"),
    ("dgn1", "SizeNWSE"),
    ("diagonal2", "SizeNESW"),
    ("nesw", "SizeNESW"),
    ("bdiag", "SizeNESW"),
    ("diag2", "SizeNESW"),
    ("dgn2", "SizeNESW"),
    ("vertical", "SizeNS"),
    ("size_ns", "SizeNS"),
    ("vert", "SizeNS"),
    ("horizontal", "SizeWE"),
    ("size_we", "SizeWE"),
    ("size_ew", "SizeWE"),
    ("horz", "SizeWE"),
    ("sizeall", "SizeAll"),
    ("size_all", "SizeAll"),
    ("fleur", "SizeAll"),
    ("move", "SizeAll"),
    ("ibeam", "IBeam"),
    ("beam", "IBeam"),
    ("text", "IBeam"),
    ("link", "Hand"),
    ("hand", "Hand"),
    ("person", "Person"),
    ("pin", "Pin"),
    ("busy", "Wait"),
    ("wait", "Wait"),
    ("loading", "Wait"),
    ("help", "Help"),
    ("normal", "Arrow"),
    ("pointer", "Arrow"),
    ("default", "Arrow"),
    ("standard", "Arrow"),
    ("arrow", "Arrow"),
];

/// Where a mapping came from, in descending order of trustworthiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapSource {
    /// A `.crs` scheme file naming roles explicitly.
    SchemeFile,
    /// The `[Strings]` section of an `install.inf`.
    Inf,
    /// Guessed from the file name.
    FileName,
    /// Nothing identified the files, so they were assigned in order. A guess,
    /// and flagged as one.
    Position,
}

impl MapSource {
    pub fn label(self) -> &'static str {
        match self {
            MapSource::SchemeFile => "the .crs scheme file",
            MapSource::Inf => "install.inf",
            MapSource::FileName => "file names",
            MapSource::Position => "file order (a guess)",
        }
    }

    /// True when the mapping is reliable enough not to need checking.
    pub fn is_reliable(self) -> bool {
        matches!(self, MapSource::SchemeFile | MapSource::Inf)
    }
}

/// Sort key that orders `Blue-2` before `Blue-10`.
///
/// Numbered sets rely on this: a plain lexicographic sort would interleave them
/// and silently shift every role by one.
fn natural_key(name: &str) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.peek().copied() {
        if c.is_ascii_digit() {
            let mut n = String::new();
            while let Some(d) = chars.peek().copied().filter(|d| d.is_ascii_digit()) {
                n.push(d);
                chars.next();
            }
            out.push((n.parse().unwrap_or(u64::MAX), String::new()));
        } else {
            let mut t = String::new();
            while let Some(d) = chars.peek().copied().filter(|d| !d.is_ascii_digit()) {
                t.push(d.to_ascii_lowercase());
                chars.next();
            }
            out.push((0, t));
        }
    }
    out
}

/// How one file got mapped, so the UI can show its work and let it be corrected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub file: PathBuf,
    pub role: String,
    pub source: MapSource,
}

/// Result of scanning a folder.
#[derive(Debug, Clone)]
pub struct Import {
    pub pack: Pack,
    pub mappings: Vec<Mapping>,
    /// Cursor files present that could not be matched to a role.
    pub unmatched: Vec<PathBuf>,
    /// The scheme or INF file the mapping came from, if any.
    pub scheme_file: Option<PathBuf>,
    /// The strongest source any mapping used.
    pub source: MapSource,
}

fn is_cursor_file(path: &Path) -> bool {
    path.is_file()
        && matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str(),
            "cur" | "ani"
        )
}

/// Parse the `[Strings]` section of an `install.inf` into role -> file name.
fn parse_inf(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut in_strings = false;

    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_strings = line.eq_ignore_ascii_case("[Strings]");
            continue;
        }
        if !in_strings || line.is_empty() || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().trim_matches('"').trim();
        if value.is_empty() {
            continue;
        }
        if let Some((_, role)) = INF_KEYS.iter().find(|(k, _)| *k == key) {
            // A later duplicate should not clobber an earlier good mapping.
            out.entry(role.to_string()).or_insert_with(|| value.to_string());
        }
    }
    out
}

/// Parse a `.crs` scheme file: `[RoleName]` sections each carrying a `Path=`.
///
/// Section names are the Windows role names verbatim, so anything not in [`ROLES`]
/// is ignored rather than guessed at. The files are UTF-8 with a BOM in practice.
fn parse_crs(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut current: Option<&'static str> = None;

    for raw in text.lines() {
        let line = raw.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = ROLES
                .iter()
                .find(|r| r.win.eq_ignore_ascii_case(name.trim()))
                .map(|r| r.win);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("path") {
            continue;
        }
        let value = value.trim().trim_matches('"').trim();
        if value.is_empty() {
            continue;
        }
        if let Some(role) = current {
            // A path may be absolute or include folders; only the name matters.
            let file = value
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or(value)
                .to_string();
            out.entry(role.to_string()).or_insert(file);
        }
    }
    out
}

/// Guess a role from a file name.
pub fn role_from_name(file_stem: &str) -> Option<&'static str> {
    let stem = file_stem.to_ascii_lowercase();
    NAME_HINTS
        .iter()
        .find(|(frag, _)| stem.contains(frag))
        .map(|(_, role)| *role)
}

/// Scan `dir` for cursor files and build a pack from them.
///
/// Nothing is written; the caller decides whether to save.
pub fn from_folder(dir: &Path, name: &str) -> Result<Import> {
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| Error::io(dir, e))?
        .flatten()
        .map(|e| e.path())
        .collect();

    let mut files: Vec<PathBuf> = entries.iter().filter(|p| is_cursor_file(p)).cloned().collect();
    files.sort();

    if files.is_empty() {
        return Err(Error::NoFrames {
            dir: dir.to_path_buf(),
        });
    }

    // Strongest source first: an explicit .crs scheme, then install.inf.
    let crs_path = entries
        .iter()
        .find(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("crs"))
        })
        .cloned();

    let inf_path = entries
        .iter()
        .find(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case("install.inf"))
        })
        .cloned();

    let (declared, source, scheme_file) = if let Some(path) = &crs_path {
        let text = read_text(path)?;
        (parse_crs(&text), MapSource::SchemeFile, Some(path.clone()))
    } else if let Some(path) = &inf_path {
        let text = read_text(path)?;
        (parse_inf(&text), MapSource::Inf, Some(path.clone()))
    } else {
        (BTreeMap::new(), MapSource::FileName, None)
    };

    let mut mappings: Vec<Mapping> = Vec::new();
    let mut taken: Vec<String> = Vec::new();

    for (role, file_name) in &declared {
        let found = files.iter().find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(file_name))
        });
        if let Some(path) = found {
            mappings.push(Mapping {
                file: path.clone(),
                role: role.clone(),
                source,
            });
            taken.push(role.clone());
        }
    }

    // Anything the scheme did not cover falls back to name guessing.
    let mut unmatched = Vec::new();
    for path in &files {
        if mappings.iter().any(|m| &m.file == path) {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        match role_from_name(stem) {
            Some(role) if !taken.iter().any(|t| t == role) => {
                taken.push(role.to_string());
                mappings.push(Mapping {
                    file: path.clone(),
                    role: role.to_string(),
                    source: MapSource::FileName,
                });
            }
            _ => unmatched.push(path.clone()),
        }
    }

    // Nothing identified them at all. Sets named `Blue-1..Blue-17` are common and
    // conventionally follow the standard Windows pointer order, so assign
    // positionally and flag it as a guess rather than refusing outright.
    let mut source = source;
    if mappings.is_empty() {
        if files.len() > ROLES.len() {
            return Err(Error::Other(format!(
                "found {} cursor file(s) in {}, which is more than the {} Windows pointers, \
                 and nothing says which is which. Add a .crs or install.inf, or name them \
                 after the pointer they replace (for example arrow.cur, busy.ani).",
                files.len(),
                dir.display(),
                ROLES.len()
            )));
        }

        let mut ordered = files.clone();
        ordered.sort_by_key(|p| natural_key(p.file_name().and_then(|n| n.to_str()).unwrap_or("")));

        unmatched.clear();
        source = MapSource::Position;
        for (path, role) in ordered.iter().zip(ROLES.iter()) {
            mappings.push(Mapping {
                file: path.clone(),
                role: role.win.to_string(),
                source: MapSource::Position,
            });
        }
    }

    mappings.sort_by_key(|m| {
        ROLES
            .iter()
            .position(|r| r.win == m.role)
            .unwrap_or(usize::MAX)
    });

    let mut roles = BTreeMap::new();
    for m in &mappings {
        let rel = m.file.strip_prefix(dir).unwrap_or(&m.file).to_path_buf();
        roles.insert(
            m.role.clone(),
            RoleSpec {
                source: Some(rel),
                // Left unset on purpose: the file already carries its hotspot.
                hotspot: None,
                ..Default::default()
            },
        );
    }

    let pack = Pack {
        format: 1,
        name: name.to_string(),
        author: None,
        version: Some("1.0.0".into()),
        description: Some(format!(
            "Imported from {} ready-made cursor file(s).",
            mappings.len()
        )),
        base_size: 32,
        sizes: crate::DEFAULT_SIZES.to_vec(),
        roles,
    };
    pack.validate()?;

    Ok(Import {
        pack,
        mappings,
        unmatched,
        scheme_file,
        source,
    })
}

/// Read a text file, tolerating a UTF-8 BOM and invalid sequences.
///
/// Scheme files are frequently written by Windows tools that emit a BOM, and a
/// stray non-UTF-8 byte should not make an otherwise good mapping unusable.
fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(text.trim_start_matches('\u{feff}').to_string())
}

impl Import {
    /// Copy the mapped cursor files into `dest` and write a `pack.json` beside
    /// them, so the resulting pack is self-contained and can be moved or shared.
    pub fn materialize(&self, dest: &Path) -> Result<()> {
        std::fs::create_dir_all(dest).map_err(|e| Error::io(dest, e))?;

        for m in &self.mappings {
            let name = m
                .file
                .file_name()
                .ok_or_else(|| Error::Other(format!("{} has no file name", m.file.display())))?;
            let to = dest.join(name);
            std::fs::copy(&m.file, &to).map_err(|e| Error::io(&to, e))?;
        }

        self.pack.save(dest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inf_strings_section_is_parsed() {
        let inf = r#"
[Version]
signature="$CHICAGO$"

[Scheme.Reg]
HKCU,"Control Panel\Cursors\Schemes","%SCHEME_NAME%",,"..."

[Strings]
SCHEME_NAME = "My Pack"
pointer     = "normal.cur"
help        = "help.cur"
work        = "working.ani"
busy        = "busy.ani"
cross       = "cross.cur"
text        = "beam.cur"
hand        = "pen.cur"
unavailiable= "no.cur"
vert        = "ns.cur"
horz        = "we.cur"
dgn1        = "nwse.cur"
dgn2        = "nesw.cur"
move        = "move.cur"
alternate   = "up.cur"
link        = "link.cur"
"#;
        let map = parse_inf(inf);
        assert_eq!(map.get("Arrow").unwrap(), "normal.cur");
        assert_eq!(map.get("Wait").unwrap(), "busy.ani");
        assert_eq!(map.get("AppStarting").unwrap(), "working.ani");
        // The two that are routinely confused.
        assert_eq!(map.get("NWPen").unwrap(), "pen.cur", "inf 'hand' means the pen");
        assert_eq!(map.get("Hand").unwrap(), "link.cur", "inf 'link' is the hand");
        assert_eq!(map.get("No").unwrap(), "no.cur");
        assert_eq!(map.len(), 15);
        // Keys outside [Strings] must be ignored.
        assert!(!map.values().any(|v| v.contains("Control Panel")));
    }

    #[test]
    fn crs_sections_are_parsed() {
        // Real files are UTF-8 with a BOM and CRLF line endings.
        let crs = "\u{feff}[Arrow]\r\nPath=normal.cur\r\n\r\n[Wait]\r\nPath=weaver.ani\r\n\
                   \r\n[AppStarting]\r\nPath=backbusy.ani\r\n\r\n[NotARole]\r\nPath=junk.cur\r\n\
                   \r\n[SizeNWSE]\r\nPath=C:\\somewhere\\diag1.cur\r\n\r\n[Crosshair]\r\nPath=\r\n";
        let map = parse_crs(crs);

        assert_eq!(map.get("Arrow").unwrap(), "normal.cur", "BOM must not break the first section");
        assert_eq!(map.get("Wait").unwrap(), "weaver.ani");
        assert_eq!(map.get("AppStarting").unwrap(), "backbusy.ani");
        assert_eq!(map.get("SizeNWSE").unwrap(), "diag1.cur", "a full path should reduce to its file name");
        assert!(!map.contains_key("NotARole"), "unknown sections are ignored, not guessed");
        assert!(!map.contains_key("Crosshair"), "an empty Path is not a mapping");
    }

    /// Regression from a real downloaded pack: its file names lead the heuristics
    /// to the wrong answer, and the `.crs` has to override them.
    ///
    /// `SilksongBackBusyCursor.ani` contains "busy" but is *AppStarting*; the real
    /// Wait cursor is `SilksongBusyWeaver.ani`, which contains no useful hint at all.
    #[test]
    fn scheme_file_beats_misleading_file_names() {
        let dir = std::env::temp_dir().join("custo-import-crs");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        for n in [
            "SilksongNormalCursor.cur",
            "SilksongBackBusyCursor.ani",
            "SilksongBusyWeaver.ani",
            "SilksongLinkCursorGeo.cur",
        ] {
            std::fs::write(dir.join(n), b"placeholder").unwrap();
        }
        std::fs::write(
            dir.join("scheme.crs"),
            "\u{feff}[Arrow]\r\nPath=SilksongNormalCursor.cur\r\n\
             [AppStarting]\r\nPath=SilksongBackBusyCursor.ani\r\n\
             [Wait]\r\nPath=SilksongBusyWeaver.ani\r\n\
             [Pin]\r\nPath=SilksongLinkCursorGeo.cur\r\n",
        )
        .unwrap();

        // What name guessing alone would have concluded, for contrast.
        assert_eq!(role_from_name("SilksongBackBusyCursor"), Some("Wait"));

        let import = from_folder(&dir, "Silksong").unwrap();
        assert_eq!(import.source, MapSource::SchemeFile);
        assert_eq!(import.mappings.len(), 4);
        assert!(import.unmatched.is_empty());

        let role_of = |file: &str| {
            import
                .mappings
                .iter()
                .find(|m| m.file.file_name().unwrap() == file)
                .map(|m| m.role.as_str())
                .unwrap()
        };
        assert_eq!(role_of("SilksongBackBusyCursor.ani"), "AppStarting");
        assert_eq!(role_of("SilksongBusyWeaver.ani"), "Wait");
        assert_eq!(role_of("SilksongLinkCursorGeo.cur"), "Pin");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A `.crs` must take priority over an `install.inf` sitting beside it.
    #[test]
    fn scheme_file_outranks_install_inf() {
        let dir = std::env::temp_dir().join("custo-import-both");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("a.cur"), b"x").unwrap();
        std::fs::write(dir.join("b.cur"), b"x").unwrap();
        std::fs::write(dir.join("scheme.crs"), "[Arrow]\r\nPath=a.cur\r\n").unwrap();
        std::fs::write(dir.join("install.inf"), "[Strings]\r\npointer = \"b.cur\"\r\n").unwrap();

        let import = from_folder(&dir, "Both").unwrap();
        assert_eq!(import.source, MapSource::SchemeFile);
        let arrow = import.mappings.iter().find(|m| m.role == "Arrow").unwrap();
        assert_eq!(arrow.file.file_name().unwrap(), "a.cur");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Names that a real pack uses and the old hint list missed.
    /// Numbered sets must sort 2 before 10, or every role shifts by one.
    #[test]
    fn natural_sort_orders_numbers_numerically() {
        let mut names = vec!["Blue-10.cur", "Blue-2.cur", "Blue-1.cur", "Blue-17.cur"];
        names.sort_by_key(|n| natural_key(n));
        assert_eq!(names, vec!["Blue-1.cur", "Blue-2.cur", "Blue-10.cur", "Blue-17.cur"]);
    }

    /// A set of anonymously named files still imports, in canonical role order,
    /// clearly flagged as guesswork.
    #[test]
    fn anonymous_sets_map_by_position() {
        let dir = std::env::temp_dir().join("custo-import-pos");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 1..=17 {
            std::fs::write(dir.join(format!("Blue-{i}.cur")), b"x").unwrap();
        }

        let import = from_folder(&dir, "Blue").unwrap();
        assert_eq!(import.source, MapSource::Position);
        assert!(!import.source.is_reliable(), "positional mapping must be flagged");
        assert_eq!(import.mappings.len(), 17);
        assert!(import.unmatched.is_empty());

        let of = |f: &str| {
            import.mappings.iter()
                .find(|m| m.file.file_name().unwrap() == f)
                .map(|m| m.role.as_str()).unwrap()
        };
        assert_eq!(of("Blue-1.cur"), "Arrow");
        assert_eq!(of("Blue-2.cur"), "Help");
        assert_eq!(of("Blue-10.cur"), "SizeWE", "10 must not sort next to 1");
        assert_eq!(of("Blue-17.cur"), "Person");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// More files than there are pointers means positional guessing is meaningless.
    #[test]
    fn oversized_anonymous_sets_are_refused() {
        let dir = std::env::temp_dir().join("custo-import-toomany");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 1..=25 {
            std::fs::write(dir.join(format!("zz{i}.cur")), b"x").unwrap();
        }
        assert!(from_folder(&dir, "Too Many").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn name_hints_cover_real_world_spellings() {
        assert_eq!(role_from_name("SilksongDiagonal1Cursor"), Some("SizeNWSE"));
        assert_eq!(role_from_name("SilksongDiagonal2Cursor"), Some("SizeNESW"));
        assert_eq!(role_from_name("SilksongUnavalibleCursor"), Some("No"));
        assert_eq!(role_from_name("SilksongWritingCursor"), Some("NWPen"));
        assert_eq!(role_from_name("SilksongAltCursor"), Some("UpArrow"));
    }

    #[test]
    fn name_hints_prefer_specific_matches() {
        assert_eq!(role_from_name("normal"), Some("Arrow"));
        assert_eq!(role_from_name("Arrow"), Some("Arrow"));
        assert_eq!(role_from_name("busy"), Some("Wait"));
        assert_eq!(role_from_name("working"), Some("AppStarting"));
        assert_eq!(role_from_name("crosshair"), Some("Crosshair"));
        assert_eq!(role_from_name("size_nwse"), Some("SizeNWSE"));
        assert_eq!(role_from_name("link"), Some("Hand"));
        assert_eq!(role_from_name("handwriting"), Some("NWPen"));
        assert_eq!(role_from_name("zzz"), None);
    }

    /// "normal" contains "no" — the generic pattern must not win.
    #[test]
    fn generic_patterns_do_not_swallow_specific_ones() {
        assert_eq!(role_from_name("normal"), Some("Arrow"));
        assert_eq!(role_from_name("normal_select"), Some("Arrow"));
        assert_eq!(role_from_name("unavailable"), Some("No"));
        assert_eq!(role_from_name("precision"), Some("Crosshair"));
    }

    #[test]
    fn a_role_is_never_assigned_twice() {
        let dir = std::env::temp_dir().join("custo-import-dup");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Two files that both look like the arrow.
        for n in ["arrow.cur", "normal.cur", "busy.ani"] {
            std::fs::write(dir.join(n), b"placeholder").unwrap();
        }

        let import = from_folder(&dir, "Dup").unwrap();
        let arrows = import.mappings.iter().filter(|m| m.role == "Arrow").count();
        assert_eq!(arrows, 1, "only one file may claim Arrow");
        assert_eq!(import.unmatched.len(), 1, "the loser should be reported");
        std::fs::remove_dir_all(&dir).ok();
    }
}
