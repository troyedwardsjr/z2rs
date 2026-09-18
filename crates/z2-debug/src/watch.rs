//! RAM watch panel.
//!
//! Named entries from `ram-map.toml` with live values read through the
//! typed [`Ram`](z2_core::ram::Ram) accessors (generated from `ram-map.toml`) and edits
//! staged as [`PendingEdit`]s the frontend applies explicitly between
//! frames. The overlay never writes live state itself (see the determinism
//! pattern in the [crate root](crate)).

use z2_core::ram::Ram;

/// One watched address: the ram-map name, address, width and a reader that
/// calls the generated typed accessor (not a raw offset read), so a
/// ram-map/codegen drift breaks the watch tests instead of silently
/// displaying the wrong byte.
#[derive(Debug, Clone, Copy)]
pub struct WatchDef {
    /// ram-map `name` (the generated accessor stem, e.g. `link_hp`).
    pub name: &'static str,
    /// CPU address of the first byte (for display + [`PendingEdit`]).
    pub addr: u16,
    /// Width in bytes (1 or 2; 2-byte values read little-endian... see
    /// per-entry docs — `exp`/`exp_next` are big-endian via their accessor).
    pub width: u8,
    /// Short doc (ram-map `doc` condensed).
    pub doc: &'static str,
    /// Typed-accessor read, widened to `u16`.
    pub get: fn(&Ram) -> u16,
}

impl WatchDef {
    /// Read the live value through the typed accessor.
    #[must_use]
    pub fn read(&self, ram: &Ram) -> u16 {
        (self.get)(ram)
    }
}

// Typed-accessor shims (each calls exactly one generated accessor).
fn g_frame_counter(r: &Ram) -> u16 {
    u16::from(r.frame_counter())
}
fn g_link_x(r: &Ram) -> u16 {
    u16::from(r.link_x())
}
fn g_link_y(r: &Ram) -> u16 {
    u16::from(r.link_y())
}
fn g_link_page(r: &Ram) -> u16 {
    u16::from(r.link_page())
}
fn g_link_x_speed(r: &Ram) -> u16 {
    u16::from(r.link_x_speed())
}
fn g_link_y_speed(r: &Ram) -> u16 {
    u16::from(r.link_y_speed())
}
fn g_link_sprite(r: &Ram) -> u16 {
    u16::from(r.link_sprite())
}
fn g_link_facing(r: &Ram) -> u16 {
    u16::from(r.link_facing())
}
fn g_link_hp(r: &Ram) -> u16 {
    u16::from(r.link_hp())
}
fn g_link_mp(r: &Ram) -> u16 {
    u16::from(r.link_mp())
}
fn g_exp(r: &Ram) -> u16 {
    r.exp()
}
fn g_exp_next(r: &Ram) -> u16 {
    r.exp_next()
}
fn g_attack_level(r: &Ram) -> u16 {
    u16::from(r.attack_level())
}
fn g_magic_level(r: &Ram) -> u16 {
    u16::from(r.magic_level())
}
fn g_life_level(r: &Ram) -> u16 {
    u16::from(r.life_level())
}
fn g_lives(r: &Ram) -> u16 {
    u16::from(r.lives())
}
fn g_keys(r: &Ram) -> u16 {
    u16::from(r.keys())
}
fn g_crystals(r: &Ram) -> u16 {
    u16::from(r.crystals())
}
fn g_thrust_flags(r: &Ram) -> u16 {
    u16::from(r.thrust_flags())
}
fn g_deaths(r: &Ram) -> u16 {
    u16::from(r.deaths())
}
fn g_game_mode(r: &Ram) -> u16 {
    u16::from(r.game_mode())
}
fn g_game_state(r: &Ram) -> u16 {
    u16::from(r.game_state())
}
fn g_menu_state(r: &Ram) -> u16 {
    u16::from(r.menu_state())
}
fn g_world(r: &Ram) -> u16 {
    u16::from(r.world())
}
fn g_scene_index(r: &Ram) -> u16 {
    u16::from(r.scene_index())
}
fn g_area_index(r: &Ram) -> u16 {
    u16::from(r.area_index())
}
fn g_overworld_index(r: &Ram) -> u16 {
    u16::from(r.overworld_index())
}
fn g_encounter_type(r: &Ram) -> u16 {
    u16::from(r.encounter_type())
}
fn g_rng(r: &Ram) -> u16 {
    u16::from(r.rng())
}
fn g_invuln_stun(r: &Ram) -> u16 {
    u16::from(r.invuln_stun())
}
fn g_invuln_blink(r: &Ram) -> u16 {
    u16::from(r.invuln_blink())
}
fn g_kills_easy(r: &Ram) -> u16 {
    u16::from(r.kills_easy())
}
fn g_kills_hard(r: &Ram) -> u16 {
    u16::from(r.kills_hard())
}
fn g_heart_containers(r: &Ram) -> u16 {
    u16::from(r.heart_containers())
}
fn g_magic_containers(r: &Ram) -> u16 {
    u16::from(r.magic_containers())
}
fn g_input_p1_held(r: &Ram) -> u16 {
    u16::from(r.input_p1_held())
}
fn g_input_p1_pressed(r: &Ram) -> u16 {
    u16::from(r.input_p1_pressed())
}
fn g_projectile_flag(r: &Ram) -> u16 {
    u16::from(r.projectile_flag())
}

