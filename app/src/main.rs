//! CustoMouse — a small tray app for installing and switching cursor packs.
//!
//! Launched with `--tray` (which is how autostart runs it) the window stays
//! hidden and only the tray icon appears.

// Release builds are a GUI app, so no console window flashes up on launch.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod packs;
mod preview;
#[cfg(windows)]
mod tray;

/// The app icon: the inverted Windows pointer with a bold white plus, generated
/// alongside the bundled pack by `cargo run -p cursorpack --example gen_inverted`.
///
/// Baked in as a PNG rather than read from `C:\Windows` at run time, so the icon
/// is identical everywhere and does not depend on the host machine.
const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");
const ICON_NATIVE: u32 = 256;

/// Render the app icon at `size`, as RGBA. Falls back to a transparent square if
/// decoding ever fails, because an icon is never worth crashing over.
pub fn render_app_icon(size: u32) -> Vec<u8> {
    let blank = || vec![0u8; (size * size * 4) as usize];
    let Ok(img) = image::load_from_memory(ICON_PNG) else {
        return blank();
    };
    let rgba = img.to_rgba8();
    if rgba.width() != ICON_NATIVE {
        return blank();
    }
    if size == ICON_NATIVE {
        return rgba.into_raw();
    }
    cursorpack::raster::resize_rgba(&rgba.into_raw(), ICON_NATIVE, size).unwrap_or_else(|_| blank())
}

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let start_hidden = std::env::args().any(|a| a == "--tray");

    let icon = {
        let rgba = render_app_icon(64);
        egui::IconData {
            rgba,
            width: 64,
            height: 64,
        }
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("CustoMouse")
            .with_inner_size([1180.0, 900.0])
            .with_min_inner_size([760.0, 480.0])
            .with_icon(icon)
            .with_visible(!start_hidden),
        ..Default::default()
    };

    eframe::run_native(
        "CustoMouse",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, start_hidden)))),
    )
}
