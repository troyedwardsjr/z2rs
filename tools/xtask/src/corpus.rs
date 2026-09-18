//! `xtask corpus mint`: predicate-driven snapshot minting.
//!
//! ```
//! cargo xtask corpus mint --movie <M.fm2|M.bk2> [--out DIR] [--labels a,b,c]
//!                            [--every N] [--frames N] [--rom PATH]
//! ```
//!
//! The minter replays a movie through the lockstep oracle from power-on,
//! polls a RAM predicate per [`LABEL_PLAN`](z2_verify::snapshot::LABEL_PLAN)
//! label on every replayed frame, and captures a
//! [`Snapshot`](z2_verify::snapshot::Snapshot) on each predicate's rising
//! edge. It also records the `$12` frame-counter stall map (lag map) and a
//! deterministic `mint-manifest.json`.
//!
//! ## Predicate table
//!
//! Addresses come from `ram-map.toml` (verified entries) plus the listing
//! `third_party/z2disassembly/ram-map.txt` (`$56B` Town Code, `$56C` Palace
//! Code) and the bank-7 area-load formulas in `prg7.asm` (read-only sources):
//!
//! * `$056B = ($0748 - $2C) / 2`, `+4` when the region is not West
//!   (`$1CB8F` comment) — town code 0-7.
//! * `$056C = $0748 - $34` (`$1CB98` comment) — palace code 0-1; the palace
//!   index is Region(`$0706`) * 4 + Palace Code (bank-7 graphics-table
//!   comments), i.e. (region, code) = (0,0) Parapa 1, (0,1) Midoro 2,
//!   (1,0) Island 3, (1,1) Maze 4, (2,0) Ocean 5, (2,1) Three-Eye-Rock 6 —
//!   consistent with `$0707` world groups (3 = palaces 1/2/5, 4 = 3/4/6).
//! * `$0707` world, `$0706` region, `$076C` special-routine values and the
//!   `$077B` spell / `$0785` item orders are `ram-map.toml`-verified.
//!
//! Confidence legend: **high** = verified address + oracle-observed behavior
//! (boot-title) or verified address + persistent-flag mechanism (spells,
//! items, skills); **medium** = verified address + disassembly-sourced value
//! mapping, never yet observed live (no corpus movie reaches gameplay under
//! the present oracle — see below); **manual** = no constructible predicate.
//!
//! | label | predicate (rising edge) | detect | confidence |
//! |---|---|---|---|
//! | `boot-title` | `$736 == $11` | auto | high (pixel-verified title box) |
//! | `file-select` | — | manual (file-select mode byte unmapped) | — |
//! | `game-start` | — | manual (no verified game-start state) | — |
//! | `first-overworld-step` | — | manual (overworld pos `$73/$74` not in ram-map) | — |
//! | `town-rauru-enter` | `$707==1 && $56B==0` | auto | medium |
//! | `town-ruto-enter` | `$707==1 && $56B==1` | auto | medium |
//! | `town-saria-enter` | `$707==1 && $56B==2` | auto | medium |
//! | `town-mido-enter` | `$707==1 && $56B==3` | auto | medium |
//! | `town-nabooru-enter` | `$707==2 && $56B==4` | auto | medium |
//! | `town-darunia-enter` | `$707==2 && $56B==5` | auto | medium |
//! | `town-new-kasuto-enter` | `$707==2 && $56B==6` | auto | medium |
//! | `town-old-kasuto-enter` | `$707==2 && $56B==7` | auto | medium |
//! | `spell-shield` … `spell-thunder` | `$77B+i != 0` (i = 0..8) | auto | high |
//! | `skill-downstab` | `$796 & $10 != 0` | auto | high |
//! | `skill-upstab` | `$796 & $04 != 0` | auto | high |
//! | `item-candle` | `$785 != 0` | auto | high |
//! | `item-glove` | `$786 != 0` | auto | high |
//! | `item-raft` | `$787 != 0` | auto | high |
//! | `item-magic-key` | `$78C != 0` | auto | high |
//! | `palace1-enter` | `$707==3 && $706==0 && $56C==0` | auto | medium |
//! | `palace2-enter` | `$707==3 && $706==0 && $56C==1` | auto | medium |
//! | `palace3-enter` | `$707==4 && $706==1 && $56C==0` | auto | medium |
//! | `palace4-enter` | `$707==4 && $706==1 && $56C==1` | auto | medium |
//! | `palace5-enter` | `$707==3 && $706==2 && $56C==0` | auto | medium |
//! | `palace6-enter` | `$707==4 && $706==2 && $56C==1` | auto | medium |
//! | `boss-horsehead` … `boss-barba` (6) | — | manual (no boss-room mapping) | — |
//! | `great-palace-enter` | `$707==5` | auto | medium |
//! | `boss-thunderbird`, `boss-dark-link` | — | manual (no boss-room mapping) | — |
//! | `ending` | `$76C==4` (roll credits) | auto | medium |
//!
//! 31 auto rules, 11 manual. Manual labels are skipped with a clear message
//! and never guessed.
//!
//! ## Lag detection (`$12` stall signal)
//!
//! The detector flags raw frame `f` as lagged when `ram[$12]` did not
//! advance between the post-step reads of frames `f-1` and `f`, armed on the
//! first observed advance (pre-NMI boot frames precede the game's frame loop
//! and are not lag). Measured on `warp-glitch.bk2` (19,946 frames): 10,630
//! stalls, including a ~10k-frame terminal freeze while the replay sits on
//! the title/attract loop — so `$12`-stall conflates true gameplay lag with
//! idle/menu/wedged states. Consequences, by design:
//!
//! * `input_history` stores the **raw** track (bit-exact replay through
//!   `verify --snapshot`, which steps one frame per byte); the logic track
//!   is derived via the lag map, never stored.
//! * the lag map (`<movie>.lag.json`: `raw_frames`, `logic_frames`,
//!   `skipped[]`) is advisory bookkeeping, consistent with
//!   [`LagMap`](z2_verify::lag::LagMap) (`logic = raw - skipped.len()`).
//!
//! ## Snapshot content
//!
//! `ram`/`wram` are the live oracle images at capture (for diffing);
//! `ppu`/`apu`/`mapper` are empty (the oracle exposes no split export —
//! reserved); `oracle_blob` carries the full `save_state` image (the restore
//! source for `load_state`; v2 codec). `state_source` is set to `OracleV1`.
//!
//! ## Output layout (`--out DIR`)
//!
//! Default: `$Z2_CORPUS/snapshots`, else `./corpus/snapshots`.
//!
//! * `<label>.snap` — one per captured label (first rising edge wins;
//!   refires are counted in the manifest, never written).
//! * `<movie-stem>.lag.json` — per-movie lag map.
//! * `mint-manifest.json` — deterministic (sorted, no timestamps): captures
//!   (label → file, source movie, `frame_raw`/`frame_logic`, CRC32 hashes,
//!   predicate, detect, confidence), skipped labels with reasons, `--every`
//!   periodic checkpoints. Re-minting the same movie yields identical files.
//!
//! ## Why corpus movies reach (almost) nothing yet
//!
//! Probed Sep 2026 against the live oracle: all three `.bk2`/`.fm2` movies
//! press Start at raw frames 11+19, which boots to the title (`$11` at
//! f=27) and then times out into the attract loop — later Start presses
//! (e.g. f=4002) land outside the title's narrow Start-accept window and
//! are ignored, so no movie under the present oracle ever reaches file
//! select, gameplay, or any area/spell/item state. (The warp-glitch movie
//! additionally needs its TASeditor greenzone savestates, which `.bk2`
//! cannot carry.) This is an oracle/boot-timing fidelity gap (cf. the known
//! frame-7 NMI-quantization divergence), not a mint gap: the pipeline
//! captures every predicate that fires, proves multi-label determinism on a
//! scripted oracle, and keeps all 31 auto rules armed for in-sync movies.

