//! The launcher view: pure `egui::Ui` calls over a [`LauncherState`].
//!
//! [`launcher_ui`] only edits settings and reports what the player asked for
//! as an [`Action`]; opening file dialogs and starting the game are the
//! caller's job (see [`crate::app`]), so the view can be embedded elsewhere.

use std::path::{Path, PathBuf};
use std::time::Duration;

use egui::{Color32, RichText};

use crate::game::{self, GameExit, RunningGame};
use crate::settings::{
    build_args, format_command, scan_packs, validate, Multiplayer, NetMode, PackEntry, Settings,
    Widescreen, DEFAULT_SIGNAL_URL, GAME_CONFIG_FILE_NAME, MAX_HD_SCALE, MAX_SCALE,
};

/// Something the view wants the caller to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Start the game with the current settings.
    Play,
    /// Pick the ROM file.
    BrowseRom,
    /// Pick an HD pack folder.
    BrowseHdPack,
    /// Pick the game program (Advanced).
    BrowseGameBinary,
    /// Open the data folder in the file browser.
    OpenDataFolder,
    /// Open (creating it first) the folder the pack list is read from.
    OpenPacksFolder,
    /// Read the pack list again after packs were added.
    RescanPacks,
    /// Pick the randomizer sprite IPS patch.
    BrowseSpriteIps,
    /// Pick where the randomizer spoiler log goes.
    BrowseSpoiler,
}

/// Top-level tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// ROM, display, packs and players.
    #[default]
    Play,
    /// ZALiA-inspired enhancements (runtime options, including the light
    /// "Randomizer & start" group).
    Enhancements,
    /// The ROM randomizer: seed and flags (`z2-rando`).
    Randomizer,
}

impl Tab {
    /// Every tab, in strip order.
    pub const ALL: [Tab; 3] = [Tab::Play, Tab::Enhancements, Tab::Randomizer];
}

/// Launcher state: settings plus the running game, if any.
pub struct LauncherState {
    /// What the player picked.
    pub settings: Settings,
    /// `<data-dir>` (settings, packs, the game's config).
    pub data_dir: PathBuf,
    /// Packs found under `<data-dir>/packs/`.
    pub packs: Vec<PackEntry>,
    /// The HD dropdown is on "Custom folder…".
    pub hd_custom: bool,
    /// The game, while it runs.
    pub game: Option<RunningGame>,
    /// How the last game ended (shown when it failed).
    pub last_exit: Option<GameExit>,
    /// Latest error from the launcher itself (spawn failure, save failure…).
    pub error: Option<String>,
    /// What the game's graphics-backend crash guard says
    /// ([`game::gpu_guard_notice`]), read at start and after each run.
    pub gpu_notice: Option<String>,
    /// Give Play keyboard focus the first time it is enabled, so Enter starts.
    focus_play: bool,
    /// Selected top-level tab.
    pub tab: Tab,
    /// The flag-string text box (may hold a string being typed).
    pub flag_text: String,
    /// Why [`Self::flag_text`] does not parse, if it does not.
    pub flag_error: Option<String>,
    /// Flag string the widgets produced last frame, to notice widget edits.
    last_flags: String,
    /// Open every randomizer section the first time they are drawn (tests).
    pub rando_expand: bool,
    /// Open every enhancement group (the headless render test uses it so
    /// every control is drawn).
    pub(crate) enh_force_open: bool,
}

impl LauncherState {
    /// State for `settings`, with packs scanned from `data_dir`.
    pub fn new(settings: Settings, data_dir: PathBuf) -> Self {
        let packs = scan_packs(&data_dir.join("packs"));
        let hd = settings.hd_pack.trim();
        let hd_custom = !hd.is_empty() && !packs.iter().any(|p| p.path.as_path() == Path::new(hd));
        let mut state = LauncherState {
            settings,
            data_dir,
            packs,
            hd_custom,
            game: None,
            last_exit: None,
            error: None,
            gpu_notice: None,
            focus_play: true,
            tab: Tab::Play,
            flag_text: String::new(),
            flag_error: None,
            last_flags: String::new(),
            rando_expand: false,
            enh_force_open: false,
        };
        state.refresh_gpu_notice();
        state
    }

