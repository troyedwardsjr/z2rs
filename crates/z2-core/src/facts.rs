//! `GameFacts`: the typed automation snapshot.
//!
//! `GameFacts` is the one vocabulary shared by ported engine code, the
//! debug overlay and the QA/headless player: a plain-data snapshot of the
//! emulated state that serializes to identical JSON on native and web
//! (`wasm32-unknown-unknown`). This module is pure data + [`Ram`] reads —
//! no threads, no filesystem, no windowing — so it compiles unchanged for
//! both targets.
//!
//! Ownership note: [`Game`] is defined locally (the asset
//! loader keeps its own types); it wraps a [`Ram`] image, which crosses
//! crate boundaries as raw bytes via [`Ram::from_slice`]/[`Ram::as_slice`].
//!
//! # GameFacts JSON schema
//!
//! ```json
//! {
//!   "link":         { "x","y","page","x_speed","y_speed","facing","hp","mp",
//!                      "exp","attack","magic","life","lives","keys" },
//!   "spells":       { "shield","jump","life","fairy","fire","reflect",
//!                      "spell","thunder" },
//!   "items":        { "candle","glove","raft","boots","flute","cross",
//!                      "hammer","magic_key" },
//!   "world":        { "overworld","world","scene","area","encounter",
//!                      "crystals" },
//!   "mode":         { "mode","state","menu" },
//!   "enemies":      [ { "slot","id","exists","x","y","page","facing",
//!                        "speed","hp","stun" } x 6 ],
//!   "projectiles":  [ { "slot","id","x","y","page","facing","speed" } x 6 ],
//!   "projectile_flag": 0,
//!   "timers":       { "frame","invuln_stun","invuln_blink","kills_easy",
//!                      "kills_hard" },
//!   "rng": 0,
//!   "input":        { "p1_pressed","p2_pressed","p1_held","p2_held" }
//! }
//! ```
//!
//! All integers are `u8` except `link.exp` (`u16`, big-endian `$0775/76`);
//! spell/item flags are booleans (`false` = zero byte).
//!
//! # Mesen spot-check procedure (acceptance aid)
//!
//! `Ram` IS the NES CPU-RAM image (`$0000-$07FF`), so every field below maps
//! 1:1 onto Mesen's Debugger → Memory Viewer at the same savestate/frame:
//! set a breakpoint or pause, read the listed address, compare with the
//! `facts()` JSON. `tests/ram_facts.rs::mesen_spot_check_mapping` encodes
//! the 10-field spot-check table and asserts `facts()` exposes exactly the
//! bytes a reviewer would read in Mesen.

use crate::ram::Ram;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// Link state: position, motion, meters and progression (`$29/$4D/$3B`,
/// `$70/$57D`, `$773/$774`, `$775-6`, `$777-9`, `$700`, `$793`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkFacts {
    /// X position LSB (`$004D`).
    pub x: u8,
    /// Y position (`$0029`).
    pub y: u8,
    /// Map page (`$003B`).
    pub page: u8,
    /// X speed (`$0070`).
    pub x_speed: u8,
    /// Y speed (`$057D`).
    pub y_speed: u8,
    /// Facing direction, side scroll (`$009F`).
    pub facing: u8,
    /// Life left in meter (`$0774`).
    pub hp: u8,
    /// Magic left in meter (`$0773`).
    pub mp: u8,
    /// Experience, big-endian `$0775(MSB)/$0776(LSB)`.
    pub exp: u16,
    /// Attack level (`$0777`).
    pub attack: u8,
    /// Magic level (`$0778`).
    pub magic: u8,
    /// Life level (`$0779`).
    pub life: u8,
    /// Lives (`$0700`).
    pub lives: u8,
    /// Keys (`$0793`).
    pub keys: u8,
}