use std::path::{Path, PathBuf};

use z2_verify::oracle::{Oracle, TetanesOracle};
use z2_verify::snapshot::{BlobLayoutV1, Snapshot};

// ---------------------------------------------------------------------------
// RAM addresses (see module docs for sources).
// ---------------------------------------------------------------------------

/// Game mode (`ram-map.toml`, verified).
const A_MODE: u16 = 0x0736;
/// Special routine / game state (`ram-map.toml`, verified).
const A_STATE: u16 = 0x076C;
/// Frame counter: the lag-detection signal (see module docs).
const A_FRAME_CTR: u16 = 0x0012;
/// Overworld index: 0 West, 1 DM/Maze Island, 2 East (verified).
const A_REGION: u16 = 0x0706;
/// World/area type (verified; 1 west towns, 2 east towns, 3 palaces 1/2/5,
/// 4 palaces 3/4/6, 5 Great Palace+ending).
const A_WORLD: u16 = 0x0707;
/// Town Code = ($748-$2C)/2 (+4 off-West); listing + prg7 formula.
const A_TOWN: u16 = 0x056B;
/// Palace Code = $748-$34; listing + prg7 formula.
const A_PALACE: u16 = 0x056C;
/// Title-screen mode (oracle-observed + pixel-verified).
const MODE_TITLE: u8 = 0x11;
/// `$76C` value that begins the credit roll (ram-map doc).
const STATE_CREDITS: u8 = 0x04;

fn peek(ram: &[u8], addr: u16) -> u8 {
    ram.get(addr as usize).copied().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Predicates: pure functions of CPU RAM, one per auto label.
// ---------------------------------------------------------------------------

fn p_boot_title(r: &[u8]) -> bool {
    peek(r, A_MODE) == MODE_TITLE
}
fn p_town_rauru(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 1 && peek(r, A_TOWN) == 0
}
fn p_town_ruto(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 1 && peek(r, A_TOWN) == 1
}
fn p_town_saria(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 1 && peek(r, A_TOWN) == 2
}
fn p_town_mido(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 1 && peek(r, A_TOWN) == 3
}
fn p_town_nabooru(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 2 && peek(r, A_TOWN) == 4
}
fn p_town_darunia(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 2 && peek(r, A_TOWN) == 5
}
fn p_town_new_kasuto(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 2 && peek(r, A_TOWN) == 6
}
fn p_town_old_kasuto(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 2 && peek(r, A_TOWN) == 7
}
/// Spell flag `i` (0 shield … 7 thunder) learned.
fn p_spell<const I: usize>(r: &[u8]) -> bool {
    peek(r, 0x077B + I as u16) != 0
}
fn p_spell_shield(r: &[u8]) -> bool {
    p_spell::<0>(r)
}
fn p_spell_jump(r: &[u8]) -> bool {
    p_spell::<1>(r)
}
fn p_spell_life(r: &[u8]) -> bool {
    p_spell::<2>(r)
}
fn p_spell_fairy(r: &[u8]) -> bool {
    p_spell::<3>(r)
}
fn p_spell_fire(r: &[u8]) -> bool {
    p_spell::<4>(r)
}
fn p_spell_reflect(r: &[u8]) -> bool {
    p_spell::<5>(r)
}
fn p_spell_spell(r: &[u8]) -> bool {
    p_spell::<6>(r)
}
fn p_spell_thunder(r: &[u8]) -> bool {
    p_spell::<7>(r)
}
fn p_skill_downstab(r: &[u8]) -> bool {
    peek(r, 0x0796) & 0x10 != 0
}
fn p_skill_upstab(r: &[u8]) -> bool {
    peek(r, 0x0796) & 0x04 != 0
}
fn p_item_candle(r: &[u8]) -> bool {
    peek(r, 0x0785) != 0
}
fn p_item_glove(r: &[u8]) -> bool {
    peek(r, 0x0786) != 0
}
fn p_item_raft(r: &[u8]) -> bool {
    peek(r, 0x0787) != 0
}
fn p_item_magic_key(r: &[u8]) -> bool {
    peek(r, 0x078C) != 0
}
fn p_palace1(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 3 && peek(r, A_REGION) == 0 && peek(r, A_PALACE) == 0
}
fn p_palace2(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 3 && peek(r, A_REGION) == 0 && peek(r, A_PALACE) == 1
}
fn p_palace3(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 4 && peek(r, A_REGION) == 1 && peek(r, A_PALACE) == 0
}
fn p_palace4(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 4 && peek(r, A_REGION) == 1 && peek(r, A_PALACE) == 1
}
fn p_palace5(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 3 && peek(r, A_REGION) == 2 && peek(r, A_PALACE) == 0
}
fn p_palace6(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 4 && peek(r, A_REGION) == 2 && peek(r, A_PALACE) == 1
}
fn p_great_palace(r: &[u8]) -> bool {
    peek(r, A_WORLD) == 5
}
fn p_ending(r: &[u8]) -> bool {
    peek(r, A_STATE) == STATE_CREDITS
}
/// Manual labels never match (the minter skips them before polling).
fn p_never(_: &[u8]) -> bool {
    false
}

// ---------------------------------------------------------------------------
// Rule table.
// ---------------------------------------------------------------------------

/// How a label is detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detect {
    /// RAM predicate, polled every replayed frame, captured on rising edge.
    Auto,
    /// No reliable predicate: skipped with a clear message, never guessed.
    Manual,
}