    /// Read the game's graphics-backend guard file again.
    pub fn refresh_gpu_notice(&mut self) {
        self.gpu_notice = std::fs::read_to_string(self.data_dir.join(game::GPU_GUARD_FILE_NAME))
            .ok()
            .and_then(|t| game::gpu_guard_notice(&t));
    }

    /// Read `<data-dir>/packs/` again (after the player unzipped a pack).
    pub fn rescan_packs(&mut self) {
        self.packs = scan_packs(&self.data_dir.join("packs"));
        let hd = self.settings.hd_pack.trim();
        self.hd_custom =
            !hd.is_empty() && !self.packs.iter().any(|p| p.path.as_path() == Path::new(hd));
    }

    /// `<data-dir>/launcher.json`.
    pub fn settings_path(&self) -> PathBuf {
        Settings::path_in(&self.data_dir)
    }

    /// Save the settings; failures land in [`Self::error`].
    pub fn save(&mut self) {
        if let Err(e) = self.settings.save(&self.settings_path()) {
            self.error = Some(format!(
                "Could not save launcher settings to {}: {e}",
                self.settings_path().display()
            ));
        }
    }

    /// `true` while the game process is alive.
    pub fn game_running(&self) -> bool {
        self.game.is_some()
    }

    /// Check on the game; call once per frame.
    pub fn poll(&mut self) {
        if let Some(g) = &mut self.game {
            if let Some(exit) = g.poll() {
                self.game = None;
                self.last_exit = Some(exit);
                self.refresh_gpu_notice();
            }
        }
    }

    /// Validate, save and start the game. `Ok` once it is running.
    pub fn play(&mut self) -> Result<(), String> {
        self.error = None;
        self.last_exit = None;
        let problems = validate(&self.settings);
        if !problems.is_empty() {
            return Err(problems.join("\n"));
        }
        self.save();
        let program = game::resolve_game_binary_here(&self.settings.game_bin_override)?;
        let log = self.data_dir.join(game::GAME_LOG_FILE_NAME);
        let running = RunningGame::spawn(&program, &build_args(&self.settings), &log)
            .map_err(|e| format!("Could not start {}: {e}", program.display()))?;
        self.game = Some(running);
        Ok(())
    }
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).weak());
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(6.0);
    ui.heading(title);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
}

/// Draw the launcher into `ui`. Returns what the player asked for, if
/// anything, this frame.
pub fn launcher_ui(ui: &mut egui::Ui, state: &mut LauncherState) -> Option<Action> {
    state.poll();
    if state.game_running() {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
    let mut action = None;

    egui::Panel::bottom("z2-launcher-play")
        .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(12.0))
        .show(ui, |ui| play_bar(ui, state, &mut action));

    egui::CentralPanel::default_margins().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut state.tab, Tab::Play, RichText::new("Play").size(16.0));
            let label = if state.settings.enhancements.any_gameplay_active() {
                "Enhancements (on)"
            } else {
                "Enhancements"
            };
            ui.selectable_value(
                &mut state.tab,
                Tab::Enhancements,
                RichText::new(label).size(16.0),
            );
            let label = if state.settings.rando.enabled {
                "ROM randomizer (on)"
            } else {
                "ROM randomizer"
            };
            ui.selectable_value(
                &mut state.tab,
                Tab::Randomizer,
                RichText::new(label).size(16.0),
            );
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match state.tab {
                Tab::Play => {
                    rom_section(ui, state, &mut action);
                    display_section(ui, &mut state.settings);
                    hd_section(ui, state, &mut action);
                    players_section(ui, &mut state.settings);
                    ui.add_space(6.0);
                    command_section(ui, state);
                    controls_section(ui, state, &mut action);
                    advanced_section(ui, state, &mut action);
                    ui.add_space(8.0);
                }
                Tab::Enhancements => {
                    crate::enh_ui::enhancements_section(
                        ui,
                        &mut state.settings,
                        state.enh_force_open,
                    );
                    ui.add_space(8.0);
                }
                Tab::Randomizer => {
                    randomizer_tab(ui, state, &mut action);
                    ui.add_space(8.0);
                }
            });
    });
    action
}

