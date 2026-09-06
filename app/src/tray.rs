//! The tray icon and its menu.
//!
//! On Windows a new tray icon goes into the overflow ("hidden icons") flyout by
//! default, which is where a background app belongs. Users can drag it onto the
//! taskbar if they want it always visible.

use std::collections::HashMap;
use std::path::PathBuf;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

/// What the user picked in the tray.
#[derive(Debug, Clone)]
pub enum TrayCommand {
    Open,
    RestoreDefault,
    Quit,
    ApplyPack(PathBuf),
}

pub struct Tray {
    _icon: TrayIcon,
    actions: HashMap<MenuId, TrayCommand>,
}

impl Tray {
    pub fn new(packs: &[(String, PathBuf)], active: Option<&str>) -> anyhow::Result<Self> {
        let menu = Menu::new();
        let mut actions = HashMap::new();

        let open = MenuItem::new("Open CustoMouse", true, None);
        actions.insert(open.id().clone(), TrayCommand::Open);
        menu.append(&open)?;
        menu.append(&PredefinedMenuItem::separator())?;

        if packs.is_empty() {
            let none = MenuItem::new("No packs found", false, None);
            menu.append(&none)?;
        } else {
            for (name, dir) in packs {
                // A leading dot marks the pack that is currently applied; tray menus
                // have no reliable check-mark styling across Windows versions.
                let label = if active == Some(name.as_str()) {
                    format!("• {name}")
                } else {
                    format!("   {name}")
                };
                let item = MenuItem::new(label, true, None);
                actions.insert(item.id().clone(), TrayCommand::ApplyPack(dir.clone()));
                menu.append(&item)?;
            }
        }

        menu.append(&PredefinedMenuItem::separator())?;

        // Deliberately top level and always enabled: getting back to the Windows
        // default must never be something the user has to hunt for.
        let restore = MenuItem::new("Restore Windows Default", true, None);
        actions.insert(restore.id().clone(), TrayCommand::RestoreDefault);
        menu.append(&restore)?;

        menu.append(&PredefinedMenuItem::separator())?;
        let quit = MenuItem::new("Quit", true, None);
        actions.insert(quit.id().clone(), TrayCommand::Quit);
        menu.append(&quit)?;

        let tooltip = match active {
            Some(name) => format!("CustoMouse — {name}"),
            None => "CustoMouse — Windows default".to_string(),
        };

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(tooltip)
            .with_icon(tray_image()?)
            .build()?;

        Ok(Tray {
            _icon: icon,
            actions,
        })
    }

    /// Drain pending tray and menu events.
    pub fn poll(&self) -> Vec<TrayCommand> {
        let mut out = Vec::new();

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Some(cmd) = self.actions.get(&event.id) {
                out.push(cmd.clone());
            }
        }

        // A left click on the icon opens the window, which is what people expect.
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = event {
                out.push(TrayCommand::Open);
            }
        }

        out
    }
}

/// Render the app icon from the embedded SVG, so there is no image file to ship
/// and the icon is crisp at whatever size the shell asks for.
fn tray_image() -> anyhow::Result<tray_icon::Icon> {
    let rgba = crate::render_app_icon(32);
    tray_icon::Icon::from_rgba(rgba, 32, 32).map_err(Into::into)
}