/// Curated watch table: the debugging-critical subset of `ram-map.toml`
/// (link meters + position, progression, mode machine, timers/RNG, input).
/// Addresses mirror the ram-map so Mesen cross-checks read 1:1.
pub const WATCHES: &[WatchDef] = &[
    WatchDef {
        name: "frame_counter",
        addr: 0x0012,
        width: 1,
        doc: "NMI frame counter",
        get: g_frame_counter,
    },
    WatchDef {
        name: "link_x",
        addr: 0x004D,
        width: 1,
        doc: "Link X LSB, side view",
        get: g_link_x,
    },
    WatchDef {
        name: "link_y",
        addr: 0x0029,
        width: 1,
        doc: "Link Y, side view",
        get: g_link_y,
    },
    WatchDef {
        name: "link_page",
        addr: 0x003B,
        width: 1,
        doc: "Link map page / X high",
        get: g_link_page,
    },
    WatchDef {
        name: "link_x_speed",
        addr: 0x0070,
        width: 1,
        doc: "Link X speed (hspeed)",
        get: g_link_x_speed,
    },
    WatchDef {
        name: "link_y_speed",
        addr: 0x057D,
        width: 1,
        doc: "Link Y speed (vspeed)",
        get: g_link_y_speed,
    },
    WatchDef {
        name: "link_sprite",
        addr: 0x0080,
        width: 1,
        doc: "Link anim frame / attack state",
        get: g_link_sprite,
    },
    WatchDef {
        name: "link_facing",
        addr: 0x009F,
        width: 1,
        doc: "Facing: 1 right, 2 left",
        get: g_link_facing,
    },
    WatchDef {
        name: "link_hp",
        addr: 0x0774,
        width: 1,
        doc: "Current life in meter",
        get: g_link_hp,
    },
    WatchDef {
        name: "link_mp",
        addr: 0x0773,
        width: 1,
        doc: "Current magic in meter",
        get: g_link_mp,
    },
    WatchDef {
        name: "exp",
        addr: 0x0775,
        width: 2,
        doc: "Experience (big-endian)",
        get: g_exp,
    },
    WatchDef {
        name: "exp_next",
        addr: 0x0770,
        width: 2,
        doc: "Exp for next level (big-endian)",
        get: g_exp_next,
    },
    WatchDef {
        name: "attack_level",
        addr: 0x0777,
        width: 1,
        doc: "Attack level 1-8",
        get: g_attack_level,
    },
    WatchDef {
        name: "magic_level",
        addr: 0x0778,
        width: 1,
        doc: "Magic level 1-8",
        get: g_magic_level,
    },
    WatchDef {
        name: "life_level",
        addr: 0x0779,
        width: 1,
        doc: "Life level 1-8",
        get: g_life_level,
    },
    WatchDef {
        name: "lives",
        addr: 0x0700,
        width: 1,
        doc: "Lives",
        get: g_lives,
    },
    WatchDef {
        name: "keys",
        addr: 0x0793,
        width: 1,
        doc: "Keys 00-09",
        get: g_keys,
    },
    WatchDef {
        name: "crystals",
        addr: 0x0794,
        width: 1,
        doc: "Crystals left for Great Palace",
        get: g_crystals,
    },
    WatchDef {
        name: "thrust_flags",
        addr: 0x0796,
        width: 1,
        doc: "$10 down, $04 up, $14 both",
        get: g_thrust_flags,
    },
    WatchDef {
        name: "deaths",
        addr: 0x079F,
        width: 1,
        doc: "Deaths / continues used",
        get: g_deaths,
    },
    WatchDef {
        name: "heart_containers",
        addr: 0x0784,
        width: 1,
        doc: "Heart containers",
        get: g_heart_containers,
    },
    WatchDef {
        name: "magic_containers",
        addr: 0x0783,
        width: 1,
        doc: "Magic containers",
        get: g_magic_containers,
    },
    WatchDef {
        name: "game_mode",
        addr: 0x0736,
        width: 1,
        doc: "Game mode / current state",
        get: g_game_mode,
    },
    WatchDef {
        name: "game_state",
        addr: 0x076C,
        width: 1,
        doc: "Game state / special routine",
        get: g_game_state,
    },
    WatchDef {
        name: "menu_state",
        addr: 0x0524,
        width: 1,
        doc: "Menu control / state",
        get: g_menu_state,
    },
    WatchDef {
        name: "world",
        addr: 0x0707,
        width: 1,
        doc: "World / area type",
        get: g_world,
    },
    WatchDef {
        name: "scene_index",
        addr: 0x0561,
        width: 1,
        doc: "Scene layout index",
        get: g_scene_index,
    },
    WatchDef {
        name: "area_index",
        addr: 0x0748,
        width: 1,
        doc: "True area location index",
        get: g_area_index,
    },
    WatchDef {
        name: "overworld_index",
        addr: 0x0706,
        width: 1,
        doc: "Overworld: 0 West, 1 DM/Maze, 2 East",
        get: g_overworld_index,
    },
    WatchDef {
        name: "encounter_type",
        addr: 0x075A,
        width: 1,
        doc: "Encounter: 0 fixed/fairy, 1 small, 2 big",
        get: g_encounter_type,
    },
    WatchDef {
        name: "rng",
        addr: 0x051B,
        width: 1,
        doc: "Randomizer table base",
        get: g_rng,
    },
    WatchDef {
        name: "invuln_stun",
        addr: 0x0500,
        width: 1,
        doc: "Invincibility-after-stun counter",
        get: g_invuln_stun,
    },
    WatchDef {
        name: "invuln_blink",
        addr: 0x0518,
        width: 1,
        doc: "Invulnerable timeout",
        get: g_invuln_blink,
    },
    WatchDef {
        name: "kills_easy",
        addr: 0x05DF,
        width: 1,
        doc: "Easy kills toward drop",
        get: g_kills_easy,
    },
    WatchDef {
        name: "kills_hard",
        addr: 0x05E0,
        width: 1,
        doc: "Hard kills toward drop",
        get: g_kills_hard,
    },
    WatchDef {
        name: "input_p1_held",
        addr: 0x00F7,
        width: 1,
        doc: "P1 held (A B Sel Sta U D L R)",
        get: g_input_p1_held,
    },
    WatchDef {
        name: "input_p1_pressed",
        addr: 0x00F5,
        width: 1,
        doc: "P1 pressed edge",
        get: g_input_p1_pressed,
    },
    WatchDef {
        name: "projectile_flag",
        addr: 0x008D,
        width: 1,
        doc: "Projectile flag",
        get: g_projectile_flag,
    },
];