/// Keep the flag-string box in step with the widgets: when the options
/// changed since the last frame (a widget or a preset), show their string.
fn sync_flag_text(state: &mut LauncherState) {
    let current = state.settings.rando.flags.to_flag_string();
    if current != state.last_flags {
        state.flag_text = current.clone();
        state.flag_error = None;
        state.last_flags = current;
    }
}

fn randomizer_tab(ui: &mut egui::Ui, state: &mut LauncherState, action: &mut Option<Action>) {
    use z2_rando::flags::{Flags, Preset};
    sync_flag_text(state);
    section(ui, "ROM randomizer", |ui| {
        ui.checkbox(
            &mut state.settings.rando.enabled,
            "Play a randomized game (generated from your ROM each time you start)",
        );
        hint(
            ui,
            "Rebuilds the world (map, palaces, items, enemies) from a seed. The Enhancements \
             tab's \"Randomizer & start\" group is a lighter runtime option: its start loadout, \
             scaling and palettes apply on top of a randomized game, but its item shuffle is \
             turned off while this is on.",
        );
        egui::Grid::new("rando-head")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Seed");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut state.settings.rando.seed)
                            .hint_text("any text")
                            .desired_width(200.0),
                    )
                    .on_hover_text("The same seed and options always give the same game.");
                    if ui.button("Random seed").clicked() {
                        state.settings.rando.seed = crate::settings::random_seed();
                    }
                });
                ui.end_row();

                ui.label("Preset");
                let r = &mut state.settings.rando;
                let current = Preset::detect(&r.flags);
                egui::ComboBox::from_id_salt("rando-preset")
                    .selected_text(current.map_or("Custom", Preset::label))
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for p in Preset::ALL {
                            if ui
                                .selectable_label(current == Some(*p), p.label())
                                .on_hover_text(p.description())
                                .clicked()
                            {
                                r.flags = p.flags();
                                r.preset = Some(*p);
                            }
                        }
                    });
                ui.end_row();

                ui.label("Flags");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut state.flag_text)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY),
                );
                if resp.changed() {
                    match Flags::from_flag_string(&state.flag_text) {
                        Ok(f) => {
                            state.last_flags = f.to_flag_string();
                            state.settings.rando.flags = f;
                            state.flag_error = None;
                        }
                        Err(e) => state.flag_error = Some(e.to_string()),
                    }
                }
                ui.end_row();
            });
        if let Some(err) = &state.flag_error {
            ui.label(
                RichText::new(format!("Flag string not applied: {err}"))
                    .color(ui.visuals().error_fg_color),
            );
        }
        if ui.button("Copy flag string").clicked() {
            ui.ctx()
                .copy_text(state.settings.rando.flags.to_flag_string());
        }
        hint(
            ui,
            "Paste a flag string to load someone's options, or change the options below and share the string. Racers need the same seed, flags and z2rs version.",
        );
        ui.add_space(4.0);
        egui::Grid::new("rando-files")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Spoiler log");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut state.settings.rando.spoiler_path)
                            .hint_text("none")
                            .desired_width(260.0),
                    );
                    if ui.button("Choose…").clicked() {
                        *action = Some(Action::BrowseSpoiler);
                    }
                });
                ui.end_row();
            });
    });
    ui.add_space(6.0);
    let open = state.rando_expand;
    let sprite = &mut state.settings.rando.sprite_ips;
    let f = &mut state.settings.rando.flags;
    let titles = crate::rando_ui::MODULE_TITLES;
    let header = |ui: &mut egui::Ui, i: usize, body: &mut dyn FnMut(&mut egui::Ui)| {
        egui::CollapsingHeader::new(titles[i])
            .id_salt(("rando-module", i))
            .default_open(open)
            .show(ui, |ui| body(ui));
    };
    header(ui, 0, &mut |ui| {
        crate::rando_ui::start::ui(ui, &mut f.start)
    });
    header(ui, 1, &mut |ui| {
        crate::rando_ui::overworld::ui(ui, &mut f.overworld)
    });
    header(ui, 2, &mut |ui| {
        crate::rando_ui::palaces::ui(ui, &mut f.palaces)
    });
    header(ui, 3, &mut |ui| {
        crate::rando_ui::items::ui(ui, &mut f.items)
    });
    header(ui, 4, &mut |ui| {
        crate::rando_ui::enemies::ui(ui, &mut f.enemies)
    });
    header(ui, 5, &mut |ui| {
        crate::rando_ui::stats::ui(ui, &mut f.stats)
    });
    header(ui, 6, &mut |ui| {
        crate::rando_ui::spells::ui(ui, &mut f.spells)
    });
    header(ui, 7, &mut |ui| {
        crate::rando_ui::drops::ui(ui, &mut f.drops)
    });
    header(ui, 8, &mut |ui| {
        crate::rando_ui::hints::ui(ui, &mut f.hints)
    });
    header(ui, 9, &mut |ui| {
        crate::rando_ui::towns::ui(ui, &mut f.towns)
    });
    header(ui, 10, &mut |ui| crate::rando_ui::qol::ui(ui, &mut f.qol));
    header(ui, 11, &mut |ui| {
        if crate::rando_ui::cosmetic::sprite_patch(ui, sprite) {
            *action = Some(Action::BrowseSpriteIps);
        }
        crate::rando_ui::cosmetic::ui(ui, &mut f.cosmetic)
    });
    hint(
        ui,
        "A few options are kept in flag strings but have no effect yet (their tooltips say so); README.md lists what each option does.",
    );
}