impl Detect {
    fn as_str(self) -> &'static str {
        match self {
            Detect::Auto => "auto",
            Detect::Manual => "manual",
        }
    }
}

/// One mintable label: predicate + provenance.
#[derive(Clone, Copy)]
pub struct LabelRule {
    /// [`LABEL_PLAN`](z2_verify::snapshot::LABEL_PLAN) id.
    pub label: &'static str,
    /// Auto (polled) or manual (skipped with a message).
    pub detect: Detect,
    /// `high` / `medium` (see module docs); `manual` rules carry `"-"`.
    pub confidence: &'static str,
    /// Human-readable predicate (recorded in the manifest).
    pub predicate_desc: &'static str,
    /// RAM predicate; `p_never` for manual rules.
    pub matches: fn(&[u8]) -> bool,
}

/// The full 42-label rule table, in [`LABEL_PLAN`](z2_verify::snapshot::LABEL_PLAN) order.
pub const LABEL_RULES: &[LabelRule] = &[
    LabelRule {
        label: "boot-title",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$736 == $11 rising edge (title screen up)",
        matches: p_boot_title,
    },
    LabelRule {
        label: "file-select",
        detect: Detect::Manual,
        confidence: "-",
        predicate_desc: "manual: file-select mode byte unmapped (never observed; movies time out of title into attract)",
        matches: p_never,
    },
    LabelRule {
        label: "game-start",
        detect: Detect::Manual,
        confidence: "-",
        predicate_desc: "manual: no verified game-start state (North Palace area value unmapped)",
        matches: p_never,
    },
    LabelRule {
        label: "first-overworld-step",
        detect: Detect::Manual,
        confidence: "-",
        predicate_desc: "manual: overworld position bytes $73/$74 not in ram-map; overworld mode value unmapped",
        matches: p_never,
    },
    LabelRule {
        label: "town-rauru-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 1 && $56B == 0 rising edge",
        matches: p_town_rauru,
    },
    LabelRule {
        label: "town-ruto-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 1 && $56B == 1 rising edge",
        matches: p_town_ruto,
    },
    LabelRule {
        label: "town-saria-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 1 && $56B == 2 rising edge",
        matches: p_town_saria,
    },
    LabelRule {
        label: "town-mido-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 1 && $56B == 3 rising edge",
        matches: p_town_mido,
    },
    LabelRule {
        label: "town-nabooru-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 2 && $56B == 4 rising edge",
        matches: p_town_nabooru,
    },
    LabelRule {
        label: "town-darunia-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 2 && $56B == 5 rising edge",
        matches: p_town_darunia,
    },
    LabelRule {
        label: "town-new-kasuto-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 2 && $56B == 6 rising edge",
        matches: p_town_new_kasuto,
    },
    LabelRule {
        label: "town-old-kasuto-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 2 && $56B == 7 rising edge",
        matches: p_town_old_kasuto,
    },
    LabelRule {
        label: "spell-shield",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$77B != 0 rising edge (learned)",
        matches: p_spell_shield,
    },
    LabelRule {
        label: "spell-jump",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$77C != 0 rising edge (learned)",
        matches: p_spell_jump,
    },
    LabelRule {
        label: "spell-life",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$77D != 0 rising edge (learned)",
        matches: p_spell_life,
    },
    LabelRule {
        label: "spell-fairy",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$77E != 0 rising edge (learned)",
        matches: p_spell_fairy,
    },
    LabelRule {
        label: "spell-fire",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$77F != 0 rising edge (learned)",
        matches: p_spell_fire,
    },
    LabelRule {
        label: "spell-reflect",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$780 != 0 rising edge (learned)",
        matches: p_spell_reflect,
    },
    LabelRule {
        label: "spell-spell",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$781 != 0 rising edge (learned)",
        matches: p_spell_spell,
    },
    LabelRule {
        label: "spell-thunder",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$782 != 0 rising edge (learned)",
        matches: p_spell_thunder,
    },
    LabelRule {
        label: "skill-downstab",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$796 & $10 != 0 rising edge",
        matches: p_skill_downstab,
    },
    LabelRule {
        label: "skill-upstab",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$796 & $04 != 0 rising edge",
        matches: p_skill_upstab,
    },
    LabelRule {
        label: "item-candle",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$785 != 0 rising edge (picked up)",
        matches: p_item_candle,
    },
    LabelRule {
        label: "item-glove",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$786 != 0 rising edge (picked up)",
        matches: p_item_glove,
    },
    LabelRule {
        label: "item-raft",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$787 != 0 rising edge (picked up)",
        matches: p_item_raft,
    },
    LabelRule {
        label: "item-magic-key",
        detect: Detect::Auto,
        confidence: "high",
        predicate_desc: "$78C != 0 rising edge (picked up)",
        matches: p_item_magic_key,
    },
    LabelRule {
        label: "palace1-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 3 && $706 == 0 && $56C == 0 rising edge",
        matches: p_palace1,
    },
    LabelRule {
        label: "boss-horsehead",
        detect: Detect::Manual,
        confidence: "-",
        predicate_desc: "manual: no verified boss-room (area,scene) mapping",
        matches: p_never,
    },
    LabelRule {
        label: "palace2-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 3 && $706 == 0 && $56C == 1 rising edge",
        matches: p_palace2,
    },
    LabelRule {
        label: "boss-helmethead",
        detect: Detect::Manual,
        confidence: "-",
        predicate_desc: "manual: no verified boss-room (area,scene) mapping",
        matches: p_never,
    },
    LabelRule {
        label: "palace3-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 4 && $706 == 1 && $56C == 0 rising edge",
        matches: p_palace3,
    },
    LabelRule {
        label: "boss-rebonack",
        detect: Detect::Manual,
        confidence: "-",
        predicate_desc: "manual: no verified boss-room (area,scene) mapping",
        matches: p_never,
    },
    LabelRule {
        label: "palace4-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 4 && $706 == 1 && $56C == 1 rising edge",
        matches: p_palace4,
    },
    LabelRule {
        label: "boss-carock",
        detect: Detect::Manual,
        confidence: "-",
        predicate_desc: "manual: no verified boss-room (area,scene) mapping",
        matches: p_never,
    },
    LabelRule {
        label: "palace5-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 3 && $706 == 2 && $56C == 0 rising edge",
        matches: p_palace5,
    },
    LabelRule {
        label: "boss-gooma",
        detect: Detect::Manual,
        confidence: "-",
        predicate_desc: "manual: no verified boss-room (area,scene) mapping",
        matches: p_never,
    },
    LabelRule {
        label: "palace6-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 4 && $706 == 2 && $56C == 1 rising edge",
        matches: p_palace6,
    },
    LabelRule {
        label: "boss-barba",
        detect: Detect::Manual,
        confidence: "-",
        predicate_desc: "manual: no verified boss-room (area,scene) mapping",
        matches: p_never,
    },
    LabelRule {
        label: "great-palace-enter",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$707 == 5 rising edge",
        matches: p_great_palace,
    },
    LabelRule {
        label: "boss-thunderbird",
        detect: Detect::Manual,
        confidence: "-",
        predicate_desc: "manual: no verified boss-room (area,scene) mapping",
        matches: p_never,
    },
    LabelRule {
        label: "boss-dark-link",
        detect: Detect::Manual,
        confidence: "-",
        predicate_desc: "manual: no verified boss-room (area,scene) mapping",
        matches: p_never,
    },
    LabelRule {
        label: "ending",
        detect: Detect::Auto,
        confidence: "medium",
        predicate_desc: "$76C == 4 rising edge (roll credits)",
        matches: p_ending,
    },
];

