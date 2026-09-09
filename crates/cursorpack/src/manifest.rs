//! The pack manifest (`pack.json`) — the "template" that describes how a mouse
//! looks and animates.

use crate::{Error, Result, DEFAULT_SIZES};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A Windows cursor role and how it maps elsewhere.
pub struct Role {
    /// Value name under `HKCU\Control Panel\Cursors`.
    pub win: &'static str,
    /// What to call it in the UI.
    pub label: &'static str,
    /// What it is for, shown as help text in the import wizard.
    pub hint: &'static str,
    /// Xcursor names for the Linux exporter. The first is the canonical file;
    /// the rest become symlinks.
    pub xcursor: &'static [&'static str],
}

/// Every role Windows can use. `Person` and `Pin` only exist on some installs, so
/// the applier intersects this list with what the live registry actually has.
pub const ROLES: &[Role] = &[
    Role {
        win: "Arrow",
        label: "Normal Select",
        hint: "The everyday pointer. This is the one people notice — start here.",
        xcursor: &["default", "left_ptr", "arrow", "top_left_arrow"],
    },
    Role {
        win: "Help",
        label: "Help Select",
        hint: "Arrow with a question mark.",
        xcursor: &["help", "question_arrow", "whats_this", "left_ptr_help"],
    },
    Role {
        win: "AppStarting",
        label: "Working in Background",
        hint: "Arrow plus a spinner. Usually animated.",
        xcursor: &["progress", "left_ptr_watch", "half-busy"],
    },
    Role {
        win: "Wait",
        label: "Busy",
        hint: "The full busy spinner. Usually animated.",
        xcursor: &["wait", "watch"],
    },
    Role {
        win: "Crosshair",
        label: "Precision Select",
        hint: "Crosshair for precise picking.",
        xcursor: &["crosshair", "cross", "tcross"],
    },
    Role {
        win: "IBeam",
        label: "Text Select",
        hint: "The text caret. Keep it thin or text gets hard to click.",
        xcursor: &["text", "xterm", "ibeam"],
    },
    Role {
        win: "NWPen",
        label: "Handwriting",
        hint: "Pen input.",
        xcursor: &["pencil", "draft"],
    },
    Role {
        win: "No",
        label: "Unavailable",
        hint: "The blocked / not-allowed symbol.",
        xcursor: &["not-allowed", "no-drop", "circle", "forbidden"],
    },
    Role {
        win: "SizeNS",
        label: "Resize Vertical",
        hint: "Up-down resize arrow.",
        xcursor: &["ns-resize", "sb_v_double_arrow", "v_double_arrow", "size_ver"],
    },
    Role {
        win: "SizeWE",
        label: "Resize Horizontal",
        hint: "Left-right resize arrow.",
        xcursor: &["ew-resize", "sb_h_double_arrow", "h_double_arrow", "size_hor"],
    },
    Role {
        win: "SizeNWSE",
        label: "Resize Diagonal 1",
        hint: "Top-left to bottom-right resize.",
        xcursor: &["nwse-resize", "size_fdiag", "nw-resize", "se-resize"],
    },
    Role {
        win: "SizeNESW",
        label: "Resize Diagonal 2",
        hint: "Top-right to bottom-left resize.",
        xcursor: &["nesw-resize", "size_bdiag", "ne-resize", "sw-resize"],
    },
    Role {
        win: "SizeAll",
        label: "Move",
        hint: "Four-way move arrow.",
        xcursor: &["all-scroll", "fleur", "move", "size_all"],
    },
    Role {
        win: "UpArrow",
        label: "Alternate Select",
        hint: "Straight up arrow.",
        xcursor: &["up-arrow", "center_ptr", "sb_up_arrow"],
    },
    Role {
        win: "Hand",
        label: "Link Select",
        hint: "The pointing hand used on links.",
        xcursor: &["pointer", "hand2", "hand1", "pointing_hand"],
    },
    Role {
        win: "Pin",
        label: "Location Select",
        hint: "Pen/touch location. Not present on every Windows install.",
        xcursor: &["pin"],
    },
    Role {
        win: "Person",
        label: "Person Select",
        hint: "Pen/touch person. Not present on every Windows install.",
        xcursor: &["person"],
    },
];

