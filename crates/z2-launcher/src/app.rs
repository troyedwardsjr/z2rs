//! The desktop window: eframe glue around [`launcher_ui`], native file
//! pickers and window options.

use std::path::Path;

use egui::{FontId, TextStyle};

use crate::game;
use crate::settings::{data_dir, find_pack_root, Settings};
use crate::ui::{launcher_ui, Action, LauncherState};

/// Window title.
pub const WINDOW_TITLE: &str = "z2rs launcher";

/// The launcher as an eframe app.
pub struct LauncherApp {
    state: LauncherState,
}

impl LauncherApp {
    /// Load settings from the data dir and style the context.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        apply_style(&cc.egui_ctx);
        let dir = data_dir();
        let settings = Settings::load(&Settings::path_in(&dir));
        LauncherApp {
            state: LauncherState::new(settings, dir),
        }
    }

    fn handle(&mut self, ctx: &egui::Context, action: Action) {
        let s = &mut self.state.settings;
        match action {
            Action::Play => match self.state.play() {
                Ok(()) => {
                    if self.state.settings.close_on_start {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                Err(e) => self.state.error = Some(e),
            },
            Action::BrowseRom => {
                let mut dlg = rfd::FileDialog::new()
                    .set_title("Choose your Zelda II ROM")
                    .add_filter("NES ROM", &["nes", "NES"])
                    .add_filter("All files", &["*"]);
                if let Some(dir) = existing_parent(&s.rom_path) {
                    dlg = dlg.set_directory(dir);
                }
                if let Some(p) = dlg.pick_file() {
                    s.rom_path = p.to_string_lossy().into_owned();
                }
            }
            Action::BrowseHdPack => {
                let mut dlg = rfd::FileDialog::new().set_title("Choose an HD pack folder");
                let start = if s.hd_pack.trim().is_empty() {
                    Some(self.state.data_dir.join("packs")).filter(|p| p.is_dir())
                } else {
                    Some(Path::new(s.hd_pack.trim()).to_path_buf()).filter(|p| p.is_dir())
                };
                if let Some(dir) = start {
                    dlg = dlg.set_directory(dir);
                }
                if let Some(p) = dlg.pick_folder() {
                    let root = find_pack_root(&p).unwrap_or(p);
                    s.hd_pack = root.to_string_lossy().into_owned();
                    self.state.hd_custom = true;
                }
            }
            Action::BrowseGameBinary => {
                let mut dlg = rfd::FileDialog::new().set_title("Choose the z2rs game program");
                if let Some(dir) = existing_parent(&s.game_bin_override) {
                    dlg = dlg.set_directory(dir);
                }
                if let Some(p) = dlg.pick_file() {
                    s.game_bin_override = p.to_string_lossy().into_owned();
                }
            }
            Action::OpenPacksFolder => {
                let dir = self.state.data_dir.join("packs");
                if let Err(e) = game::open_folder(&dir) {
                    self.state.error = Some(format!("Could not open {}: {e}", dir.display()));
                }
            }
            Action::BrowseSpriteIps => {
                let mut dlg = rfd::FileDialog::new()
                    .set_title("Choose your sprite patch")
                    .add_filter("IPS patch", &["ips", "IPS"])
                    .add_filter("All files", &["*"]);
                if let Some(dir) = existing_parent(&s.rando.sprite_ips) {
                    dlg = dlg.set_directory(dir);
                }
                if let Some(p) = dlg.pick_file() {
                    s.rando.sprite_ips = p.to_string_lossy().into_owned();
                }
            }
            Action::BrowseSpoiler => {
                let mut dlg = rfd::FileDialog::new()
                    .set_title("Save the spoiler log as")
                    .add_filter("Text", &["txt"])
                    .set_file_name("z2rs-spoiler.txt");
                if let Some(dir) = existing_parent(&s.rando.spoiler_path) {
                    dlg = dlg.set_directory(dir);
                }
                if let Some(p) = dlg.save_file() {
                    s.rando.spoiler_path = p.to_string_lossy().into_owned();
                }
            }
            Action::RescanPacks => self.state.rescan_packs(),
            Action::OpenDataFolder => {
                if let Err(e) = game::open_folder(&self.state.data_dir) {
                    self.state.error = Some(format!(
                        "Could not open {}: {e}",
                        self.state.data_dir.display()
                    ));
                }
            }
        }
    }
}

fn existing_parent(path: &str) -> Option<std::path::PathBuf> {
    let p = Path::new(path.trim()).parent()?;
    p.is_dir().then(|| p.to_path_buf())
}

/// Dark theme with larger, easier-to-read text.
pub fn apply_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::proportional(20.0)),
            (TextStyle::Body, FontId::proportional(15.5)),
            (TextStyle::Button, FontId::proportional(15.5)),
            (TextStyle::Monospace, FontId::monospace(13.5)),
            (TextStyle::Small, FontId::proportional(12.5)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
        style.spacing.interact_size.y = 26.0;
    });
}

impl eframe::App for LauncherApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(action) = launcher_ui(ui, &mut self.state) {
            let ctx = ui.ctx().clone();
            self.handle(&ctx, action);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.state.save();
    }

    // Only the window position and size are remembered by eframe; the
    // settings themselves live in launcher.json.
    fn persist_egui_memory(&self) -> bool {
        false
    }
}

/// Window options for the launcher.
pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(WINDOW_TITLE)
            .with_app_id("z2rs-launcher")
            .with_inner_size([520.0, 640.0])
            .with_min_inner_size([440.0, 480.0]),
        ..Default::default()
    }
}

/// Open the launcher window and run until it closes.
pub fn run() -> eframe::Result {
    eframe::run_native(
        WINDOW_TITLE,
        native_options(),
        Box::new(|cc| Ok(Box::new(LauncherApp::new(cc)))),
    )
}