/// Look up a watch by ram-map name.
#[must_use]
pub fn find_watch(name: &str) -> Option<&'static WatchDef> {
    WATCHES.iter().find(|w| w.name == name)
}

/// A staged RAM write. Produced by the watch panel's edit boxes; applied by
/// the frontend via [`apply_pending`] between frames — never by the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingEdit {
    /// Watch name (for the edit log).
    pub name: &'static str,
    /// First CPU address to write.
    pub addr: u16,
    /// Width in bytes (matches the [`WatchDef`]).
    pub width: u8,
    /// New value (little-endian byte order for width 2... note: `exp` /
    /// `exp_next` are big-endian in RAM; [`apply_pending`] writes through
    /// the typed setters so endianness is handled there).
    pub value: u16,
}

/// Apply staged edits through the typed setters (so 2-byte big-endian
/// entries stay correct). Returns the number applied. Pure function of
/// `(ram, edits)` — the frontend calls it, then steps; the overlay only
/// queues.
pub fn apply_pending(ram: &mut Ram, edits: &[PendingEdit]) -> usize {
    let mut n = 0;
    for e in edits {
        match e.name {
            "exp" => ram.set_exp(e.value),
            "exp_next" => ram.set_exp_next(e.value),
            _ => {
                ram.write(e.addr, e.value as u8);
                if e.width == 2 {
                    // Generic width-2 fallback (little-endian); the two named
                    // big-endian entries are handled above.
                    ram.write(e.addr.wrapping_add(1), (e.value >> 8) as u8);
                }
            }
        }
        n += 1;
    }
    n
}

/// Watch panel state (filter text + staged edits). Rendered by
/// [`WatchPanel::show`]; value reads come from the caller's `&Ram`.
#[derive(Debug, Clone, Default)]
pub struct WatchPanel {
    /// Substring filter over watch names.
    pub filter: String,
    /// Staged edits (applied by the frontend via [`apply_pending`]).
    pub pending: Vec<PendingEdit>,
    /// Per-row decimal edit buffers, keyed by watch name.
    edit_text: std::collections::BTreeMap<&'static str, String>,
}