/// Look up a rule by label id.
pub fn rule_for(label: &str) -> Option<&'static LabelRule> {
    LABEL_RULES.iter().find(|r| r.label == label)
}

// ---------------------------------------------------------------------------
// Lag detector: armed `$12`-stall signal.
// ---------------------------------------------------------------------------

/// Lag-detector state: armed on the first `$12` advance (pre-NMI boot
/// frames precede the game's frame loop and are not lag).
#[derive(Debug, Clone, Copy, Default)]
pub struct LagDetector {
    prev: Option<u8>,
    armed: bool,
    /// Raw frames replayed before the first `$12` advance.
    pub boot_frames: u64,
}

impl LagDetector {
    /// Feed the post-step `$12` value for raw frame `frame`; returns true
    /// when the frame is flagged as lag (counter did not advance).
    pub fn feed(&mut self, frame: u64, ctr: u8) -> bool {
        let stall = match self.prev {
            None => {
                // First observation: arm only once the counter runs.
                self.prev = Some(ctr);
                if ctr != 0 {
                    self.armed = true;
                } else {
                    self.boot_frames = frame + 1;
                }
                return false;
            }
            Some(p) => {
                if !self.armed {
                    if ctr != p {
                        self.armed = true;
                    } else {
                        self.prev = Some(ctr);
                        self.boot_frames = frame + 1;
                        return false;
                    }
                }
                ctr == p
            }
        };
        self.prev = Some(ctr);
        stall
    }
}

// ---------------------------------------------------------------------------
// Mint core (generic over the oracle, so tests run on a scripted stub).
// ---------------------------------------------------------------------------

/// One captured label.
#[derive(Debug, Clone)]
pub struct Capture {
    /// Rule label.
    pub label: &'static str,
    /// Raw frames executed at capture (== inputs consumed).
    pub frame_raw: u64,
    /// Logic frames executed (`frame_raw - stalls so far`).
    pub frame_logic: u64,
    /// Live CPU RAM image.
    pub ram: Vec<u8>,
    /// Live WRAM image.
    pub wram: Vec<u8>,
    /// Full oracle save-state at capture.
    pub blob: Vec<u8>,
    /// Later rising edges of the same rule (first wins the `.snap` file).
    pub refires: u64,
}

/// One `--every` periodic checkpoint (coverage/timing, not a snapshot:
/// arbitrary frames have no [`LABEL_PLAN`](z2_verify::snapshot::LABEL_PLAN)
/// label and must never be mislabelled).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodicPoint {
    pub frame_raw: u64,
    pub frame_logic: u64,
    pub ram_crc32: u32,
}

/// Outcome of [`run_mint`].
#[derive(Debug, Clone)]
pub struct MintOutcome {
    /// Captures in firing order (first edge per label carries the blobs;
    /// refires only bump [`Capture::refires`]).
    pub captures: Vec<Capture>,
    /// Armed `$12` stalls, as raw frame indices.
    pub skipped: Vec<u32>,
    /// Raw frames replayed.
    pub replayed: usize,
    /// `replayed - skipped.len()` (matches
    /// [`LagMap`](z2_verify::lag::LagMap) arithmetic).
    pub logic_frames: u64,
    /// Pre-NMI boot frames (not counted as lag).
    pub boot_frames: u64,
    /// Periodic checkpoints.
    pub periodic: Vec<PeriodicPoint>,
}

/// Replay `track` through `oracle`, polling `rules` every frame and
/// capturing on rising edges.
///
/// * `max_frames == 0` means the whole track.
/// * `every > 0` records a [`PeriodicPoint`] every `every` raw frames.
/// * Deterministic: same oracle + track + rules ⇒ identical outcome.
///
/// Note: `max_frames` only bounds predicate polling; the stored
/// `input_history` always carries the full source-movie track.
pub fn run_mint<O: Oracle>(
    oracle: &mut O,
    track: &[u8],
    rules: &[&LabelRule],
    every: u64,
    max_frames: usize,
) -> MintOutcome {
    let total = if max_frames == 0 {
        track.len()
    } else {
        max_frames.min(track.len())
    };
    // Previous-frame predicate states (rising edges); start from power-on
    // RAM so a predicate already true at frame 0 still counts frame 0.
    let mut prev_hit: Vec<bool> = rules.iter().map(|r| (r.matches)(oracle.ram())).collect();
    let mut detector = LagDetector::default();
    let mut skipped: Vec<u32> = Vec::new();
    let mut captures: Vec<Capture> = Vec::new();
    let mut periodic: Vec<PeriodicPoint> = Vec::new();
    // Seed prev_hit from the pre-step state, then re-evaluate post-step.
    for (f, &input) in track.iter().take(total).enumerate() {
        oracle.step(input);
        let ram = oracle.ram();
        let stall = detector.feed(f as u64, ram[A_FRAME_CTR as usize]);
        if stall {
            skipped.push(f as u32);
        }
        let frame_raw = (f + 1) as u64;
        let frame_logic = frame_raw - skipped.len() as u64;
        for (i, rule) in rules.iter().enumerate() {
            let hit = (rule.matches)(ram);
            if hit && !prev_hit[i] {
                match captures.iter_mut().find(|c| c.label == rule.label) {
                    Some(c) => c.refires += 1,
                    None => captures.push(Capture {
                        label: rule.label,
                        frame_raw,
                        frame_logic,
                        ram: ram.to_vec(),
                        wram: oracle.wram().to_vec(),
                        blob: oracle.save_state(),
                        refires: 0,
                    }),
                }
            }
            prev_hit[i] = hit;
        }
        if every > 0 && frame_raw.is_multiple_of(every) {
            periodic.push(PeriodicPoint {
                frame_raw,
                frame_logic,
                ram_crc32: crc32_ieee(ram),
            });
        }
    }
    let replayed = total;
    let logic_frames = replayed as u64 - skipped.len() as u64;
    MintOutcome {
        captures,
        skipped,
        logic_frames,
        replayed,
        boot_frames: detector.boot_frames,
        periodic,
    }
}

