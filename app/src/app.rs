//! The window, the tray wiring, and the actions the user can take.

use crate::packs::{self, FoundPack};
use crate::preview::{self, RolePreview};
use egui::{Color32, RichText, ViewportCommand};
use std::path::PathBuf;
use std::time::Instant;

#[cfg(windows)]
use crate::tray::{Tray, TrayCommand};
#[cfg(windows)]
use winapply::apply::{self as engine, State};

/// Actions collected while drawing, then run once the UI borrow has ended.
enum Action {
    Apply(usize),
    RestoreDefault,
    Rescan,
    OpenPacksFolder,
    ImportFolder,
    OpenRestoreFolder,
    RunRestoreScript,
    DeletePack(usize),
    ConfirmDelete,
    CancelDelete,
    FixLowResolution(cursorpack::manifest::UpscaleMode),
    ConfirmQuit { restore: bool },
    CancelQuit,
}

#[derive(PartialEq)]
enum Tone {
    Ok,
    Warn,
    Bad,
}

/// Maximum pointers shown per row in the "every pointer in this pack" strip.
const DETAILS_COLS: usize = 6;
/// Height budgeted per row there: the chip's own box (see `role_chip`) plus its
/// two lines of caption text and row spacing.
const DETAILS_ROW_H: f32 = 68.0 + 2.0 + 11.0 + 11.0 + 6.0;

pub struct App {
    packs: Vec<FoundPack>,
    problems: Vec<String>,
    selected: Option<usize>,

    preview: Vec<RolePreview>,
    preview_for: Option<PathBuf>,
    /// Gallery thumbnails, built lazily one pack per frame so a big packs
    /// folder never stalls the first paint.
    thumbs: std::collections::HashMap<PathBuf, crate::preview::PackThumbs>,
    preview_warnings: Vec<cursorpack::raster::Warning>,
    preview_error: Option<String>,

    status: Option<(String, Tone)>,
    started: Instant,
    show_help: bool,
    quit_prompt: bool,
    /// Index of the pack awaiting delete confirmation.
    delete_prompt: Option<usize>,
    really_quit: bool,
    hidden_hint_shown: bool,
    /// While true, the window is forced hidden every pass.
    ///
    /// `ViewportBuilder::with_visible(false)` alone does not hold: eframe shows
    /// the window once it paints its first frame, so a `--tray` launch flashed up
    /// a real, taskbar-listed window. Re-asserting it each pass keeps it away
    /// until the user actually opens it from the tray.
    keep_hidden: bool,

