//! Turning a compiled pack into animated textures for the preview pane.

use cursorpack::build::Built;
use cursorpack::{ani, ico};
use egui::{ColorImage, TextureHandle, TextureOptions};

/// One role, ready to draw.
pub struct RolePreview {
    pub role: String,
    pub label: &'static str,
    pub hint: &'static str,
    pub frames: Vec<TextureHandle>,
    pub animated: bool,
    /// How long each frame is shown, in seconds.
    pub frame_secs: f32,
    /// Hotspot in the previewed image's pixel space.
    pub hotspot: (f32, f32),
    pub preview_px: u32,
    pub sizes: Vec<u32>,
    pub frame_count: usize,
}

impl RolePreview {
    /// Which frame to show at the given elapsed time.
    pub fn frame_at(&self, secs: f64) -> &TextureHandle {
        if self.frames.len() <= 1 || self.frame_secs <= 0.0 {
            return &self.frames[0];
        }
        let i = (secs / self.frame_secs as f64) as usize % self.frames.len();
        &self.frames[i]
    }
}

fn to_color_image(img: &ico::Image) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.size as usize, img.size as usize], &img.rgba)
}

/// Pick the largest embedded image, so the preview is as crisp as the pack allows.
fn largest<'a>(images: &'a [ico::Image]) -> &'a ico::Image {
    images.iter().max_by_key(|i| i.size).expect("at least one image")
}

/// Build previews for every role in a compiled pack.
pub fn build(ctx: &egui::Context, built: &Built) -> Vec<RolePreview> {
    let mut out = Vec::new();

    for cursor in &built.cursors {
        let meta = cursorpack::manifest::role(&cursor.role);
        let (label, hint) = meta
            .map(|r| (r.label, r.hint))
            .unwrap_or(("Unknown", ""));

        let (frames_rgba, frame_secs): (Vec<Vec<ico::Image>>, f32) = if cursor.animated {
            match ani::decode(&cursor.data) {
                Ok(a) => {
                    let secs = a.default_rate as f32 / ani::JIFFIES_PER_SEC;
                    (a.frames, secs)
                }
                Err(_) => continue,
            }
        } else {
            match ico::decode(&cursor.data) {
                Ok(images) => (vec![images], 0.0),
                Err(_) => continue,
            }
        };

        if frames_rgba.is_empty() {
            continue;
        }

        let first = largest(&frames_rgba[0]);
        let preview_px = first.size;
        let hotspot = (first.hot_x as f32, first.hot_y as f32);

        let textures: Vec<TextureHandle> = frames_rgba
            .iter()
            .enumerate()
            .map(|(i, frame)| {
                let img = largest(frame);
                ctx.load_texture(
                    format!("{}-{i}", cursor.role),
                    to_color_image(img),
                    TextureOptions::LINEAR,
                )
            })
            .collect();

        if textures.is_empty() {
            continue;
        }

        out.push(RolePreview {
            role: cursor.role.clone(),
            label,
            hint,
            frames: textures,
            animated: cursor.animated,
            frame_secs,
            hotspot,
            preview_px,
            sizes: cursor.sizes.clone(),
            frame_count: cursor.frames,
        });
    }

    out
}

/// Small still images for a pack's gallery tile.
pub struct PackThumbs {
    pub textures: Vec<TextureHandle>,
    pub error: Option<String>,
}

/// Render one still image per role, for the gallery. Deliberately cheap: a single
/// size, first frame only, so a folder full of packs still opens instantly.
pub fn thumbs(ctx: &egui::Context, pack: &cursorpack::manifest::Pack, dir: &std::path::Path) -> PackThumbs {
    match cursorpack::build::thumbnails(pack, dir, 64) {
        Ok(items) => PackThumbs {
            textures: items
                .iter()
                .enumerate()
                .map(|(i, (role, size, rgba))| {
                    let img = ColorImage::from_rgba_unmultiplied(
                        [*size as usize, *size as usize],
                        rgba,
                    );
                    ctx.load_texture(
                        format!("thumb-{}-{role}-{i}", pack.name),
                        img,
                        TextureOptions::LINEAR,
                    )
                })
                .collect(),
            error: None,
        },
        Err(e) => PackThumbs {
            textures: Vec::new(),
            error: Some(e.to_string()),
        },
    }
}