/// Spell possession flags (`$077B-$0782`; nonzero byte = learned).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpellFacts {
    /// Shield (`$077B`).
    pub shield: bool,
    /// Jump (`$077C`).
    pub jump: bool,
    /// Life (`$077D`).
    pub life: bool,
    /// Fairy (`$077E`).
    pub fairy: bool,
    /// Fire (`$077F`).
    pub fire: bool,
    /// Reflect (`$0780`).
    pub reflect: bool,
    /// Spell (`$0781`).
    pub spell: bool,
    /// Thunder (`$0782`).
    pub thunder: bool,
}

/// Item possession flags (`$0785-$078C`; nonzero byte = owned).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemFacts {
    /// Candle (`$0785`).
    pub candle: bool,
    /// Glove (`$0786`).
    pub glove: bool,
    /// Raft (`$0787`).
    pub raft: bool,
    /// Boots (`$0788`).
    pub boots: bool,
    /// Flute (`$0789`).
    pub flute: bool,
    /// Cross (`$078A`).
    pub cross: bool,
    /// Hammer (`$078B`).
    pub hammer: bool,
    /// Magic key (`$078C`).
    pub magic_key: bool,
}

/// World placement (`$706/$707`, `$561/$748`, `$75A`, `$794`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldFacts {
    /// Overworld index (`$0706`): 0 West, 1 DM/Maze Isle, 2 East.
    pub overworld: u8,
    /// World/area type (`$0707`).
    pub world: u8,
    /// Scene/map index (`$0561`).
    pub scene: u8,
    /// True area location index (`$0748`).
    pub area: u8,
    /// Encounter type (`$075A`): 0 fixed/fairy, 1 small, 2 big.
    pub encounter: u8,
    /// Crystals left for the Great Palace (`$0794`).
    pub crystals: u8,
}

/// Mode machine (`$736/$76C/$524`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeFacts {
    /// Game mode / current state (`$0736`).
    pub mode: u8,
    /// Game state (`$076C`).
    pub state: u8,
    /// Menu control/state (`$0524`).
    pub menu: u8,
}

/// One enemy slot (`$2A-2F/$4E-53/$3C-41/$60-65/$71-76/$A1-A6/$B6-BB/$C2-C7/$40E-413`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnemyFacts {
    /// Slot index 0-5 (slot `s` reads `base + s`).
    pub slot: u8,
    /// ID/type (`$00A1 + slot`).
    pub id: u8,
    /// Exists/active flag (`$00B6 + slot`).
    pub exists: u8,
    /// X position LSB (`$004E + slot`).
    pub x: u8,
    /// Y position (`$002A + slot`).
    pub y: u8,
    /// Map page (`$003C + slot`).
    pub page: u8,
    /// Facing direction (`$0060 + slot`).
    pub facing: u8,
    /// Speed (`$0071 + slot`).
    pub speed: u8,
    /// HP (`$00C2 + slot`).
    pub hp: u8,
    /// Stun-delay-when-hit timer (`$040E + slot`).
    pub stun: u8,
}

/// One projectile slot (`$30-35/$42-47/$54-59/$66-6B/$77-7C/$87-8C`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectileFacts {
    /// Slot index 0-5 (slot `s` reads `base + s`).
    pub slot: u8,
    /// ID/type (`$0087 + slot`).
    pub id: u8,
    /// X (`$0054 + slot`).
    pub x: u8,
    /// Y (`$0030 + slot`).
    pub y: u8,
    /// Map page (`$0042 + slot`).
    pub page: u8,
    /// Facing direction (`$0066 + slot`).
    pub facing: u8,
    /// Speed (`$0077 + slot`).
    pub speed: u8,
}

/// Timers and counters (`$12`, `$500`, `$518`, `$5DF-5E0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerFacts {
    /// Frame counter (`$0012`).
    pub frame: u8,
    /// Invincibility-after-stun counter (`$0500`).
    pub invuln_stun: u8,
    /// Invulnerable timeout (`$0518`).
    pub invuln_blink: u8,
    /// Easy kills toward drop (`$05DF`).
    pub kills_easy: u8,
    /// Hard kills toward drop (`$05E0`).
    pub kills_hard: u8,
}