    #[cfg(windows)]
    state: State,
    #[cfg(windows)]
    tray: Option<Tray>,
    #[cfg(windows)]
    tray_stale: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, start_hidden: bool) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        let (packs, problems) = packs::discover();

        #[cfg(windows)]
        let state = engine::reconcile_state();

        let mut app = App {
            selected: (!packs.is_empty()).then_some(0),
            packs,
            problems,
            preview: Vec::new(),
            preview_for: None,
            thumbs: std::collections::HashMap::new(),
            preview_warnings: Vec::new(),
            preview_error: None,
            status: None,
            started: Instant::now(),
            show_help: false,
            quit_prompt: false,
            delete_prompt: None,
            really_quit: false,
            hidden_hint_shown: start_hidden,
            keep_hidden: start_hidden,
            #[cfg(windows)]
            state,
            #[cfg(windows)]
            tray: None,
            #[cfg(windows)]
            tray_stale: true,
        };

        // A fresh install defaults both switches on. Register autostart for real
        // so the checked box reflects reality, then persist so the value is the
        // user's own choice from here on.
        #[cfg(windows)]
        if !engine::state_exists() {
            if let Err(e) = set_autostart(app.state.start_with_windows) {
                log::warn!("could not register autostart on first run: {e}");
                app.state.start_with_windows = false;
            }
            if let Err(e) = engine::save_state(&app.state) {
                log::warn!("could not save initial settings: {e}");
            }
        }

        // Put the user's chosen pack back. With "restore on exit" on, quitting
        // reverts to the Windows defaults, so without this a reboot would silently
        // lose the pointer they picked.
        #[cfg(windows)]
        match engine::reapply_last_pack() {
            Ok(Some(name)) => {
                app.state = engine::load_state();
                app.say(format!("Reapplied \"{name}\"."), Tone::Ok);
            }
            Ok(None) => {}
            Err(e) => {
                log::warn!("could not reapply the last pack: {e}");
                app.say(format!("Could not reapply the last pack: {e}"), Tone::Warn);
            }
        }

        // Prefer showing whatever is currently applied.
        #[cfg(windows)]
        if let Some(dir) = app.state.active_pack_dir.clone() {
            if let Some(i) = app.packs.iter().position(|p| p.dir == dir) {
                app.selected = Some(i);
            }
        }

        app
    }

    // ------------------------------------------------------------ data

    fn rescan(&mut self) {
        let previous = self.selected.and_then(|i| self.packs.get(i)).map(|p| p.dir.clone());
        let (packs, problems) = packs::discover();
        self.packs = packs;
        self.problems = problems;
        self.selected = previous
            .and_then(|dir| self.packs.iter().position(|p| p.dir == dir))
            .or_else(|| (!self.packs.is_empty()).then_some(0));
        self.preview_for = None;
        self.thumbs.clear();
        self.say(format!("Found {} pack(s).", self.packs.len()), Tone::Ok);
    }

    /// Compile the selected pack for preview. Cheap enough to do on selection.
    fn refresh_preview(&mut self, ctx: &egui::Context) {
        let Some(pack) = self.selected.and_then(|i| self.packs.get(i)) else {
            self.preview.clear();
            self.preview_for = None;
            return;
        };
        if self.preview_for.as_ref() == Some(&pack.dir) {
            return;
        }

        self.preview.clear();
        self.preview_warnings.clear();
        self.preview_error = None;
        self.preview_for = Some(pack.dir.clone());

        match cursorpack::build::build(&pack.pack, &pack.dir) {
            Ok(built) => {
                self.preview_warnings = built.warnings.clone();
                self.preview = preview::build(ctx, &built);
            }
            Err(e) => self.preview_error = Some(e.to_string()),
        }
    }

    /// Build at most one pack's thumbnails per frame, so the window stays
    /// responsive no matter how many packs are installed.
    fn refresh_thumbs(&mut self, ctx: &egui::Context) {
        let next = self
            .packs
            .iter()
            .find(|p| !self.thumbs.contains_key(&p.dir))
            .map(|p| (p.dir.clone(), p.pack.clone()));

        if let Some((dir, pack)) = next {
            let t = crate::preview::thumbs(ctx, &pack, &dir);
            self.thumbs.insert(dir, t);
            ctx.request_repaint();
        }
    }

    /// Turn a folder of ready-made `.cur` / `.ani` files into a pack.
    ///
    /// This is the path for cursor sets downloaded from the web. Nothing is
    /// converted: the files are already the format Windows wants. The role mapping
    /// comes from a `.crs` scheme file or `install.inf` when the pack ships one,
    /// and only falls back to guessing from file names otherwise.
    fn import_folder(&mut self) {
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Choose a folder containing .cur / .ani files")
            .pick_folder()
        else {
            return;
        };

        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Imported Pack".to_string());

        let import = match cursorpack::import::from_folder(&dir, &name) {
            Ok(i) => i,
            Err(e) => {
                self.say(format!("Could not import: {e}"), Tone::Bad);
                return;
            }
        };

        let dest = packs::user_packs_dir().join(sanitize(&name));
        if let Err(e) = import.materialize(&dest) {
            self.say(format!("Could not write the imported pack: {e}"), Tone::Bad);
            return;
        }

        let mut msg = format!(
            "Imported \"{name}\" — {} of 17 pointer(s) mapped from {}.",
            import.mappings.len(),
            import.source.label()
        );
        if !import.unmatched.is_empty() {
            msg.push_str(&format!(
                " {} file(s) could not be matched to a role and were skipped.",
                import.unmatched.len()
            ));
        }

        self.rescan();
        if let Some(i) = self.packs.iter().position(|p| p.dir == dest) {
            self.selected = Some(i);
        }
        self.say(msg, if import.unmatched.is_empty() { Tone::Ok } else { Tone::Warn });
    }

    /// Delete an imported pack: its compiled cursors and its copied source files.
    fn delete_pack(&mut self, index: usize) {
        let Some(found) = self.packs.get(index) else {
            return;
        };
        if found.builtin {
            self.say("Built-in packs cannot be deleted.", Tone::Warn);
            return;
        }

        let dir = found.dir.clone();
        let name = found.pack.name.clone();

        // Only ever delete inside our own packs folder. A pack.json discovered
        // somewhere else must never cause us to remove that directory.
        let root = packs::user_packs_dir();
        let inside = dir
            .canonicalize()
            .ok()
            .zip(root.canonicalize().ok())
            .is_some_and(|(d, r)| d.starts_with(r));
        if !inside {
            self.say(
                format!("Refusing to delete {} - it is outside the packs folder.", dir.display()),
                Tone::Bad,
            );
            return;
        }

        #[cfg(windows)]
        match engine::delete_pack(&dir, &name) {
            Ok(()) => {
                self.state = engine::load_state();
                self.tray_stale = true;
                self.rescan();
                self.say(format!("Deleted {name}."), Tone::Ok);
            }
            Err(e) => self.say(format!("Could not delete {name}: {e}"), Tone::Bad),
        }
        #[cfg(not(windows))]
        {
            let _ = name;
            self.say("Deleting packs is only implemented on Windows so far.", Tone::Warn);
        }
    }

    /// Turn on `upscale` (in the given mode) for every role in the selected
    /// pack that is currently stuck at a low native resolution, then rebuild
    /// the preview so the effect is visible immediately.
    ///
    /// This is the one-click answer to "how do I fix that low-res warning":
    /// each affected role gets its larger sizes filled in — either resampled
    /// from its largest embedded image, or traced into vector shapes first —
    /// instead of leaving Windows to stretch it live.
    fn fix_low_resolution(&mut self, mode: cursorpack::manifest::UpscaleMode) {
        let Some(found) = self.selected.and_then(|i| self.packs.get(i)) else {
            return;
        };
        if found.builtin {
            self.say(
                "Built-in packs are not meant to be edited. Import a copy to change it.",
                Tone::Warn,
            );
            return;
        }

        let roles: Vec<String> = self
            .preview_warnings
            .iter()
            .filter_map(|w| match w {
                cursorpack::raster::Warning::LowResolutionSource { role, .. } => {
                    Some(role.clone())
                }
                _ => None,
            })
            .collect();
        if roles.is_empty() {
            return;
        }

        let dir = found.dir.clone();
        let mut pack = match cursorpack::manifest::Pack::load(&dir) {
            Ok(p) => p,
            Err(e) => {
                self.say(format!("Could not read pack.json: {e}"), Tone::Bad);
                return;
            }
        };

        let mut changed = 0;
        for role in &roles {
            if let Some(spec) = pack.roles.get_mut(role) {
                spec.upscale = mode;
                changed += 1;
            }
        }
        if let Err(e) = pack.save(&dir) {
            self.say(format!("Could not save pack.json: {e}"), Tone::Bad);
            return;
        }

        // Force the preview (and, if this pack is applied, the next apply) to
        // pick up the change.
        self.preview_for = None;
        self.thumbs.remove(&dir);
        let via = match mode {
            cursorpack::manifest::UpscaleMode::Vector => "vector tracing, experimental",
            _ => "resampling",
        };
        self.say(
            format!(
                "Upscaling ({via}) turned on for {changed} pointer(s) in \"{}\".",
                pack.name
            ),
            Tone::Ok,
        );
    }

    fn say(&mut self, msg: impl Into<String>, tone: Tone) {
        self.status = Some((msg.into(), tone));
    }

    // ------------------------------------------------------------ actions

    #[cfg(windows)]
    fn apply_pack(&mut self, dir: PathBuf) {
        match engine::apply(&dir) {
            Ok(applied) => {
                let mut msg = format!(
                    "Applied \"{}\" — {} cursor(s) set.",
                    applied.pack_name,
                    applied.roles.len()
                );
                if !applied.reset_to_default.is_empty() {
                    msg.push_str(&format!(
                        " {} role(s) left at the Windows default.",
                        applied.reset_to_default.len()
                    ));
                }
                self.state = engine::load_state();
                self.tray_stale = true;
                self.say(msg, Tone::Ok);
            }
            Err(e) => self.say(format!("Could not apply: {e}"), Tone::Bad),
        }
    }

    #[cfg(windows)]
    fn restore_default(&mut self) {
        match engine::restore_defaults() {
            Ok(()) => {
                self.state = engine::load_state();
                self.tray_stale = true;
                self.say("Windows default cursors restored.", Tone::Ok);
            }
            Err(winapply::Error::NoRestorePoint) => {
                self.say(
                    "Nothing to restore — no pack has been applied yet, so your cursors are already the Windows defaults.",
                    Tone::Warn,
                );
            }
            Err(e) => self.say(format!("Could not restore: {e}"), Tone::Bad),
        }
    }

    #[cfg(not(windows))]
    fn apply_pack(&mut self, _dir: PathBuf) {
        self.say("Applying cursors is only implemented on Windows so far.", Tone::Warn);
    }

    #[cfg(not(windows))]
    fn restore_default(&mut self) {
        self.say("Restoring is only implemented on Windows so far.", Tone::Warn);
    }

    /// Restore on the way out, keeping the remembered pack so the next launch
    /// puts it back. Quitting is not the same as saying "I want Windows cursors".
    fn restore_default_leaving(&mut self) {
        #[cfg(windows)]
        if let Err(e) = engine::restore_defaults_for_exit() {
            log::warn!("could not restore on exit: {e}");
        }
    }

    fn run(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Apply(i) => {
                if let Some(p) = self.packs.get(i) {
                    let dir = p.dir.clone();
                    self.apply_pack(dir);
                }
            }
            Action::RestoreDefault => self.restore_default(),
            Action::DeletePack(i) => self.delete_prompt = Some(i),
            Action::CancelDelete => self.delete_prompt = None,
            Action::ConfirmDelete => {
                if let Some(i) = self.delete_prompt.take() {
                    self.delete_pack(i);
                }
            }
            Action::FixLowResolution(mode) => self.fix_low_resolution(mode),
            Action::Rescan => self.rescan(),
            Action::OpenPacksFolder => packs::open_folder(&packs::user_packs_dir()),
            Action::ImportFolder => self.import_folder(),
            Action::OpenRestoreFolder => {
                #[cfg(windows)]
                packs::open_folder(&winapply::restore::data_root());
            }
            Action::RunRestoreScript => {
                #[cfg(windows)]
                {
                    let script = winapply::restore::script_path();
                    if script.is_file() {
                        let _ = std::process::Command::new("cmd.exe")
                            .args(["/C", "start", "", &script.to_string_lossy()])
                            .spawn();
                    } else {
                        self.say(
                            "No restore script yet — it is written the first time a pack is applied.",
                            Tone::Warn,
                        );
                    }
                }
            }
            Action::ConfirmQuit { restore } => {
                if restore {
                    self.restore_default_leaving();
                }
                self.quit_prompt = false;
                self.really_quit = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            Action::CancelQuit => self.quit_prompt = false,
        }
    }

    fn active_pack_name(&self) -> Option<String> {
        #[cfg(windows)]
        {
            self.state.active_pack.clone()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    fn request_quit(&mut self, ctx: &egui::Context) {
        let has_pack = self.active_pack_name().is_some();
        #[cfg(windows)]
        let auto_restore = self.state.restore_on_exit;
        #[cfg(not(windows))]
        let auto_restore = false;

        if has_pack && !auto_restore {
            // Never exit leaving cursors changed without saying so.
            self.open_window(ctx);
            self.quit_prompt = true;
        } else {
            if auto_restore {
                self.restore_default_leaving();
            }
            self.really_quit = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    // ------------------------------------------------------------ tray

    #[cfg(windows)]
    fn sync_tray(&mut self) {
        if !self.tray_stale {
            return;
        }
        self.tray_stale = false;

        let entries: Vec<(String, PathBuf)> = self
            .packs
            .iter()
            .map(|p| (p.pack.name.clone(), p.dir.clone()))
            .collect();

        match Tray::new(&entries, self.state.active_pack.as_deref()) {
            Ok(t) => self.tray = Some(t),
            Err(e) => log::warn!("could not build tray icon: {e}"),
        }
    }

    #[cfg(windows)]
    fn pump_tray(&mut self, ctx: &egui::Context) {
        let commands = match &self.tray {
            Some(t) => t.poll(),
            None => Vec::new(),
        };
        for cmd in commands {
            match cmd {
                TrayCommand::Open => self.open_window(ctx),
                TrayCommand::RestoreDefault => self.restore_default(),
                TrayCommand::Quit => self.request_quit(ctx),
                TrayCommand::ApplyPack(dir) => {
                    self.apply_pack(dir.clone());
                    if let Some(i) = self.packs.iter().position(|p| p.dir == dir) {
                        self.selected = Some(i);
                    }
                }
            }
        }
    }
}

fn show_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(ViewportCommand::Focus);
}

impl App {
    /// Reveal the window and stop forcing it hidden.
    fn open_window(&mut self, ctx: &egui::Context) {
        self.keep_hidden = false;
        show_window(ctx);
    }
}

impl eframe::App for App {
    /// Runs even while the window is hidden, which is what lets the tray keep
    /// working with no visible window and no egui pass.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Assert this before anything else, so a --tray launch never flashes a
        // window while the tray is still being built.
        if self.keep_hidden {
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }

        #[cfg(windows)]
        {
            self.sync_tray();
            self.pump_tray(ctx);
        }

        // Closing the window hides to the tray rather than quitting.
        if ctx.input(|i| i.viewport().close_requested()) && !self.really_quit {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            self.keep_hidden = true;
            self.hidden_hint_shown = true;
        }

        // Keep polling the tray at a low rate; negligible cost while hidden.
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.refresh_thumbs(&ctx);
        self.refresh_preview(&ctx);

        let mut actions: Vec<Action> = Vec::new();

        self.header(ui, &mut actions);
        self.footer(ui, &mut actions);
        self.sidebar(ui, &mut actions);
        self.central(ui, &mut actions);

        if self.quit_prompt {
            self.quit_modal(&ctx, &mut actions);
        }
        if self.delete_prompt.is_some() {
            self.delete_modal(&ctx, &mut actions);
        }
        if self.show_help {
            self.help_window(&ctx);
        }

        for action in actions {
            self.run(action, &ctx);
        }
    }
}

// ---------------------------------------------------------------- panels

impl App {
    fn header(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::top("header").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("CustoMouse").size(20.0).strong());
                ui.add_space(12.0);

                match self.active_pack_name() {
                    Some(name) => {
                        ui.label(RichText::new("Active:").weak());
                        ui.label(RichText::new(name).strong().color(Color32::LIGHT_GREEN));
                    }
                    None => {
                        ui.label(RichText::new("Using the Windows default cursors").weak());
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Always available, never buried: this is the way out.
                    let btn = egui::Button::new(RichText::new("Restore Windows Default").strong())
                        .fill(Color32::from_rgb(120, 40, 40));
                    if ui
                        .add(btn)
                        .on_hover_text(
                            "Put the original Windows cursors back immediately.\n\
                             Always available, even if no pack is applied.",
                        )
                        .clicked()
                    {
                        actions.push(Action::RestoreDefault);
                    }
                    if ui.button("Help").clicked() {
                        self.show_help = true;
                    }
                });
            });
            ui.add_space(6.0);
        });
    }

    fn footer(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::top("status").show(ui, |ui| {
            ui.add_space(4.0);
            if let Some((msg, tone)) = &self.status {
                let color = match tone {
                    Tone::Ok => Color32::LIGHT_GREEN,
                    Tone::Warn => Color32::from_rgb(230, 190, 90),
                    Tone::Bad => Color32::from_rgb(240, 120, 120),
                };
                ui.label(RichText::new(msg).color(color));
            } else if self.hidden_hint_shown {
                ui.label(
                    RichText::new(
                        "Closing this window keeps CustoMouse running in the hidden-icons tray.",
                    )
                    .weak(),
                );
            } else {
                ui.label(RichText::new("Ready.").weak());
            }

            for line in collapse_warnings(&self.preview_warnings) {
                ui.label(
                    RichText::new(format!("• {line}"))
                        .color(Color32::from_rgb(210, 180, 100))
                        .small(),
                );
            }

            // Low resolution is the one warning with a one-click fix: upscale
            // the affected roles instead of leaving Windows to stretch them.
            let can_fix = self
                .selected
                .and_then(|i| self.packs.get(i))
                .is_some_and(|p| !p.builtin)
                && self.preview_warnings.iter().any(|w| {
                    matches!(w, cursorpack::raster::Warning::LowResolutionSource { .. })
                });
            if can_fix {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Fix the low-res pointer(s) in this pack:")
                            .small()
                            .weak(),
                    );
                    if ui
                        .small_button("Resample")
                        .on_hover_text(
                            "Fills in the larger sizes by resampling the pointer's largest \
                             embedded image with a high-quality filter. Fast and predictable, \
                             but still soft above its native size — resampling cannot invent \
                             detail that was never there.",
                        )
                        .clicked()
                    {
                        actions.push(Action::FixLowResolution(
                            cursorpack::manifest::UpscaleMode::Raster,
                        ));
                    }
                    if ui
                        .small_button("Vector trace (experimental)")
                        .on_hover_text(
                            "Experimental, not generally recommended. Traces the pointer's \
                             largest embedded image into vector shapes, then renders each \
                             larger size from that trace, so edges stay crisp instead of \
                             getting soft — but the trace often flattens shading and fine \
                             detail into blocky regions, so results are inconsistent and can \
                             look worse than a plain resample. Works best, if at all, on \
                             simple, flat-color art. Try Resample first.",
                        )
                        .clicked()
                    {
                        actions.push(Action::FixLowResolution(
                            cursorpack::manifest::UpscaleMode::Vector,
                        ));
                    }
                });
            }

            for p in &self.problems {
                ui.label(RichText::new(format!("• {p}")).color(Color32::from_rgb(240, 120, 120)).small());
            }
            ui.add_space(4.0);
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::Panel::left("packs")
            .exact_size(250.0)
            .show(ui, |ui| {
                ui.add_space(8.0);
                if ui
                    .add(
                        egui::Button::new("Import .cur / .ani folder...")
                            .min_size(egui::vec2(ui.available_width(), 26.0)),
                    )
                    .on_hover_text(
                        "For cursor sets downloaded from the web. Point at the folder                          holding the .cur / .ani files and they are used as they are -                          no conversion, nothing redrawn.",
                    )
                    .clicked()
                {
                    actions.push(Action::ImportFolder);
                }
                ui.horizontal(|ui| {
                    if ui.button("Rescan").clicked() {
                        actions.push(Action::Rescan);
                    }
                    if ui.button("Open packs folder").clicked() {
                        actions.push(Action::OpenPacksFolder);
                    }
                });

                ui.separator();
                ui.label(RichText::new("Settings").strong());
                self.settings(ui, actions);

                ui.separator();
                ui.label(RichText::new("Getting back to normal").strong());
                ui.label(
                    RichText::new(
                        "Your original cursor settings are saved before the first change, \
                         and a Restore-Default-Cursors.cmd script is written next to them. \
                         That script works even if this app is closed or removed.",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Open backup folder").clicked() {
                        actions.push(Action::OpenRestoreFolder);
                    }
                    if ui.button("Run restore script").clicked() {
                        actions.push(Action::RunRestoreScript);
                    }
                });
            });
    }

    fn settings(&mut self, ui: &mut egui::Ui, _actions: &mut [Action]) {
        #[cfg(windows)]
        {
            let mut changed = false;

            let mut restore_on_exit = self.state.restore_on_exit;
            if ui
                .checkbox(
                    &mut restore_on_exit,
                    "Restore Windows default when I quit",
                )
                .on_hover_text("Leaves no trace when you close the app.")
                .changed()
            {
                self.state.restore_on_exit = restore_on_exit;
                changed = true;
            }

            let mut autostart = self.state.start_with_windows;
            if ui
                .checkbox(&mut autostart, "Start with Windows (hidden in tray)")
                .on_hover_text("Adds a per-user startup entry. No admin rights needed.")
                .changed()
            {
                match set_autostart(autostart) {
                    Ok(()) => {
                        self.state.start_with_windows = autostart;
                        changed = true;
                    }
                    Err(e) => self.say(format!("Could not change startup setting: {e}"), Tone::Bad),
                }
            }

            if changed {
                if let Err(e) = engine::save_state(&self.state) {
                    self.say(format!("Could not save settings: {e}"), Tone::Bad);
                }
            }
        }
        #[cfg(not(windows))]
        {
            ui.label(RichText::new("Settings are Windows-only for now.").weak());
        }
    }

    fn central(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Cursor packs").size(16.0).strong());
                ui.label(RichText::new(format!("{} found", self.packs.len())).weak().small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ready = self.selected.is_some() && self.preview_error.is_none();
                    if ui
                        .add_enabled(
                            ready,
                            egui::Button::new(RichText::new("Apply selected pack").strong()),
                        )
                        .clicked()
                    {
                        if let Some(i) = self.selected {
                            actions.push(Action::Apply(i));
                        }
                    }

                    let deletable = self
                        .selected
                        .and_then(|i| self.packs.get(i))
                        .is_some_and(|p| !p.builtin);
                    if ui
                        .add_enabled(deletable, egui::Button::new("Delete"))
                        .on_hover_text("Remove this imported pack and the files it copied in.")
                        .on_disabled_hover_text("Built-in packs cannot be deleted.")
                        .clicked()
                    {
                        if let Some(i) = self.selected {
                            actions.push(Action::DeletePack(i));
                        }
                    }
                });
            });
            ui.separator();

            if self.packs.is_empty() {
                ui.add_space(24.0);
                ui.label(RichText::new("No packs yet.").size(15.0).strong());
                ui.label(
                    RichText::new(
                        "Click \"Open packs folder\" on the left, drop a pack folder in, then Rescan.",
                    )
                    .weak(),
                );
                return;
            }

            let mut clicked = None;
            let mut double = None;
            let selected = self.selected;

            // The gallery must not eat the whole panel, or the details strip below
            // it ends up off-screen. auto_shrink is deliberately left at its
            // default so the scroll area takes only the height its tiles need,
            // capped by max_height.
            let has_details = !self.preview.is_empty() || self.preview_error.is_some();
            let detail_rows = self.preview.len().div_ceil(DETAILS_COLS).max(1);
            // Reserve enough height for up to two rows without scrolling; a pack
            // with more rows scrolls within the space instead of pushing the
            // gallery off screen.
            let details_h = if !has_details {
                0.0
            } else if self.preview_error.is_some() {
                80.0
            } else {
                40.0 + detail_rows.min(2) as f32 * DETAILS_ROW_H + 10.0
            };
            let gallery_h = (ui.available_height() - details_h).max(150.0);

            egui::ScrollArea::vertical()
                .id_salt("gallery-scroll")
                .max_height(gallery_h)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        for (i, found) in self.packs.iter().enumerate() {
                            let resp = pack_tile(
                                ui,
                                &found.pack,
                                found.builtin,
                                self.thumbs.get(&found.dir),
                                selected == Some(i),
                            );
                            if resp.clicked() {
                                clicked = Some(i);
                            }
                            if resp.double_clicked() {
                                double = Some(i);
                            }
                        }
                    });
                });

            if let Some(i) = clicked {
                self.selected = Some(i);
            }
            if let Some(i) = double {
                self.selected = Some(i);
                actions.push(Action::Apply(i));
            }

            if has_details {
                ui.separator();
                self.details(ui);
            }
        });
    }

    /// Details for the selected pack: every role at the size Windows will draw it,
    /// with animations running at their real speed.
    fn details(&mut self, ui: &mut egui::Ui) {
        if self.preview.is_empty() && self.preview_error.is_none() {
            return;
        }
        {
            ui.add_space(4.0);
            if let Some(err) = &self.preview_error {
                ui.label(
                    RichText::new("This pack could not be built")
                        .strong()
                        .color(Color32::from_rgb(240, 120, 120)),
                );
                ui.label(RichText::new(err).color(Color32::from_rgb(240, 160, 160)));
                return;
            }

            ui.horizontal(|ui| {
                ui.label(RichText::new("Every pointer in this pack").strong());
                ui.label(
                    RichText::new("actual size, animations at real speed")
                        .weak()
                        .small(),
                );
            });
            ui.add_space(4.0);

            let secs = self.started.elapsed().as_secs_f64();
            let ppp = ui.ctx().pixels_per_point();

            egui::ScrollArea::vertical()
                .id_salt("details-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for row in self.preview.chunks(DETAILS_COLS) {
                        ui.horizontal_top(|ui| {
                            for role in row {
                                role_chip(ui, role, secs, ppp);
                                ui.add_space(12.0);
                            }
                        });
                        ui.add_space(6.0);
                    }
                });
        }
    }

    fn quit_modal(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        egui::Modal::new(egui::Id::new("quit-prompt")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new("Quit CustoMouse?").size(16.0).strong());
            ui.add_space(8.0);
            ui.label(
                "A custom cursor pack is applied. Cursors stay as they are unless you \
                 restore them — they do not need this app to keep working.",
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new("Restore default, then quit")
                            .fill(Color32::from_rgb(120, 40, 40)),
                    )
                    .clicked()
                {
                    actions.push(Action::ConfirmQuit { restore: true });
                }
                if ui.button("Keep my cursors and quit").clicked() {
                    actions.push(Action::ConfirmQuit { restore: false });
                }
                if ui.button("Cancel").clicked() {
                    actions.push(Action::CancelQuit);
                }
            });
        });
    }

    fn delete_modal(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(name) = self
            .delete_prompt
            .and_then(|i| self.packs.get(i))
            .map(|p| p.pack.name.clone())
        else {
            self.delete_prompt = None;
            return;
        };
        let active = self.active_pack_name().as_deref() == Some(name.as_str());

        egui::Modal::new(egui::Id::new("delete-prompt")).show(ctx, |ui| {
            ui.set_width(430.0);
            ui.label(RichText::new(format!("Delete {name}?")).size(16.0).strong());
            ui.add_space(8.0);
            ui.label(
                "This removes the copy of the pack kept in the packs folder. \
                 The original files you imported from are left alone.",
            );
            if active {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "This pack is currently applied, so the Windows default cursors \
                         are restored first.",
                    )
                    .color(Color32::from_rgb(230, 190, 90)),
                );
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new("Delete").fill(Color32::from_rgb(120, 40, 40)))
                    .clicked()
                {
                    actions.push(Action::ConfirmDelete);
                }
                if ui.button("Cancel").clicked() {
                    actions.push(Action::CancelDelete);
                }
            });
        });
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_help;
        egui::Window::new("Making your own pack")
            .open(&mut open)
            .default_width(560.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("Already have .cur / .ani files?").strong());
                ui.label(
                    "Use \"Import .cur / .ani folder\" in the sidebar. Those files are                      already Windows cursors, so nothing is converted and nothing is                      redrawn - they are copied through byte for byte, keeping their own                      hotspots and animation timing. If the folder has an install.inf,                      that is used to map each file to the right pointer; otherwise the                      file names are matched.",
                );
                ui.add_space(8.0);

                ui.label(RichText::new("Where packs live").strong());
                ui.label(
                    "Put each pack in its own folder inside the packs folder \
                     (button in the sidebar). A pack is a folder with a pack.json in it.",
                );
                ui.add_space(8.0);

                ui.label(RichText::new("What art you can drop in").strong());
                egui::Grid::new("formats").striped(true).show(ui, |ui| {
                    ui.label(RichText::new("You supply").strong());
                    ui.label(RichText::new("You get").strong());
                    ui.end_row();
                    for (a, b) in [
                        (
                            "a .cur or .ani file",
                            "Used exactly as-is. Already a Windows cursor — do NOT convert it",
                        ),
                        ("one .svg file", "Static cursor, sharp at every size — best quality"),
                        ("one .png, 128px or larger", "Static cursor, downscaled only"),
                        ("a folder of name_00.png … name_11.png", "Animated, frames in order"),
                        ("a sprite sheet .png plus a grid", "Animated, cells read left to right"),
                        (".gif or animated .png", "Animated, timing read from the file"),
                    ] {
                        ui.label(a);
                        ui.label(b);
                        ui.end_row();
                    }
                });
                ui.add_space(8.0);

                ui.label(RichText::new("Rules worth knowing").strong());
                for line in [
                    "Frame numbers must run consecutively — a gap is reported as an error.",
                    "Every frame of an animation must be the same size.",
                    "Art below 128px is rejected, because upscaling it would look soft.",
                    "Non-square art is padded to a square, anchored at the top left.",
                    "hotspot is the clickable point, given in base_size coordinates.",
                    "Animated cursors keep 32/48/64 px; Windows limits how much each frame may hold.",
                    "An imported .cur/.ani only has the sizes it shipped with - that cannot be improved after the fact.",
                ] {
                    ui.label(format!("• {line}"));
                }
            });
        self.show_help = open;
    }
}