fn rom_section(ui: &mut egui::Ui, state: &mut LauncherState, action: &mut Option<Action>) {
    section(ui, "Your game file", |ui| {
        ui.horizontal(|ui| {
            let browse_w = 96.0;
            ui.add(
                egui::TextEdit::singleline(&mut state.settings.rom_path)
                    .hint_text("Path to your Zelda II .nes file")
                    .desired_width(ui.available_width() - browse_w),
            );
            if ui.button("Browse…").clicked() {
                *action = Some(Action::BrowseRom);
            }
        });
        hint(
            ui,
            "Use your own copy of Zelda II: The Adventure of Link (USA) as a .nes file. \
             The launcher only remembers where it is. The file is never copied or uploaded.",
        );
    });
}

fn display_section(ui: &mut egui::Ui, s: &mut Settings) {
    section(ui, "Display", |ui| {
        egui::Grid::new("display-grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Window size");
                ui.add(egui::Slider::new(&mut s.scale, 1..=MAX_SCALE).suffix("x"));
                ui.end_row();

                ui.label("Fullscreen");
                ui.checkbox(&mut s.fullscreen, "Start in fullscreen");
                ui.end_row();

                ui.label("Scaling");
                ui.checkbox(&mut s.integer_scale, "Whole-number scaling only");
                ui.end_row();

                ui.label("Widescreen");
                egui::ComboBox::from_id_salt("widescreen")
                    .selected_text(s.widescreen.label())
                    .show_ui(ui, |ui| {
                        for w in [
                            Widescreen::Off,
                            Widescreen::W16x10,
                            Widescreen::W16x9,
                            Widescreen::W21x9,
                        ] {
                            ui.selectable_value(&mut s.widescreen, w, w.label());
                        }
                    });
                ui.end_row();
            });
        if s.widescreen != Widescreen::Off {
            ui.add_space(4.0);
            ui.checkbox(&mut s.wide_gameplay, "Enemies use the wide screen");
            hint(
                ui,
                "Enemies can appear and move in the extra space at the sides. This changes \
                 the game a little; online partners must pick the same.",
            );
            ui.checkbox(&mut s.margin_sprites, "Draw sprites in the margins");
            hint(
                ui,
                "Show characters and items that are just off the original screen.",
            );
        }
        hint(
            ui,
            "F11 or Alt+Enter switches fullscreen while playing. The picture fills the \
             screen height; whole-number scaling is a little sharper but can leave black \
             borders. On a 16:10 screen (Steam Deck, Legion Go, many laptops) pick 16:10 \
             widescreen to fill it edge to edge.",
        );
    });
}