// ---------------------------------------------------------------------------
// CRC32 + JSON (hand-rolled: xtask has no serde dependency).
// ---------------------------------------------------------------------------

/// CRC32-IEEE (same algorithm as the snapshot codec).
pub fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let m = if crc & 1 == 1 { 0xEDB8_8320 } else { 0 };
            crc = (crc >> 1) ^ m;
        }
    }
    !crc
}

fn json_escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Render the per-movie lag map (`<movie-stem>.lag.json`).
pub fn lag_json(movie: &str, outcome: &MintOutcome) -> String {
    let mut s = String::from("{\n");
    s.push_str("  \"movie\": ");
    json_escape(movie, &mut s);
    s.push_str(",\n");
    s.push_str(&format!("  \"raw_frames\": {},\n", outcome.replayed));
    s.push_str(&format!("  \"logic_frames\": {},\n", outcome.logic_frames));
    s.push_str("  \"skipped\": [");
    for (i, f) in outcome.skipped.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&format!("{f}"));
    }
    s.push_str("],\n");
    s.push_str("  \"detector\": \"f12-stall-armed\",\n");
    s.push_str(&format!(
        "  \"boot_frames_before_first_tick\": {},\n",
        outcome.boot_frames
    ));
    s.push_str(&format!(
        "  \"summary\": \"raw={} logic={} lag={}\"\n",
        outcome.replayed,
        outcome.logic_frames,
        outcome.skipped.len()
    ));
    s.push_str("}\n");
    s
}

/// Manifest entry for one capture.
fn capture_json(
    s: &mut String,
    cap: &Capture,
    file: &str,
    source_movie: &str,
    track_len: usize,
    blob_len: usize,
) {
    let rule = rule_for(cap.label).expect("capture of a known rule");
    s.push_str("    {\n");
    s.push_str("      \"label\": ");
    json_escape(cap.label, s);
    s.push_str(",\n      \"file\": ");
    json_escape(file, s);
    s.push_str(",\n      \"source_movie\": ");
    json_escape(source_movie, s);
    s.push_str(",\n");
    s.push_str(&format!(
        "      \"frame_raw\": {},\n      \"frame_logic\": {},\n",
        cap.frame_raw, cap.frame_logic
    ));
    s.push_str(&format!(
        "      \"ram_crc32\": \"{:08x}\",\n      \"wram_crc32\": \"{:08x}\",\n",
        crc32_ieee(&cap.ram),
        crc32_ieee(&cap.wram)
    ));
    s.push_str(&format!(
        "      \"blob_crc32\": \"{:08x}\",\n      \"blob_len\": {blob_len},\n      \"input_len\": {track_len},\n",
        crc32_ieee(&cap.blob),
    ));
    s.push_str("      \"predicate\": ");
    json_escape(rule.predicate_desc, s);
    s.push_str(",\n      \"detect\": ");
    json_escape(rule.detect.as_str(), s);
    s.push_str(",\n      \"confidence\": ");
    json_escape(rule.confidence, s);
    s.push_str(&format!(",\n      \"refires\": {}\n    }}", cap.refires));
}

/// Render `mint-manifest.json` (deterministic: plan order, no timestamps).
#[allow(clippy::too_many_arguments)]
pub fn manifest_json(
    movie: &str,
    track_len: usize,
    outcome: &MintOutcome,
    snap_files: &[(&'static str, String, usize)],
    skipped: &[(&'static str, String)],
    every: u64,
) -> String {
    let mut s = String::from("{\n");
    s.push_str("  \"tool\": \"xtask-corpus-mint\",\n  \"tool_version\": 1,\n");
    s.push_str("  \"movie\": ");
    json_escape(movie, &mut s);
    s.push_str(",\n");
    s.push_str(&format!("  \"movie_frames\": {track_len},\n"));
    s.push_str(&format!("  \"replayed_frames\": {},\n", outcome.replayed));
    s.push_str(&format!("  \"every\": {every},\n"));
    s.push_str("  \"snapshots\": [\n");
    // `snap_files` arrives in capture (firing) order — deterministic.
    for (i, (label, file, blob_len)) in snap_files.iter().enumerate() {
        let cap = outcome
            .captures
            .iter()
            .find(|c| c.label == *label)
            .expect("snap file for a capture");
        if i > 0 {
            s.push_str(",\n");
        }
        capture_json(&mut s, cap, file, movie, track_len, *blob_len);
    }
    s.push_str("\n  ],\n  \"skipped_labels\": [\n");
    for (i, (label, reason)) in skipped.iter().enumerate() {
        if i > 0 {
            s.push_str(",\n");
        }
        s.push_str("    {\"label\": ");
        json_escape(label, &mut s);
        s.push_str(", \"reason\": ");
        json_escape(reason, &mut s);
        s.push('}');
    }
    s.push_str("\n  ],\n  \"periodic\": [");
    for (i, p) in outcome.periodic.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "\n    {{\"frame_raw\": {}, \"frame_logic\": {}, \"ram_crc32\": \"{:08x}\"}}",
            p.frame_raw, p.frame_logic, p.ram_crc32
        ));
    }
    s.push_str("\n  ]\n}\n");
    s
}

// ---------------------------------------------------------------------------
// Snapshot files.
// ---------------------------------------------------------------------------

/// Build the encodable [`Snapshot`] for a capture (full raw track as
/// `input_history`; v2 blob attached; `OracleV1` layout).
pub fn snapshot_for(cap: &Capture, source_movie: &str, track: &[u8]) -> Result<Vec<u8>, String> {
    let snap = Snapshot::new(
        cap.ram.clone(),
        cap.wram.clone(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        track.to_vec(),
        cap.label,
        source_movie,
        cap.frame_raw,
        cap.frame_logic,
    )
    .map_err(|e| format!("snapshot {}: {}", cap.label, e.0))?;
    let mut snap = snap;
    snap.state_source = BlobLayoutV1::OracleV1;
    let snap = snap
        .with_oracle_blob(cap.blob.clone())
        .map_err(|e| format!("snapshot {} blob: {}", cap.label, e.0))?;
    snap.encode()
        .map_err(|e| format!("encode {}: {}", cap.label, e.0))
}

// ---------------------------------------------------------------------------
// CLI.
// ---------------------------------------------------------------------------

fn usage() -> &'static str {
    "cargo xtask corpus mint --movie <M.fm2|M.bk2> [--out DIR] [--labels a,b,c] [--every N] [--frames N] [--rom PATH]\n\
     \x20   --movie PATH   movie to replay (required; .fm2 or .bk2)\n\
     \x20   --out DIR      output dir (default: $Z2_CORPUS/snapshots, else ./corpus/snapshots)\n\
     \x20   --labels LIST  comma-separated LABEL_PLAN ids (default: all auto rules)\n\
     \x20   --every N      manifest checkpoint every N raw frames (default 0 = off)\n\
     \x20   --frames N     replay at most N frames (default 0 = whole movie)\n\
     \x20   --rom PATH     ROM file (default: $Z2_ROM)"
}

fn default_out_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("Z2_CORPUS") {
        PathBuf::from(dir).join("snapshots")
    } else {
        PathBuf::from("corpus/snapshots")
    }
}