/// One pack in the gallery: every pointer it defines, laid out over a
/// checkerboard so transparency is visible, with the name underneath.
fn pack_tile(
    ui: &mut egui::Ui,
    pack: &cursorpack::manifest::Pack,
    builtin: bool,
    thumbs: Option<&crate::preview::PackThumbs>,
    selected: bool,
) -> egui::Response {
    const W: f32 = 196.0;
    const ART: f32 = 156.0;
    const CAPTION: f32 = 40.0;

    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(W, ART + CAPTION), egui::Sense::click());
    let art = egui::Rect::from_min_size(rect.min, egui::vec2(W, ART));

    checkerboard(ui, art);

    match thumbs {
        None => {
            ui.painter().text(
                art.center(),
                egui::Align2::CENTER_CENTER,
                "loading...",
                egui::FontId::proportional(12.0),
                Color32::from_gray(140),
            );
        }
        Some(t) if t.error.is_some() => {
            ui.painter().text(
                art.center(),
                egui::Align2::CENTER_CENTER,
                "cannot build",
                egui::FontId::proportional(12.0),
                Color32::from_rgb(230, 120, 120),
            );
        }
        Some(t) => {
            let n = t.textures.len().max(1);
            // Roughly square arrangement, so a 4-role pack and a 17-role pack
            // both fill the tile sensibly.
            let cols = (n as f32).sqrt().ceil().max(1.0);
            let rows = (n as f32 / cols).ceil().max(1.0);
            let cw = W / cols;
            let ch = ART / rows;
            let icon = (cw.min(ch) * 0.74).min(38.0);

            for (i, tex) in t.textures.iter().enumerate() {
                let cx = art.left() + ((i as f32 % cols) + 0.5) * cw;
                let cy = art.top() + ((i as f32 / cols).floor() + 0.5) * ch;
                let r = egui::Rect::from_center_size(
                    egui::pos2(cx, cy),
                    egui::vec2(icon, icon),
                );
                egui::Image::new((tex.id(), r.size())).paint_at(ui, r);
            }
        }
    }

    // Caption
    let cap = egui::Rect::from_min_size(
        egui::pos2(rect.left(), art.bottom()),
        egui::vec2(W, CAPTION),
    );
    let painter = ui.painter();
    painter.rect_filled(cap, 0.0, Color32::from_gray(32));
    painter.text(
        egui::pos2(cap.left() + 8.0, cap.top() + 8.0),
        egui::Align2::LEFT_TOP,
        &pack.name,
        egui::FontId::proportional(14.0),
        Color32::from_gray(230),
    );
    let sub = match (&pack.author, builtin) {
        (Some(a), true) => format!("by {a} · built in"),
        (Some(a), false) => format!("by {a}"),
        (None, true) => "built in".to_string(),
        (None, false) => format!("{} pointers", pack.active_roles().len()),
    };
    painter.text(
        egui::pos2(cap.left() + 8.0, cap.top() + 26.0),
        egui::Align2::LEFT_TOP,
        sub,
        egui::FontId::proportional(11.0),
        Color32::from_gray(150),
    );

    // Selection outline, plus a subtle hover cue.
    let stroke = if selected {
        egui::Stroke::new(2.0, Color32::from_rgb(240, 150, 60))
    } else if resp.hovered() {
        egui::Stroke::new(1.0, Color32::from_gray(120))
    } else {
        egui::Stroke::new(1.0, Color32::from_gray(60))
    };
    ui.painter()
        .rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);

    resp.on_hover_text(match &pack.description {
        Some(d) => d.clone(),
        None => pack.name.clone(),
    })
}