fn hd_section(ui: &mut egui::Ui, state: &mut LauncherState, action: &mut Option<Action>) {
    section(ui, "HD graphics pack (optional)", |ui| {
        let current = state.settings.hd_pack.trim().to_string();
        let selected = if state.hd_custom {
            "Custom folder…".to_string()
        } else if current.is_empty() {
            "None".to_string()
        } else {
            state
                .packs
                .iter()
                .find(|p| p.path.as_path() == Path::new(&current))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Custom folder…".to_string())
        };
        ui.horizontal(|ui| {
            ui.label("Pack");
            egui::ComboBox::from_id_salt("hd-pack")
                .selected_text(selected)
                .width(260.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(!state.hd_custom && current.is_empty(), "None")
                        .clicked()
                    {
                        state.hd_custom = false;
                        state.settings.hd_pack.clear();
                    }
                    for p in &state.packs {
                        let on = !state.hd_custom && Path::new(&current) == p.path.as_path();
                        if ui.selectable_label(on, &p.name).clicked() {
                            state.hd_custom = false;
                            state.settings.hd_pack = p.path.to_string_lossy().into_owned();
                        }
                    }
                    if ui
                        .selectable_label(state.hd_custom, "Custom folder…")
                        .clicked()
                    {
                        state.hd_custom = true;
                    }
                });
        });
        if state.hd_custom {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.settings.hd_pack)
                        .hint_text("Folder that contains pack.json")
                        .desired_width(ui.available_width() - 96.0),
                );
                if ui.button("Browse…").clicked() {
                    *action = Some(Action::BrowseHdPack);
                }
            });
        }
        if !state.settings.hd_pack.trim().is_empty() {
            ui.horizontal(|ui| {
                ui.label("HD scale");
                ui.add(egui::Slider::new(
                    &mut state.settings.hd_scale,
                    1..=MAX_HD_SCALE,
                ));
            });
            hint(
                ui,
                "Use the scale the pack was drawn at (often 4). A scale the pack does not \
                 support falls back to the pack's own.",
            );
        }
        hint(
            ui,
            &format!(
                "Redrawn graphics. Unzip a pack into {} and press Refresh to list it \
                 here, or choose Custom folder… to use one from anywhere.",
                state.data_dir.join("packs").display()
            ),
        );
        ui.horizontal(|ui| {
            if ui.button("Open packs folder").clicked() {
                *action = Some(Action::OpenPacksFolder);
            }
            if ui.button("Refresh").clicked() {
                *action = Some(Action::RescanPacks);
            }
        });
    });
}