/// Load a player-1 input track from `.fm2` or `.bk2` (extension-selected):
/// the `xtask verify` loader, so header warnings and the wrong-platform /
/// Reset-press refusals apply to the mint path too.
fn load_movie_track(path: &Path) -> Result<Vec<u8>, String> {
    super::verify::load_movie_track(path)
}

pub fn run(args: &[String]) -> i32 {
    let mut it = args.iter().peekable();
    match it.next().map(|s| s.as_str()) {
        Some("mint") => {}
        Some("-h" | "--help") | None => {
            println!("{usage}", usage = usage());
            return 0;
        }
        Some(other) => {
            eprintln!(
                "xtask corpus: unknown subcommand '{other}'\n\n{usage}",
                usage = usage()
            );
            return 2;
        }
    }
    let mut movie: Option<String> = None;
    let mut out: Option<String> = None;
    let mut labels: Option<String> = None;
    let mut every: u64 = 0;
    let mut frames: usize = 0;
    let mut rom: Option<String> = None;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--movie" => movie = it.next().cloned(),
            "--out" => out = it.next().cloned(),
            "--labels" => labels = it.next().cloned(),
            "--every" => match it.next().and_then(|s| s.parse().ok()) {
                Some(n) => every = n,
                None => {
                    eprintln!(
                        "xtask corpus mint: --every needs a number\n\n{usage}",
                        usage = usage()
                    );
                    return 2;
                }
            },
            "--frames" => match it.next().and_then(|s| s.parse().ok()) {
                Some(n) => frames = n,
                None => {
                    eprintln!(
                        "xtask corpus mint: --frames needs a number\n\n{usage}",
                        usage = usage()
                    );
                    return 2;
                }
            },
            "--rom" => rom = it.next().cloned(),
            "-h" | "--help" => {
                println!("{usage}", usage = usage());
                return 0;
            }
            other => {
                eprintln!(
                    "xtask corpus mint: unknown arg '{other}'\n\n{usage}",
                    usage = usage()
                );
                return 2;
            }
        }
    }
    let Some(movie) = movie else {
        eprintln!(
            "xtask corpus mint: need --movie M\n\n{usage}",
            usage = usage()
        );
        return 2;
    };
    let out_dir = out.map(PathBuf::from).unwrap_or_else(default_out_dir);

    // Resolve the active rule set.
    let mut active: Vec<&LabelRule> = Vec::new();
    let mut skipped: Vec<(&'static str, String)> = Vec::new();
    let explicit_labels = labels.is_some();
    match labels.as_deref() {
        Some(list) => {
            for id in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                match rule_for(id) {
                    None => {
                        eprintln!("xtask corpus mint: unknown label '{id}' (not in LABEL_PLAN)");
                        return 2;
                    }
                    Some(rule) if rule.detect == Detect::Manual => {
                        eprintln!("skip {id}: detect=manual ({})", rule.predicate_desc);
                        skipped.push((
                            rule.label,
                            format!("detect=manual: {}", rule.predicate_desc),
                        ));
                    }
                    Some(rule) => active.push(rule),
                }
            }
            if active.is_empty() && skipped.len() == 1 {
                eprintln!("xtask corpus mint: '{0}' is detect=manual; nothing to capture (never guess frames)", skipped[0].0);
                return 2;
            }
        }
        None => {
            for rule in LABEL_RULES {
                if rule.detect == Detect::Auto {
                    active.push(rule);
                }
            }
        }
    }

    let track = match load_movie_track(Path::new(&movie)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("xtask corpus mint: {e}");
            return 2;
        }
    };
    if track.is_empty() {
        eprintln!("xtask corpus mint: movie has no frames: {movie}");
        return 2;
    }
    let rom_path = match crate::verify::rom_path(rom.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("xtask corpus mint: {e}");
            return 2;
        }
    };
    let mut oracle = match TetanesOracle::load_rom(&rom_path) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("xtask corpus mint: load ROM {}: {e}", rom_path.display());
            return 2;
        }
    };

    let source_name = Path::new(&movie)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| movie.clone());
    let stem = Path::new(&movie)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "movie".to_string());

    eprintln!(
        "mint: replaying {} frames from {source_name} ...",
        if frames == 0 {
            track.len()
        } else {
            frames.min(track.len())
        }
    );
    let outcome = run_mint(&mut oracle, &track, &active, every, frames);

    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("xtask corpus mint: create {}: {e}", out_dir.display());
        return 2;
    }
    // Snapshots (first edge per label wins; refires only counted).
    let mut snap_files: Vec<(&'static str, String, usize)> = Vec::new();
    for cap in &outcome.captures {
        match snapshot_for(cap, &source_name, &track) {
            Ok(bytes) => {
                let name = format!("{}.snap", cap.label);
                let path = out_dir.join(&name);
                if let Err(e) = std::fs::write(&path, &bytes) {
                    eprintln!("xtask corpus mint: write {}: {e}", path.display());
                    return 2;
                }
                eprintln!(
                    "capture {}: raw={} logic={} ({} bytes)",
                    cap.label,
                    cap.frame_raw,
                    cap.frame_logic,
                    bytes.len()
                );
                snap_files.push((cap.label, name, cap.blob.len()));
            }
            Err(e) => {
                eprintln!("xtask corpus mint: {e}");
                return 2;
            }
        }
    }
    // Lag map (per movie).
    let lag_name = format!("{stem}.lag.json");
    if let Err(e) = std::fs::write(out_dir.join(&lag_name), lag_json(&source_name, &outcome)) {
        eprintln!("xtask corpus mint: write {lag_name}: {e}");
        return 2;
    }
    // Skipped labels: manual (when minting all) + auto-but-unreached.
    if !explicit_labels {
        for rule in LABEL_RULES {
            if rule.detect == Detect::Manual {
                eprintln!("skip {}: detect=manual", rule.label);
                skipped.push((
                    rule.label,
                    format!("detect=manual: {}", rule.predicate_desc),
                ));
            }
        }
    }
    for rule in &active {
        if !outcome.captures.iter().any(|c| c.label == rule.label) {
            skipped.push((
                rule.label,
                format!(
                    "unreached: predicate never fired in {} frames ({})",
                    outcome.replayed, rule.predicate_desc
                ),
            ));
            eprintln!("skip {}: unreached in this movie", rule.label);
        }
    }
    let manifest = manifest_json(
        &source_name,
        track.len(),
        &outcome,
        &snap_files,
        &skipped,
        every,
    );
    if let Err(e) = std::fs::write(out_dir.join("mint-manifest.json"), &manifest) {
        eprintln!("xtask corpus mint: write mint-manifest.json: {e}");
        return 2;
    }
    println!(
        "mint ok: {} snapshot(s), {} skipped, lag {}/{} ({} stalls) -> {}",
        snap_files.len(),
        skipped.len(),
        outcome.logic_frames,
        outcome.replayed,
        outcome.skipped.len(),
        out_dir.display()
    );
    0
}