/// One pointer in the detail strip, drawn at the size Windows will actually use.
fn role_chip(ui: &mut egui::Ui, role: &RolePreview, secs: f64, ppp: f32) {
    ui.vertical(|ui| {
        let box_side = 68.0;
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(box_side, box_side), egui::Sense::hover());
        resp.on_hover_text(format!(
            "{} — registry role \"{}\"
{}",
            role.label, role.role, role.hint
        ));
        checkerboard(ui, rect);

        let tex = role.frame_at(secs);
        // Largest size that still fits the box, drawn at true pixel size.
        let px = role
            .sizes
            .iter()
            .copied()
            .filter(|&s| s as f32 / ppp <= box_side - 12.0)
            .max()
            .unwrap_or(32) as f32;
        let pts = px / ppp;
        let r = egui::Rect::from_center_size(rect.center(), egui::vec2(pts, pts));
        egui::Image::new((tex.id(), r.size())).paint_at(ui, r);

        // Hotspot marker.
        let hx = r.left() + r.width() * role.hotspot.0 / role.preview_px as f32;
        let hy = r.top() + r.height() * role.hotspot.1 / role.preview_px as f32;
        let p = ui.painter();
        let cross = Color32::from_rgb(255, 90, 90);
        p.line_segment([egui::pos2(hx - 4.0, hy), egui::pos2(hx + 4.0, hy)], (1.0, cross));
        p.line_segment([egui::pos2(hx, hy - 4.0), egui::pos2(hx, hy + 4.0)], (1.0, cross));

        ui.add_space(2.0);
        ui.label(RichText::new(role.label).small().strong());
        if role.animated {
            ui.label(
                RichText::new(format!("{} frames", role.frame_count))
                    .small()
                    .color(Color32::LIGHT_BLUE),
            );
        } else {
            ui.label(RichText::new(format!("to {}px", role.sizes.iter().max().copied().unwrap_or(32))).small().weak());
        }
    });
}