pub fn role(name: &str) -> Option<&'static Role> {
    ROLES.iter().find(|r| r.win.eq_ignore_ascii_case(name))
}

/// Grid description for a sprite sheet.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Grid {
    pub cols: u32,
    pub rows: u32,
    /// How many cells are actually used, reading left-to-right, top-to-bottom.
    /// Defaults to `cols * rows`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<u32>,
}

fn default_fps() -> f32 {
    20.0
}
fn default_loop() -> bool {
    true
}

/// How one cursor role is drawn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSpec {
    /// Path to the art, relative to the pack directory. A file or, for numbered
    /// frame sequences, a directory. Omitted when `inherit` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,

    /// Hotspot in `base_size` coordinates, scaled for every emitted size.
    ///
    /// Optional because a `.cur`/`.ani` source already carries its own hotspot;
    /// omitting this keeps the one baked into the file. `[0, 0]` is a real value
    /// (top-left tip), so absence has to be distinguishable from zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotspot: Option<[u32; 2]>,

    /// Target frame rate for animated sources. Quantised to 1/60s steps.
    #[serde(default = "default_fps")]
    pub fps: f32,

    #[serde(rename = "loop", default = "default_loop")]
    pub looping: bool,

    /// Present when `source` is a sprite sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<Grid>,

    /// `"system"` leaves this role at the Windows default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherit: Option<String>,

    /// For a ready-made `.cur`/`.ani` source that only carries small images:
    /// synthesize the pack's larger sizes instead of leaving Windows to
    /// stretch it at display time.
    ///
    /// Off by default, so a plain import stays a byte-for-byte copy of the
    /// source file. Ignored for drawn (SVG/PNG/GIF) sources, which already
    /// render at every requested size.
    ///
    /// Accepts the legacy `true`/`false` from older packs: `true` reads as
    /// [`UpscaleMode::Raster`].
    #[serde(default, deserialize_with = "deserialize_upscale_mode")]
    pub upscale: UpscaleMode,
}

impl Default for RoleSpec {
    fn default() -> Self {
        RoleSpec {
            source: None,
            hotspot: None,
            fps: default_fps(),
            looping: true,
            grid: None,
            inherit: None,
            upscale: UpscaleMode::Off,
        }
    }
}

/// How a ready-made source's missing larger sizes get filled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpscaleMode {
    /// Leave the source at its native size; Windows stretches it on the fly.
    #[default]
    Off,
    /// Resample the largest embedded image with the pipeline's Lanczos
    /// filter. Fast and predictable, but stays soft above the source's
    /// native size — resampling cannot invent detail that was never there.
    Raster,
    /// Experimental, not generally recommended. Traces the largest embedded
    /// image into vector shapes, then renders each missing size from that
    /// trace. Edges stay crisp at any size instead of softening, but the
    /// trace often flattens shading and fine detail into blocky regions —
    /// results are inconsistent and can look worse than [`Raster`](Self::Raster).
    Vector,
}

/// Accepts either the legacy bare `true`/`false` or the current mode string.
fn deserialize_upscale_mode<'de, D>(d: D) -> std::result::Result<UpscaleMode, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Legacy(bool),
        Mode(UpscaleMode),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::Legacy(true) => UpscaleMode::Raster,
        Raw::Legacy(false) => UpscaleMode::Off,
        Raw::Mode(m) => m,
    })
}

impl RoleSpec {
    /// True when this role should be left as the Windows default.
    pub fn is_system(&self) -> bool {
        self.source.is_none() || self.inherit.as_deref() == Some("system")
    }
}

fn default_base_size() -> u32 {
    32
}
fn default_sizes() -> Vec<u32> {
    DEFAULT_SIZES.to_vec()
}
fn default_format() -> u32 {
    1
}

/// A cursor pack.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pack {
    #[serde(default = "default_format")]
    pub format: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Coordinate space that `hotspot` values are expressed in.
    #[serde(default = "default_base_size")]
    pub base_size: u32,

    /// Sizes to emit into every cursor file.
    #[serde(default = "default_sizes")]
    pub sizes: Vec<u32>,

    /// Roles, keyed by Windows role name.
    #[serde(default)]
    pub roles: BTreeMap<String, RoleSpec>,
}