// ---------------------------------------------------------------------------
// Tests (run without ROM/movies via a scripted oracle stub).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use z2_verify::oracle::{Port, FRAME_PIXELS};

    /// Scripted [`Oracle`] test double: replays a canned RAM trajectory so
    /// predicate + determinism tests need no ROM.
    struct ScriptedPort {
        frames: Vec<[u8; 0x800]>,
        idx: usize,
        wram: Box<[u8; 0x2000]>,
    }

    impl ScriptedPort {
        fn new(frames: Vec<[u8; 0x800]>) -> Self {
            ScriptedPort {
                frames,
                idx: 0,
                wram: Box::new([0; 0x2000]),
            }
        }
        fn cur(&self) -> &[u8; 0x800] {
            &self.frames[self.idx.min(self.frames.len() - 1)]
        }
    }

    impl Port for ScriptedPort {
        fn step(&mut self, _input: u8) {
            self.idx += 1;
        }
        fn ram(&self) -> &[u8; 0x800] {
            self.cur()
        }
        fn wram(&self) -> &[u8; 0x2000] {
            &self.wram
        }
        fn oam(&self) -> &[u8; 256] {
            const Z: &[u8; 256] = &[0; 256];
            Z
        }
        fn palette(&self) -> &[u8; 32] {
            const Z: &[u8; 32] = &[0; 32];
            Z
        }
        fn frame_indexed(&self) -> &[u8; FRAME_PIXELS] {
            const Z: &[u8; FRAME_PIXELS] = &[0; FRAME_PIXELS];
            Z
        }
    }

    impl Oracle for ScriptedPort {
        fn load_rom(_: &Path) -> Result<Self, String> {
            Ok(ScriptedPort::new(vec![[0; 0x800]]))
        }
        fn save_state(&self) -> Vec<u8> {
            // Deterministic function of progress (not of host state).
            let mut v = b"X2SCRIPT".to_vec();
            v.extend_from_slice(&(self.idx as u64).to_le_bytes());
            v
        }
        fn load_state(&mut self, s: &[u8]) -> Result<(), String> {
            if s.len() != 16 || &s[..8] != b"X2SCRIPT" {
                return Err("bad scripted state".to_string());
            }
            self.idx = u64::from_le_bytes(s[8..16].try_into().unwrap()) as usize;
            Ok(())
        }
    }

    /// Trajectory: boot zeros → title ($11) at post-step frames 1-2 →
    /// Rauru (world 1, town 0) at frames 3-4 → shield ($77B) from frame 5.
    /// `$12` advances every frame except post-step frame 4 (stall).
    /// Nine entries so eight steps never hit the end clamp (a clamp would
    /// repeat `$12` and fake a stall).
    fn scripted_frames() -> Vec<[u8; 0x800]> {
        let mut v = vec![[0u8; 0x800]; 9];
        for (i, f) in v.iter_mut().enumerate() {
            f[A_FRAME_CTR as usize] = (i + 1) as u8;
        }
        // Post-step frame 4 stalls ($12 repeats frame 3's value).
        v[5][A_FRAME_CTR as usize] = v[4][A_FRAME_CTR as usize];
        v[2][A_MODE as usize] = MODE_TITLE;
        v[3][A_MODE as usize] = MODE_TITLE;
        for f in v.iter_mut().take(6).skip(4) {
            f[0x0707] = 1;
            f[A_TOWN as usize] = 0;
        }
        for f in v.iter_mut().take(9).skip(6) {
            f[0x077B] = 1;
        }
        v
    }

    fn auto_subset(labels: &[&str]) -> Vec<&'static LabelRule> {
        labels.iter().map(|l| rule_for(l).unwrap()).collect()
    }

    #[test]
    fn table_covers_label_plan() {
        assert_eq!(LABEL_RULES.len(), z2_verify::snapshot::LABEL_PLAN.len());
        for (rule, plan) in LABEL_RULES
            .iter()
            .zip(z2_verify::snapshot::LABEL_PLAN.iter())
        {
            assert_eq!(rule.label, plan.id);
            assert!(z2_verify::snapshot::label_is_known(rule.label));
        }
        let auto = LABEL_RULES
            .iter()
            .filter(|r| r.detect == Detect::Auto)
            .count();
        let manual = LABEL_RULES
            .iter()
            .filter(|r| r.detect == Detect::Manual)
            .count();
        assert_eq!(auto, 31);
        assert_eq!(manual, 11);
    }

    #[test]
    fn predicates_fire_only_on_their_state() {
        let mut ram = [0u8; 0x800];
        assert!(!p_boot_title(&ram));
        ram[A_MODE as usize] = MODE_TITLE;
        assert!(p_boot_title(&ram));
        ram[A_MODE as usize] = 0x0B;
        assert!(!p_boot_title(&ram));

        assert!(!p_town_rauru(&ram));
        ram[0x0707] = 1;
        ram[A_TOWN as usize] = 0;
        assert!(p_town_rauru(&ram));
        assert!(!p_town_ruto(&ram));
        assert!(!p_town_nabooru(&ram));

        assert!(!p_spell_shield(&ram));
        ram[0x077B] = 1;
        assert!(p_spell_shield(&ram));
        assert!(!p_spell_jump(&ram));

        assert!(!p_skill_downstab(&ram));
        ram[0x0796] = 0x10;
        assert!(p_skill_downstab(&ram));
        assert!(!p_skill_upstab(&ram));
        ram[0x0796] = 0x14;
        assert!(p_skill_upstab(&ram));

        assert!(!p_palace1(&ram));
        ram[0x0707] = 3;
        ram[A_REGION as usize] = 0;
        ram[A_PALACE as usize] = 0;
        assert!(p_palace1(&ram));
        assert!(!p_palace2(&ram));

        assert!(!p_ending(&ram));
        ram[A_STATE as usize] = STATE_CREDITS;
        assert!(p_ending(&ram));

        assert!(!p_never(&ram));
    }

    #[test]
    fn rising_edge_captures_once_per_label() {
        let rules = auto_subset(&["boot-title"]);
        let mut o = ScriptedPort::new(scripted_frames());
        // Title holds frames 2..=3 (post-step indexing); one capture.
        let out = run_mint(&mut o, &[0; 8], &rules, 0, 0);
        assert_eq!(out.captures.len(), 1);
        assert_eq!(out.captures[0].label, "boot-title");
        assert_eq!(out.captures[0].refires, 0);
    }

    #[test]
    fn lag_detector_flags_only_stalls() {
        let rules = auto_subset(&["boot-title"]);
        let mut o = ScriptedPort::new(scripted_frames());
        let out = run_mint(&mut o, &[0; 8], &rules, 0, 0);
        // Scripted $12: advances every frame except post-step frame 4.
        assert_eq!(out.skipped, vec![4]);
        assert_eq!(out.logic_frames, 7);
        // Capture logic frame accounts for the stall before it.
        let rauru = auto_subset(&["town-rauru-enter"]);
        let mut o = ScriptedPort::new(scripted_frames());
        let out = run_mint(&mut o, &[0; 8], &rauru, 0, 0);
        assert_eq!(out.captures.len(), 1);
        // Fired at post-step frame 3 (frame_raw 4); no stall before it.
        assert_eq!(out.captures[0].frame_raw, 4);
        assert_eq!(out.captures[0].frame_logic, 4);
    }

    #[test]
    fn detector_arms_on_first_tick() {
        let mut d = LagDetector::default();
        // Pre-NMI boot frames ($12 stuck at 0) are not lag.
        assert!(!d.feed(0, 0));
        assert!(!d.feed(1, 0));
        assert!(!d.feed(2, 0));
        assert_eq!(d.boot_frames, 3);
        // First advance arms; the arming frame itself is not lag.
        assert!(!d.feed(3, 1));
        // Afterwards a repeat is lag, an advance is not.
        assert!(d.feed(4, 1));
        assert!(!d.feed(5, 2));
        assert!(d.feed(6, 2));
    }

    #[test]
    fn remint_is_deterministic() {
        let rules: Vec<&LabelRule> = LABEL_RULES
            .iter()
            .filter(|r| r.detect == Detect::Auto)
            .collect();
        let run_once = || {
            let mut o = ScriptedPort::new(scripted_frames());
            let out = run_mint(&mut o, &[0xA5; 8], &rules, 2, 0);
            let snaps: Vec<Vec<u8>> = out
                .captures
                .iter()
                .map(|c| snapshot_for(c, "scripted.bk2", &[0xA5; 8]).unwrap())
                .collect();
            let files: Vec<(&'static str, String, usize)> = out
                .captures
                .iter()
                .map(|c| (c.label, format!("{}.snap", c.label), c.blob.len()))
                .collect();
            let skipped: Vec<(&'static str, String)> = vec![];
            let manifest = manifest_json("scripted.bk2", 8, &out, &files, &skipped, 2);
            (snaps, manifest, lag_json("scripted.bk2", &out))
        };
        let (snaps_a, manifest_a, lag_a) = run_once();
        let (snaps_b, manifest_b, lag_b) = run_once();
        assert_eq!(snaps_a, snaps_b);
        assert_eq!(manifest_a, manifest_b);
        assert_eq!(lag_a, lag_b);
        // Multi-label: title + Rauru + shield all fired.
        assert_eq!(snaps_a.len(), 3);
        // Every-N checkpoints recorded deterministically.
        assert!(manifest_a.contains("\"frame_raw\": 2"));
    }

    #[test]
    fn snapshot_bytes_decode_as_v2_with_blob() {
        let mut o = ScriptedPort::new(scripted_frames());
        let rules = auto_subset(&["boot-title"]);
        let out = run_mint(&mut o, &[0; 8], &rules, 0, 0);
        let bytes = snapshot_for(&out.captures[0], "scripted.bk2", &[0; 8]).unwrap();
        let back = Snapshot::decode(&bytes).unwrap();
        assert_eq!(back.version, z2_verify::snapshot::SNAPSHOT_VERSION);
        assert_eq!(back.state_source, BlobLayoutV1::OracleV1);
        assert_eq!(back.label, "boot-title");
        assert!(!back.oracle_blob.is_empty());
        assert_eq!(back.input_history, vec![0; 8]);
    }

    #[test]
    fn json_escape_handles_quotes_and_controls() {
        let mut s = String::new();
        json_escape("a\"b\\c\n\x01d", &mut s);
        assert_eq!(s, "\"a\\\"b\\\\c\\n\\u0001d\"");
        let lag = lag_json(
            "we\"ird.bk2",
            &MintOutcome {
                captures: vec![],
                skipped: vec![3, 9],
                replayed: 10,
                logic_frames: 8,
                boot_frames: 0,
                periodic: vec![],
            },
        );
        assert!(lag.contains("\"raw_frames\": 10"));
        assert!(lag.contains("\"skipped\": [3, 9]"));
    }
}