/// Group repetitive warnings into one line each.
///
/// A downloaded pack typically trips the same low-resolution warning for every
/// pointer it contains; printing seventeen near-identical sentences buries the
/// ones that differ.
fn collapse_warnings(warnings: &[cursorpack::raster::Warning]) -> Vec<String> {
    use cursorpack::raster::Warning;

    let mut low: Vec<(&str, u32)> = Vec::new();
    let mut rest: Vec<String> = Vec::new();

    for w in warnings {
        match w {
            Warning::LowResolutionSource { role, largest } => low.push((role, *largest)),
            other => rest.push(other.to_string()),
        }
    }

    let mut out = Vec::new();
    if !low.is_empty() {
        let mut sizes: Vec<u32> = low.iter().map(|(_, s)| *s).collect();
        sizes.sort_unstable();
        sizes.dedup();
        let biggest = sizes.last().copied().unwrap_or(0);

        out.push(if low.len() == 1 {
            format!(
                "{} comes from a ready-made cursor file that only goes up to {biggest}px, \
                 so Windows will scale it up on high-DPI screens.",
                low[0].0
            )
        } else {
            format!(
                "{} pointers come from ready-made cursor files that only go up to {biggest}px. \
                 They are used exactly as supplied, but Windows will scale them up on high-DPI \
                 screens or at large pointer sizes.",
                low.len()
            )
        });
    }

    rest.dedup();
    out.extend(rest);
    out
}

