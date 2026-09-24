//! `z2-core`: CPU-adjacent core engine logic, typed RAM-map accessors,
//! and the `GameFacts` exporter.

/// Bank-7 fixed-core ports: reset/NMI/main-loop/input,
/// MMC1 switching, RNG/timers, PPU update queue, typed mode dispatch.
/// Gated behind `interp` with the interpreter (trap + bus access).
#[cfg(feature = "interp")]
pub mod bank7_common;
#[cfg(feature = "interp")]
pub mod bank7_dispatch;
#[cfg(feature = "interp")]
pub mod bank7_input;
#[cfg(feature = "interp")]
pub mod bank7_mem;
#[cfg(feature = "interp")]
pub mod bank7_mmc1;
#[cfg(feature = "interp")]
pub mod bank7_mode;
#[cfg(feature = "interp")]
pub mod bank7_nmi;
#[cfg(feature = "interp")]
pub mod bank7_ppu_queue;
#[cfg(feature = "interp")]
pub mod bank7_reset;
#[cfg(feature = "interp")]
pub mod bank7_timers;
#[cfg(feature = "interp")]
pub mod bank7_traps;
/// Boot-path logic: power-on reset, title-intro sequencing, file-select
/// entry, new-game init.
///
/// Pure logic over explicit params (no `Game` dep); trap entry points are
/// listed in [`boot_traps::BOOT_TRAPS`].
pub mod boot;
/// Boot-path trap shims.
///
/// Gated behind `interp` with the interpreter (needs `Game` + `TrapTable`).
/// Only fixed-bank (`Some(7)`) JSR-entered routines register; banked ones
/// stay data-only in [`BOOT_TRAPS`](boot_traps::BOOT_TRAPS) (aliasing
/// caveat) and JMP-only fixed entries are skipped by name.
#[cfg(feature = "interp")]
pub mod boot_traps;
/// Embedded 2A03 interpreter + MMC1 + PPU/APU façades.
///
/// Gated behind the `interp` feature so release builds compile it out
/// (`cargo build --no-default-features`); `game`/`ram`/`facts` stay available.
/// Local two-player co-op (second Link, flag-gated, off by default).
#[cfg(feature = "interp")]
pub mod coop;
#[cfg(feature = "interp")]
pub mod cpu;
/// Enemy engine: generic actor framework.
///
/// Pure logic over explicit params (no `Game` dep); trap entry points are
/// listed in [`enemy_traps::ENEMY_TRAPS`].
pub mod enemy;
/// Enemy AI per family: walker/jumper/flyer/generator/shooter/statue.
///
/// Pure logic over explicit params (no `Game` dep); consumed by
/// [`enemy_traps`] shims.
pub mod enemy_ai;
/// Boss behaviors, one section per boss.
///
/// Pure logic over explicit params (no `Game` dep); consumed by
/// [`enemy_traps`] shims.
pub mod enemy_boss;
/// Enemy data tables as ROM offsets.
///
/// Descriptors only — bytes are loaded at runtime, never copied here.
pub mod enemy_data;
/// Enemy trap shims.
///
/// Gated behind `interp` with the interpreter (needs `Game` + `TrapTable`).
#[cfg(feature = "interp")]
pub mod enemy_traps;
pub mod facts;
pub mod game;
/// Overworld systems: RLE maps, encounters, transitions.
///
/// Pure logic over explicit params (no `Game` dep); trap entry points are
/// listed in [`overworld::OVERWORLD_TRAPS`] for `xtask verify --dut game`.
pub mod overworld;
pub mod overworld_encounter;
pub mod overworld_map;
pub mod overworld_transition;
/// Palace engine: index/map sets, keys, item rooms, crystals, crumble,
/// Great Palace, Thunderbird/Dark Link triggers, ending.
///
/// Pure logic over explicit params (no `Game` dep); trap entry points are
/// listed in [`palace_traps::PALACE_TRAPS`].
pub mod palace;
/// Palace trap shims.
///
/// Gated behind `interp` with the interpreter (needs `Game` + `TrapTable`).
/// All bank-4/5 entries are data-only today (aliasing caveat); `pc_*`
/// shims are directly testable.
#[cfg(feature = "interp")]
pub mod palace_traps;
/// Player engine: movement, jumping, sword/thrusts, damage, fairy.
///
/// Pure logic over explicit params (no `Game` dep); trap entry points are
/// listed in [`player_traps::PLAYER_TRAPS`].
pub mod player;
/// Player magic/items/leveling/death.
///
/// Pure logic over explicit params (no `Game` dep); consumed by
/// [`player_traps`] shims.
pub mod player_magic;
/// Player trap shims.
///
/// Gated behind `interp` with the interpreter (needs `Game` + `TrapTable`).
#[cfg(feature = "interp")]
pub mod player_traps;
/// Real-PPU binding for the interpreter bus.
///
/// Gated behind `interp` with the interpreter (implements `cpu::PpuBus`).
#[cfg(feature = "interp")]
pub mod ppu_bind;
pub mod ram;
/// Save-slot RAM logic: new-game init, presence, name entry, lives/deaths,
/// continue/save/retry.
///
/// Pure logic over explicit params and slices (no `Game` dep); byte-protocol
/// SRAM work lives in [`save_format`]; trap entry points are listed in
/// [`title_traps::TITLE_TRAPS`].
pub mod save;
/// Battery-save format: SRAM layout, writer/loader, validity + recovery.
///
/// Pure logic over explicit slices (no `Game` dep); consumed by
/// [`title_traps`] shims.
pub mod save_format;
/// Sideview engine: loader, scrolling, collision, doors/elevators, spawning.
///
/// Pure logic over explicit params (no `Game` dep); trap shims live in
/// `sideview_traps` (interp-gated, like `bank7_traps`).
pub mod sideview;
pub mod sideview_area;
pub mod sideview_collision;
pub mod sideview_scroll;
pub mod sideview_spawn;
/// Sideview + overworld trap shims.
///
/// Gated behind `interp` with the interpreter (needs `Game` + `TrapTable`).
#[cfg(feature = "interp")]
pub mod sideview_traps;
/// Save states for rollback netplay (`GameState`, `Game::save_state` /
/// `Game::load_state`, versioned byte images).
///
/// Gated behind `interp` with the interpreter (captures CPU/mapper/PPU state).
#[cfg(feature = "interp")]
pub mod state;
/// Title / intro / file-select / game-over / ending tables + descriptors.
///
/// Pure descriptors over explicit params (no `Game` dep); text bytes are
/// caller-loaded slices, never embedded here. Trap entry points are
/// listed in [`title_traps::TITLE_TRAPS`].
pub mod title;
/// Title / file-select / name-entry / death / ending state machines.
///
/// Pure logic over explicit params (no `Game` dep); consumed by
/// [`title_traps`] shims.
pub mod title_flow;
/// Title/save/death/game-over trap shims.
///
/// Gated behind `interp` with the interpreter (needs `Game` + `TrapTable`).
/// Only fixed-bank (`Some(7)`) entries register; banked ones stay
/// data-only in [`TITLE_TRAPS`](title_traps::TITLE_TRAPS) (aliasing caveat).
#[cfg(feature = "interp")]
pub mod title_traps;
/// Town engine: 8 towns, spell order, ROM descriptors.
///
/// Pure logic over explicit params (no `Game` dep); trap entry points are
/// listed in [`town_traps::TOWN_TRAPS`].
pub mod town;
/// Town dialog box + text renderer + index resolution.
///
/// Pure logic over explicit params (no `Game` dep); text bytes are
/// caller-loaded `DIALOG` slices, never embedded here.
pub mod town_dialog;
/// Town NPCs: lists, walking AI, talk interaction.
///
/// Pure logic over explicit params (no `Game` dep); consumed by
/// [`town_traps`] shims.
pub mod town_npc;
/// Town quests: wise men, healers, thrusts, side quests, Kasuto, Bagu,
/// River Devil.
///
/// Pure logic over explicit params (no `Game` dep); consumed by
/// [`town_traps`] shims.
pub mod town_quest;
/// Town trap shims.
///
/// Gated behind `interp` with the interpreter (needs `Game` + `TrapTable`).
/// All bank-3 entries are data-only today (aliasing caveat); `tw_*`
/// shims are directly testable.
#[cfg(feature = "interp")]
pub mod town_traps;
/// Routine trap table, trap log and divergence type.
///
/// Gated behind `interp` with the interpreter (see `cpu`).
#[cfg(feature = "interp")]
pub mod traps;
/// Wide gameplay: enemies spawn and live in the widescreen margins
/// (optional, off by default, never part of verification).
#[cfg(feature = "interp")]
pub mod wide_gameplay;
/// Widescreen margin provider (level RAM / overworld map decoders).
pub mod wide_margins;
/// Widescreen margin sprites: side-view objects outside the window
/// (display-only `$EF11` observer).
#[cfg(feature = "interp")]
pub mod wide_sprites;