fn players_section(ui: &mut egui::Ui, s: &mut Settings) {
    section(ui, "Players", |ui| {
        ui.radio_value(&mut s.multiplayer, Multiplayer::Single, "Single player");
        ui.radio_value(
            &mut s.multiplayer,
            Multiplayer::Local,
            "Local co-op (same computer)",
        );
        ui.radio_value(&mut s.multiplayer, Multiplayer::Host, "Host online");
        ui.radio_value(&mut s.multiplayer, Multiplayer::Join, "Join online");

        match s.multiplayer {
            Multiplayer::Single => {}
            Multiplayer::Local => {
                ui.add_space(4.0);
                let mut pin = s.p2_pad.is_some();
                ui.horizontal(|ui| {
                    ui.checkbox(&mut pin, "Player 2 uses gamepad number");
                    let mut n = s.p2_pad.unwrap_or(1);
                    ui.add_enabled(pin, egui::DragValue::new(&mut n).range(0..=15));
                    s.p2_pad = pin.then_some(n);
                });
                hint(
                    ui,
                    "Gamepads count from 0 in the order they were connected. Leave this off \
                     to let player 2 use the second gamepad, or the keyboard (W/A/S/D).",
                );
            }
            Multiplayer::Host | Multiplayer::Join => online_fields(ui, s),
        }
    });
}

fn online_fields(ui: &mut egui::Ui, s: &mut Settings) {
    ui.add_space(4.0);
    egui::Grid::new("online-grid")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Room name");
            ui.add(
                egui::TextEdit::singleline(&mut s.room)
                    .hint_text("e.g. link-and-zelda")
                    .desired_width(240.0),
            );
            ui.end_row();

            ui.label("Server");
            ui.add(
                egui::TextEdit::singleline(&mut s.signal_url)
                    .hint_text(DEFAULT_SIGNAL_URL)
                    .desired_width(240.0),
            );
            ui.end_row();
        });
    hint(
        ui,
        "Both players pick the same room name and use the same ROM, with matching \
         widescreen, \"Enemies use the wide screen\" and gameplay enhancement settings. \
         The host is player 1.",
    );
    egui::CollapsingHeader::new("Advanced online settings")
        .id_salt("online-advanced")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Sync");
                ui.radio_value(&mut s.net_mode, NetMode::Rollback, "Rollback");
                ui.radio_value(&mut s.net_mode, NetMode::Lockstep, "Lockstep");
            });
            let max = s.net_mode.max_delay();
            s.net_delay = s.net_delay.min(max);
            ui.horizontal(|ui| {
                ui.label("Input delay (frames)");
                ui.add(egui::Slider::new(&mut s.net_delay, 0..=max));
            });
            hint(
                ui,
                "Rollback feels instant and fixes up the other player's moves (delay 0-3). \
                 Lockstep waits for both players every frame (delay 0-8).",
            );
            if ui.button("Use the public server").clicked() {
                s.signal_url = DEFAULT_SIGNAL_URL.to_string();
            }
            ui.label("ICE servers (optional)");
            ui.add(
                egui::TextEdit::singleline(&mut s.ice)
                    .hint_text("blank = default; 'none' = same network only")
                    .desired_width(f32::INFINITY),
            );
        });
}

fn command_line(state: &LauncherState) -> String {
    let program = game::resolve_game_binary_here(&state.settings.game_bin_override)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("z2-native{}", std::env::consts::EXE_SUFFIX));
    format_command(&program, &build_args(&state.settings))
}

fn command_section(ui: &mut egui::Ui, state: &LauncherState) {
    egui::CollapsingHeader::new("Command line")
        .id_salt("command")
        .show(ui, |ui| {
            hint(
                ui,
                "The exact command the Play button runs. Copy it into a script or shortcut.",
            );
            let cmd = command_line(state);
            ui.add(
                egui::TextEdit::multiline(&mut cmd.as_str())
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(command_line(state));
            }
        });
}