impl Pack {
    pub fn load(dir: &Path) -> Result<Pack> {
        let path = dir.join("pack.json");
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let pack: Pack =
            serde_json::from_str(&text).map_err(|source| Error::Json { path: path.clone(), source })?;
        pack.validate()?;
        Ok(pack)
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let path = dir.join("pack.json");
        let text = serde_json::to_string_pretty(self)
            .map_err(|source| Error::Json { path: path.clone(), source })?;
        std::fs::write(&path, text).map_err(|e| Error::io(&path, e))
    }

    /// Structural checks that do not need the art files present.
    pub fn validate(&self) -> Result<()> {
        if self.roles.is_empty() || self.roles.values().all(|r| r.is_system()) {
            return Err(Error::EmptyPack);
        }
        if self.base_size == 0 {
            return Err(Error::Other("base_size must be greater than zero".into()));
        }
        if self.sizes.is_empty() {
            return Err(Error::Other("sizes must list at least one size".into()));
        }
        if let Some(&bad) = self.sizes.iter().find(|&&s| s == 0 || s > 256) {
            return Err(Error::Other(format!("size {bad} is out of range (1..=256)")));
        }

        for (name, spec) in &self.roles {
            if role(name).is_none() {
                return Err(Error::UnknownRole(name.clone()));
            }
            if spec.is_system() {
                continue;
            }
            if let Some([x, y]) = spec.hotspot {
                if x >= self.base_size || y >= self.base_size {
                    return Err(Error::HotspotOutOfBounds {
                        role: name.clone(),
                        x: x as i32,
                        y: y as i32,
                        size: self.base_size,
                        max: self.base_size - 1,
                    });
                }
            }
            if spec.fps <= 0.0 || !spec.fps.is_finite() {
                return Err(Error::Other(format!(
                    "role '{name}' has an invalid fps of {}",
                    spec.fps
                )));
            }
            if let Some(g) = &spec.grid {
                if g.cols == 0 || g.rows == 0 {
                    return Err(Error::Other(format!(
                        "role '{name}': sprite sheet grid must have non-zero cols and rows"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Scale a hotspot from `base_size` space to an emitted size.
    ///
    /// Clamped so a hotspot can never land outside the canvas after rounding.
    pub fn scaled_hotspot(&self, spec: &RoleSpec, size: u32) -> (u16, u16) {
        let [hx, hy] = spec.hotspot.unwrap_or([0, 0]);
        let scale = size as f64 / self.base_size as f64;
        let sx = (hx as f64 * scale).round() as u32;
        let sy = (hy as f64 * scale).round() as u32;
        (
            sx.min(size.saturating_sub(1)) as u16,
            sy.min(size.saturating_sub(1)) as u16,
        )
    }

    /// Roles that will actually be written, in the canonical order.
    pub fn active_roles(&self) -> Vec<(&str, &RoleSpec)> {
        ROLES
            .iter()
            .filter_map(|r| {
                self.roles
                    .get(r.win)
                    .filter(|s| !s.is_system())
                    .map(|s| (r.win, s))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack_with(hotspot: [u32; 2], base: u32) -> Pack {
        let mut roles = BTreeMap::new();
        roles.insert(
            "Arrow".into(),
            RoleSpec {
                source: Some("a.svg".into()),
                hotspot: Some(hotspot),
                ..Default::default()
            },
        );
        Pack {
            format: 1,
            name: "t".into(),
            author: None,
            version: None,
            description: None,
            base_size: base,
            sizes: DEFAULT_SIZES.to_vec(),
            roles,
        }
    }

    /// A hotspot authored at 32px must land in the right place at every size,
    /// or clicks register somewhere other than the tip of the cursor.
    #[test]
    fn hotspot_scales_with_size() {
        let p = pack_with([3, 2], 32);
        let spec = &p.roles["Arrow"];
        assert_eq!(p.scaled_hotspot(spec, 32), (3, 2));
        assert_eq!(p.scaled_hotspot(spec, 64), (6, 4));
        assert_eq!(p.scaled_hotspot(spec, 128), (12, 8));
        assert_eq!(p.scaled_hotspot(spec, 48), (5, 3)); // 4.5 -> 5, 3.0 -> 3
        assert_eq!(p.scaled_hotspot(spec, 96), (9, 6));
    }

    /// Rounding must never push the hotspot off the canvas.
    #[test]
    fn hotspot_stays_inside_the_canvas() {
        let p = pack_with([31, 31], 32);
        let spec = &p.roles["Arrow"];
        for &s in DEFAULT_SIZES {
            let (x, y) = p.scaled_hotspot(spec, s);
            assert!((x as u32) < s && (y as u32) < s, "{s}px gave {x},{y}");
        }
    }

    #[test]
    fn rejects_hotspot_outside_base_canvas() {
        let err = pack_with([32, 0], 32).validate().unwrap_err();
        assert!(matches!(err, Error::HotspotOutOfBounds { .. }), "{err}");
    }

    #[test]
    fn rejects_unknown_role() {
        let mut p = pack_with([0, 0], 32);
        p.roles.insert("Sparkle".into(), RoleSpec {
            source: Some("x.svg".into()),
            ..Default::default()
        });
        assert!(matches!(p.validate(), Err(Error::UnknownRole(_))));
    }

    #[test]
    fn rejects_pack_that_changes_nothing() {
        let mut p = pack_with([0, 0], 32);
        p.roles.get_mut("Arrow").unwrap().inherit = Some("system".into());
        assert!(matches!(p.validate(), Err(Error::EmptyPack)));

        p.roles.clear();
        assert!(matches!(p.validate(), Err(Error::EmptyPack)));
    }

    #[test]
    fn active_roles_follow_canonical_order_not_alphabetical() {
        let mut p = pack_with([0, 0], 32);
        for r in ["Hand", "Wait", "IBeam"] {
            p.roles.insert(r.into(), RoleSpec {
                source: Some("x.svg".into()),
                ..Default::default()
            });
        }
        let order: Vec<&str> = p.active_roles().iter().map(|(n, _)| *n).collect();
        // Canonical order puts Arrow, Wait, IBeam before Hand.
        assert_eq!(order, vec!["Arrow", "Wait", "IBeam", "Hand"]);
    }

    #[test]
    fn manifest_survives_a_save_load_cycle() {
        let p = pack_with([3, 2], 32);
        let json = serde_json::to_string_pretty(&p).unwrap();
        let back: Pack = serde_json::from_str(&json).unwrap();
        back.validate().unwrap();
        assert_eq!(back.name, p.name);
        assert_eq!(back.roles["Arrow"].hotspot, Some([3, 2]));
        assert_eq!(back.sizes, DEFAULT_SIZES);
    }

    /// Older packs on disk still say `"upscale": true`/`false`. Those must
    /// keep loading exactly as they did before `UpscaleMode` existed.
    #[test]
    fn legacy_bool_upscale_still_loads() {
        let json = r#"{"name":"x","roles":{"Arrow":{"source":"a.cur","upscale":true}}}"#;
        let p: Pack = serde_json::from_str(json).unwrap();
        assert_eq!(p.roles["Arrow"].upscale, UpscaleMode::Raster);

        let json = r#"{"name":"x","roles":{"Arrow":{"source":"a.cur","upscale":false}}}"#;
        let p: Pack = serde_json::from_str(json).unwrap();
        assert_eq!(p.roles["Arrow"].upscale, UpscaleMode::Off);
    }

    #[test]
    fn vector_upscale_mode_round_trips() {
        let json = r#"{"name":"x","roles":{"Arrow":{"source":"a.cur","upscale":"vector"}}}"#;
        let p: Pack = serde_json::from_str(json).unwrap();
        assert_eq!(p.roles["Arrow"].upscale, UpscaleMode::Vector);

        let back: Pack = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.roles["Arrow"].upscale, UpscaleMode::Vector);
    }

    /// Typos in pack.json should be reported, not silently ignored.
    #[test]
    fn unknown_manifest_fields_are_rejected() {
        let json = r#"{"name":"x","roles":{},"colour":"red"}"#;
        assert!(serde_json::from_str::<Pack>(json).is_err());
    }

    #[test]
    fn every_role_has_a_unique_name_and_xcursor_mapping() {
        let mut seen = std::collections::HashSet::new();
        for r in ROLES {
            assert!(seen.insert(r.win), "duplicate role {}", r.win);
            assert!(!r.xcursor.is_empty(), "{} has no xcursor names", r.win);
        }
        assert!(role("arrow").is_some(), "lookup should be case-insensitive");
    }
}