/// Last sampled input (`$F5-F8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputFacts {
    /// Controller 1 pressed-edge (`$00F5`).
    pub p1_pressed: u8,
    /// Controller 2 pressed-edge (`$00F6`).
    pub p2_pressed: u8,
    /// Controller 1 held (`$00F7`).
    pub p1_held: u8,
    /// Controller 2 held (`$00F8`).
    pub p2_held: u8,
}

/// Whole-state automation snapshot. See the module docs for the JSON schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameFacts {
    /// Link state.
    pub link: LinkFacts,
    /// Spells learned.
    pub spells: SpellFacts,
    /// Items owned.
    pub items: ItemFacts,
    /// World placement.
    pub world: WorldFacts,
    /// Mode machine.
    pub mode: ModeFacts,
    /// Six enemy slots.
    pub enemies: [EnemyFacts; 6],
    /// Six projectile slots.
    pub projectiles: [ProjectileFacts; 6],
    /// Global projectile flag (`$008D`).
    pub projectile_flag: u8,
    /// Timers and kill counters.
    pub timers: TimerFacts,
    /// RNG state (`$051B`; confirm vs listing — see ram-map.toml).
    pub rng: u8,
    /// Last input.
    pub input: InputFacts,
}

impl GameFacts {
    /// Compact JSON (the headless `--dump` payload and the wasm bridge form).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("GameFacts serializes")
    }

    /// Pretty JSON for snapshots, dumps and human review.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("GameFacts serializes")
    }

    /// Parse back a [`GameFacts`] previously written by [`GameFacts::to_json`].
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// Minimal game handle: owns a [`Ram`] image and exports facts.
///
/// Deliberately local (the asset loader keeps its own `Game`); the two meet
/// at raw RAM bytes, never at types.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Game {
    ram: Ram,
}

impl Game {
    /// Wrap a RAM image.
    pub fn new(ram: Ram) -> Self {
        Self { ram }
    }

    /// Borrow the RAM image.
    pub fn ram(&self) -> &Ram {
        &self.ram
    }

    /// Mutably borrow the RAM image (e.g. to apply polled input bytes).
    pub fn ram_mut(&mut self) -> &mut Ram {
        &mut self.ram
    }

    /// Unwrap back into the RAM image.
    pub fn into_ram(self) -> Ram {
        self.ram
    }

    /// Export the typed automation snapshot (identical JSON on native/wasm).
    pub fn facts(&self) -> GameFacts {
        let ram = &self.ram;
        let enemy = |slot: usize| {
            let e = ram.enemy(slot);
            EnemyFacts {
                slot: slot as u8,
                id: e.id(),
                exists: e.exists(),
                x: e.x(),
                y: e.y(),
                page: e.page(),
                facing: e.facing(),
                speed: e.speed(),
                hp: e.hp(),
                stun: e.stun(),
            }
        };
        let projectile = |slot: usize| {
            let p = ram.projectile(slot);
            ProjectileFacts {
                slot: slot as u8,
                id: p.id(),
                x: p.x(),
                y: p.y(),
                page: p.page(),
                facing: p.facing(),
                speed: p.speed(),
            }
        };
        GameFacts {
            link: LinkFacts {
                x: ram.link_x(),
                y: ram.link_y(),
                page: ram.link_page(),
                x_speed: ram.link_x_speed(),
                y_speed: ram.link_y_speed(),
                facing: ram.link_facing(),
                hp: ram.link_hp(),
                mp: ram.link_mp(),
                exp: ram.exp(),
                attack: ram.attack_level(),
                magic: ram.magic_level(),
                life: ram.life_level(),
                lives: ram.lives(),
                keys: ram.keys(),
            },
            spells: SpellFacts {
                shield: ram.spell(0) != 0,
                jump: ram.spell(1) != 0,
                life: ram.spell(2) != 0,
                fairy: ram.spell(3) != 0,
                fire: ram.spell(4) != 0,
                reflect: ram.spell(5) != 0,
                spell: ram.spell(6) != 0,
                thunder: ram.spell(7) != 0,
            },
            items: ItemFacts {
                candle: ram.item(0) != 0,
                glove: ram.item(1) != 0,
                raft: ram.item(2) != 0,
                boots: ram.item(3) != 0,
                flute: ram.item(4) != 0,
                cross: ram.item(5) != 0,
                hammer: ram.item(6) != 0,
                magic_key: ram.item(7) != 0,
            },
            world: WorldFacts {
                overworld: ram.overworld_index(),
                world: ram.world(),
                scene: ram.scene_index(),
                area: ram.area_index(),
                encounter: ram.encounter_type(),
                crystals: ram.crystals(),
            },
            mode: ModeFacts {
                mode: ram.game_mode(),
                state: ram.game_state(),
                menu: ram.menu_state(),
            },
            enemies: [enemy(0), enemy(1), enemy(2), enemy(3), enemy(4), enemy(5)],
            projectiles: [
                projectile(0),
                projectile(1),
                projectile(2),
                projectile(3),
                projectile(4),
                projectile(5),
            ],
            projectile_flag: ram.projectile_flag(),
            timers: TimerFacts {
                frame: ram.frame_counter(),
                invuln_stun: ram.invuln_stun(),
                invuln_blink: ram.invuln_blink(),
                kills_easy: ram.kills_easy(),
                kills_hard: ram.kills_hard(),
            },
            rng: ram.rng(),
            input: InputFacts {
                p1_pressed: ram.input_p1_pressed(),
                p2_pressed: ram.input_p2_pressed(),
                p1_held: ram.input_p1_held(),
                p2_held: ram.input_p2_held(),
            },
        }
    }
}