/// Make a folder name safe to use as a directory.
fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "imported-pack".into()
    } else {
        s
    }
}

/// A mid-grey checkerboard, so transparent areas of a cursor are obvious.
///
/// Deliberately mid-toned rather than dark: cursor packs come in both black and
/// white, and either extreme would hide one of them.
fn checkerboard(ui: &egui::Ui, rect: egui::Rect) {
    let p = ui.painter();
    p.rect_filled(rect, 2.0, Color32::from_gray(104));
    let step: f32 = 8.0;
    let mut y = rect.top();
    let mut row = 0;
    while y < rect.bottom() {
        let mut x = rect.left();
        let mut col = row;
        while x < rect.right() {
            if col % 2 == 0 {
                let cell = egui::Rect::from_min_size(
                    egui::pos2(x, y),
                    egui::vec2(step.min(rect.right() - x), step.min(rect.bottom() - y)),
                );
                p.rect_filled(cell, 0.0, Color32::from_gray(122));
            }
            x += step;
            col += 1;
        }
        y += step;
        row += 1;
    }
}

/// Add or remove the per-user startup entry.
#[cfg(windows)]
fn set_autostart(enable: bool) -> anyhow::Result<()> {
    use auto_launch::AutoLaunchBuilder;

    let exe = std::env::current_exe()?;
    let launcher = AutoLaunchBuilder::new()
        .set_app_name("CustoMouse")
        .set_app_path(&exe.to_string_lossy())
        .set_args(&["--tray"])
        .build()?;

    if enable {
        launcher.enable()?;
    } else {
        launcher.disable()?;
    }
    Ok(())
}