fn controls_section(ui: &mut egui::Ui, state: &LauncherState, action: &mut Option<Action>) {
    egui::CollapsingHeader::new("Controls")
        .id_salt("controls")
        .show(ui, |ui| {
            egui::Grid::new("controls-grid")
                .num_columns(2)
                .spacing([16.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    let rows: &[(&str, &str)] = &[
                        (
                            "Player 1",
                            "Arrows move, Z = A, X = B, Enter = Start, Right Shift = Select",
                        ),
                        (
                            "Player 2",
                            "W/A/S/D move, G = A, F = B, T = Start, R = Select (local co-op)",
                        ),
                        ("Gamepads", "Work automatically; the second one is player 2"),
                        ("Tab", "Fast-forward (hold)"),
                        ("F5 / F7", "Save / load state"),
                        ("F6 or 1-9, 0", "Choose save slot (shown in the title bar)"),
                        ("P", "Pause (. steps one frame while paused)"),
                        ("F11 / Alt+Enter", "Toggle fullscreen"),
                        ("Esc", "Quit"),
                    ];
                    for (k, v) in rows {
                        ui.label(RichText::new(*k).strong());
                        ui.label(*v);
                        ui.end_row();
                    }
                });
            hint(
                ui,
                "Saving, loading, pause and fast-forward are off during online play. \
                 You can also drop a .nes file onto the game window.",
            );
            ui.add_space(4.0);
            let cfg = state.data_dir.join(GAME_CONFIG_FILE_NAME);
            ui.label(format!(
                "Change keys and other options in the game's settings file:\n{}",
                cfg.display()
            ));
            if ui.button("Open data folder").clicked() {
                *action = Some(Action::OpenDataFolder);
            }
        });
}

fn advanced_section(ui: &mut egui::Ui, state: &mut LauncherState, action: &mut Option<Action>) {
    egui::CollapsingHeader::new("Advanced")
        .id_salt("advanced")
        .show(ui, |ui| {
            ui.label("Game program");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.settings.game_bin_override)
                        .hint_text("blank = find it next to the launcher")
                        .desired_width(ui.available_width() - 96.0),
                );
                if ui.button("Browse…").clicked() {
                    *action = Some(Action::BrowseGameBinary);
                }
            });
            match game::resolve_game_binary_here(&state.settings.game_bin_override) {
                Ok(p) => hint(ui, &format!("Using: {}", p.display())),
                Err(e) => {
                    ui.label(RichText::new(e).color(ui.visuals().warn_fg_color));
                }
            }
            hint(
                ui,
                &format!(
                    "Game output is saved to {}",
                    state.data_dir.join(game::GAME_LOG_FILE_NAME).display()
                ),
            );
        });
}