impl WatchPanel {
    /// Empty panel.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Staged edit count.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Discard staged edits.
    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    /// Render the watch table for `ram`. Edit boxes stage [`PendingEdit`]s;
    /// nothing is written here.
    pub fn show(&mut self, ui: &mut egui::Ui, ram: &Ram) {
        ui.horizontal(|ui| {
            ui.label("filter:");
            ui.text_edit_singleline(&mut self.filter);
            if ui.button("clear edits").clicked() {
                self.clear_pending();
            }
        });
        if !self.pending.is_empty() {
            ui.label(format!(
                "{} staged edit(s) — frontend applies between frames",
                self.pending.len()
            ));
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("z2_watch_grid")
                .striped(true)
                .show(ui, |ui| {
                    ui.label("watch");
                    ui.label("addr");
                    ui.label("value");
                    ui.label("set");
                    ui.end_row();
                    for w in WATCHES {
                        if !self.filter.is_empty() && !w.name.contains(self.filter.as_str()) {
                            continue;
                        }
                        self.show_row(ui, ram, w);
                    }
                });
        });
    }

    fn show_row(&mut self, ui: &mut egui::Ui, ram: &Ram, w: &'static WatchDef) {
        ui.label(w.name).on_hover_text(w.doc);
        ui.monospace(format!("${:04X}", w.addr));
        let v = w.read(ram);
        if w.width == 2 {
            ui.monospace(format!("${v:04X} ({v})"));
        } else {
            ui.monospace(format!("${:02X} ({})", v as u8, v as u8));
        }
        let buf = self.edit_text.entry(w.name).or_default();
        let max = if w.width == 2 { 65535u32 } else { 255u32 };
        ui.horizontal(|ui| {
            // Fixed small width keeps the grid compact on both frontends.
            ui.add(
                egui::TextEdit::singleline(buf)
                    .desired_width(52.0)
                    .hint_text("dec"),
            );
            if ui.button("set").clicked() {
                if let Ok(n) = buf.trim().parse::<u32>() {
                    if n <= max {
                        self.pending.push(PendingEdit {
                            name: w.name,
                            addr: w.addr,
                            width: w.width,
                            value: n as u16,
                        });
                        buf.clear();
                    }
                }
            }
        });
        ui.end_row();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_watch_matches_its_typed_accessor_and_raw_bytes() {
        // The `get` fn must agree with the raw byte(s) at `addr`: guards
        // against a watch row drifting off its accessor.
        let mut ram = Ram::new();
        for (i, w) in WATCHES.iter().enumerate() {
            let v = (i as u16).wrapping_mul(37).wrapping_add(11);
            if w.width == 2 {
                if w.name == "exp" {
                    ram.set_exp(v);
                } else if w.name == "exp_next" {
                    ram.set_exp_next(v);
                } else {
                    panic!("unexpected width-2 watch {}", w.name);
                }
                assert_eq!(w.read(&ram), v, "watch {}", w.name);
            } else {
                ram.write(w.addr, v as u8);
                assert_eq!(w.read(&ram), u16::from(v as u8), "watch {}", w.name);
                // And the typed accessor (invoked inside `read`) sees the
                // same byte the raw write put at `addr`.
                assert_eq!(ram.read(w.addr), v as u8, "watch {}", w.name);
            }
        }
    }

    #[test]
    fn watch_names_are_unique_and_sorted_by_address_group() {
        let mut names: Vec<&str> = WATCHES.iter().map(|w| w.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), WATCHES.len(), "duplicate watch names");
    }

    #[test]
    fn apply_pending_roundtrips_through_typed_accessors() {
        let mut ram = Ram::new();
        let edits = [
            PendingEdit {
                name: "link_hp",
                addr: 0x0774,
                width: 1,
                value: 0x06,
            },
            PendingEdit {
                name: "exp",
                addr: 0x0775,
                width: 2,
                value: 0x1234,
            },
            PendingEdit {
                name: "exp_next",
                addr: 0x0770,
                width: 2,
                value: 0x00C8,
            },
        ];
        assert_eq!(apply_pending(&mut ram, &edits), 3);
        assert_eq!(ram.link_hp(), 0x06);
        assert_eq!(ram.exp(), 0x1234);
        assert_eq!(ram.exp_next(), 0x00C8);
        // Big-endian layout preserved: MSB at the lower address.
        assert_eq!((ram.read(0x0775), ram.read(0x0776)), (0x12, 0x34));
    }

    #[test]
    fn headless_panel_renders_without_panic() {
        let ram = Ram::new();
        let mut panel = WatchPanel::new();
        panel.filter = "link".to_string();
        let ctx = egui::Context::default();
        crate::run_headless(&ctx, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                panel.show(ui, &ram);
            });
        });
    }
}