/// One changed leaf between two [`GameFacts`] snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactDiff {
    /// Dotted path, e.g. `link.hp` or `enemies[2].hp`.
    pub path: String,
    /// Value in `a`.
    pub before: String,
    /// Value in `b`.
    pub after: String,
}

impl fmt::Display for FactDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} -> {}", self.path, self.before, self.after)
    }
}

fn render_scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}

fn diff_walk(path: String, a: &Value, b: &Value, out: &mut Vec<FactDiff>) {
    if a == b {
        return;
    }
    match (a, b) {
        (Value::Object(am), Value::Object(bm)) => {
            for (k, av) in am {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match bm.get(k) {
                    Some(bv) => diff_walk(p, av, bv, out),
                    None => out.push(FactDiff {
                        path: p,
                        before: render_scalar(av),
                        after: "<missing>".to_string(),
                    }),
                }
            }
            for (k, bv) in bm {
                if !am.contains_key(k) {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    out.push(FactDiff {
                        path: p,
                        before: "<missing>".to_string(),
                        after: render_scalar(bv),
                    });
                }
            }
        }
        (Value::Array(xs), Value::Array(ys)) if xs.len() == ys.len() => {
            for (i, (av, bv)) in xs.iter().zip(ys.iter()).enumerate() {
                diff_walk(format!("{path}[{i}]"), av, bv, out);
            }
        }
        _ => out.push(FactDiff {
            path,
            before: render_scalar(a),
            after: render_scalar(b),
        }),
    }
}

/// Diff two snapshots for tests: one [`FactDiff`] per changed leaf field,
/// sorted by path for deterministic assertions.
///
/// ```
/// use z2_core::facts::{Game, facts_diff};
/// use z2_core::ram::Ram;
///
/// let mut ram = Ram::new();
/// ram.set_link_hp(8);
/// let a = Game::new(ram.clone()).facts();
/// ram.set_link_hp(5);
/// let b = Game::new(ram).facts();
/// let diff = facts_diff(&a, &b);
/// assert_eq!(diff.len(), 1);
/// assert_eq!(diff[0].to_string(), "link.hp: 8 -> 5");
/// ```
pub fn facts_diff(a: &GameFacts, b: &GameFacts) -> Vec<FactDiff> {
    let va = serde_json::to_value(a).expect("GameFacts serializes");
    let vb = serde_json::to_value(b).expect("GameFacts serializes");
    let mut out = Vec::new();
    diff_walk(String::new(), &va, &vb, &mut out);
    out.sort_by(|x, y| x.path.cmp(&y.path));
    out
}