fn play_bar(ui: &mut egui::Ui, state: &mut LauncherState, action: &mut Option<Action>) {
    let problems = validate(&state.settings);
    let error_color = ui.visuals().error_fg_color;

    if let Some(err) = &state.error {
        ui.label(RichText::new(err).color(error_color));
    }
    if let Some(exit) = state.last_exit.as_ref().filter(|e| !e.success) {
        ui.label(RichText::new(format!("The game stopped ({}).", exit.status)).color(error_color));
        if let Some(gpu) = &exit.gpu_hint {
            ui.label(RichText::new(gpu).color(error_color));
        }
        if let Some(why) = exit.explanation {
            ui.label(RichText::new(why).color(error_color));
            hint(
                ui,
                &format!(
                    "Please attach {} and {} to a bug report.",
                    state
                        .data_dir
                        .join(game::GAME_CRASH_LOG_FILE_NAME)
                        .display(),
                    state.data_dir.join(game::GAME_LOG_FILE_NAME).display()
                ),
            );
        }
        if !exit.tail.is_empty() {
            egui::ScrollArea::vertical()
                .max_height(150.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    let text = exit.tail.join("\n");
                    ui.add(
                        egui::TextEdit::multiline(&mut text.as_str())
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY),
                    );
                });
        }
    }
    if !state.game_running() {
        if let Some(n) = &state.gpu_notice {
            ui.label(RichText::new(n).color(ui.visuals().warn_fg_color));
        }
        for p in &problems {
            ui.label(RichText::new(format!("• {p}")).color(ui.visuals().warn_fg_color));
        }
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let running = state.game_running();
        let label = if running { "Game running…" } else { "Play" };
        let play = ui.add_enabled(
            !running && problems.is_empty(),
            egui::Button::new(RichText::new(label).size(22.0).strong())
                .min_size(egui::vec2(160.0, 44.0))
                .fill(if running {
                    Color32::from_gray(60)
                } else {
                    Color32::from_rgb(40, 110, 60)
                }),
        );
        if state.focus_play && !running && problems.is_empty() {
            state.focus_play = false;
            play.request_focus();
        }
        if play.clicked() {
            *action = Some(Action::Play);
        }
        ui.vertical(|ui| {
            ui.checkbox(
                &mut state.settings.close_on_start,
                "Close launcher when the game starts",
            );
            if running {
                hint(ui, "Close the game window to come back here.");
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every section renders headless (no window) in every mode without
    /// panicking, and an idle frame asks for nothing.
    #[test]
    fn view_renders_headless_in_every_mode() {
        let dir = std::env::temp_dir().join(format!("z2-launcher-ui-{}", std::process::id()));
        let ctx = egui::Context::default();
        crate::app::apply_style(&ctx);
        for mp in [
            Multiplayer::Single,
            Multiplayer::Local,
            Multiplayer::Host,
            Multiplayer::Join,
        ] {
            for (ws, zalia) in [
                (Widescreen::Off, false),
                (Widescreen::W16x9, true),
                (Widescreen::W21x9, false),
            ] {
                let mut settings = Settings {
                    multiplayer: mp,
                    widescreen: ws,
                    hd_pack: "/no/such/pack".into(),
                    ..Settings::default()
                };
                if zalia {
                    settings.enhancements = z2_core::enh::Enhancements::zalia_preset();
                    settings.display_enh = z2_core::enh::DisplayEnh::zalia_preset();
                    settings.display_enh.effects_enabled = true;
                }
                let before = settings.clone();
                let mut state = LauncherState::new(settings, dir.clone());
                // Every enhancement group open, so every control renders.
                state.enh_force_open = zalia || mp == Multiplayer::Single;
                assert!(state.hd_custom, "unknown pack path shows as custom");
                for tab in Tab::ALL {
                    state.tab = tab;
                    for _ in 0..2 {
                        let mut action = None;
                        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                            action = launcher_ui(ui, &mut state);
                        });
                        out.textures_delta.clear();
                        assert_eq!(action, None);
                    }
                }
                assert_eq!(
                    state.settings, before,
                    "an idle frame leaves the settings alone"
                );
            }
        }
    }

    /// The Randomizer tab renders headless with every section open, for each
    /// preset, and the flag box follows the widgets.
    #[test]
    fn randomizer_tab_renders_headless() {
        use z2_rando::flags::Preset;
        let dir = std::env::temp_dir().join(format!("z2-launcher-rando-{}", std::process::id()));
        let ctx = egui::Context::default();
        crate::app::apply_style(&ctx);
        for p in Preset::ALL {
            let mut settings = Settings::default();
            settings.rando.enabled = true;
            settings.rando.flags = p.flags();
            let mut state = LauncherState::new(settings, dir.clone());
            state.tab = Tab::Randomizer;
            state.rando_expand = true;
            for _ in 0..2 {
                let mut action = None;
                let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    action = launcher_ui(ui, &mut state);
                });
                out.textures_delta.clear();
                assert_eq!(action, None);
            }
            assert_eq!(state.flag_text, p.flags().to_flag_string());
            assert!(state.flag_error.is_none());
        }
    }

    #[test]
    fn play_refuses_invalid_settings_without_spawning() {
        let dir = std::env::temp_dir().join(format!("z2-launcher-play-{}", std::process::id()));
        let mut state = LauncherState::new(Settings::default(), dir);
        let err = state.play().unwrap_err();
        assert!(err.contains("ROM"), "{err}");
        assert!(!state.game_running());
    }
}
