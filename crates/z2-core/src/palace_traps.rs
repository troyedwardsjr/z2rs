//! Palace trap shims + registration.
//!
//! `Game`-shim layer over the pure [`crate::palace`] module.
//! Each `fn(&mut Game)` reads its inputs from
//! `game.ram`/`game.wram`, calls a pure helper, writes the results back.
//! Boss AI delegates to the [`crate::enemy_boss`] kernels (read-only);
//! item classification delegates to [`crate::player_magic::item_pickup`]
//! (read-only); the stone stamp reuses
//! [`crate::overworld_map::TILE_MAPPINGS`] via [`crate::palace::stone_tile`]
//! (read-only). Nothing is copied here.
//!
//! | shim | label | addr |
//! |---|---|---|
//! | [`pc_palace_entry`] | `bank4_Area_Pointers_Palaces_Type_A` / `..._Type_B_` | bank 4 `$8523` / `$A000` |
//! | [`pc_key_pickup`] | `LE7B5` | bank 7 `$E7B5` |
//! | [`pc_locked_door`] | `bank7_Enemy_Routines1_Locked_Door` | bank 7 `$D991` |
//! | [`pc_item_grant`] | `bank7_get_item` | bank 7 `$E771` |
//! | [`pc_crystal_place`] | `bank4_Related_to_placing_crystal_onto_statue` | bank 4 `$9AEB` |
//! | [`pc_crystal_flight`] | `bank4_Crystal_Flying_Up` | bank 4 `$9B2B` |
//! | [`pc_crystal_refill`] | `L9B47` | bank 4 `$9B47` |
//! | [`pc_stone_stamp`] | `bank7_Turn_Palaces_into_Stone_Bank_1` | bank 7 `$E01B` |
//! | [`pc_barrier_gate`] | `bank5_Enemy_Routines1_Electric_Barrier` | bank 5 `$A238` |
//! | [`pc_thunder_door`] | `bank5_Enemy_Routines1_Thunderbird` (door half) | bank 5 `$A359` |
//! | [`pc_thunder_wake`] | `bank5_Enemy_Routines1_Thunderbird` (wake half) | bank 5 `$A36B` |
//! | [`pc_darklink_setup`] | `L97DE` | bank 5 `$97DE` |
//! | [`pc_darklink_phase`] | `bank5_Enemy_Routines1_Dark_Link_Battle_Trigger` | bank 5 `$97C6` |
//! | [`pc_triforce`] | `LB39E` | bank 5 `$B3A7` |
//! | [`pc_ending`] | `STA/INC $076C` | bank 5 `$A6EC` |
//!
//! Banked code (all bank-4/5 `$8000-$BFFF` entries) is listed in
//! [`PALACE_TRAPS`] with `bank = None` and intentionally *not* registered:
//! 16-bit trap keys alias across `$8000-$BFFF` (see `traps.rs` M1 caveat).
//! Same rule as the banked entries in [`register_sideview_traps`](crate::sideview_traps::register_sideview_traps)
//! (only `Some(7)` fixed-bank entries register) and the all-banked
//! [`register_town_traps`](crate::town_traps::register_town_traps)
//! (data-only). The `pc_*` functions below are the directly-testable shim
//! bodies (like the `tw_*` functions in `town_traps.rs`): unit + snapshot
//! tests call them on synthetic `Game`s without mapper-aware routing.

use crate::enemy_boss;
use crate::game::Game;
use crate::palace as pal;
use crate::player_magic;

// ---------------------------------------------------------------------------
// Trap table.
// ---------------------------------------------------------------------------

/// Trap entry: (routine name, PRG bank, entry address).
pub type TrapEntry = (&'static str, Option<u8>, u16);

/// Registration table for `main` to wire into the trap dispatcher.
///
/// All bank-4/5 entries are `None` (aliasing caveat above) and skipped by
/// [`register_palace_traps`]; the table is the ledger-cited inventory that
/// `ports.toml` + `tests/palace_traps_tests.rs` cross-check.
pub const PALACE_TRAPS: &[TrapEntry] = &[
    ("bank4_Area_Pointers_Palaces_Type_A", None, 0x8523),
    ("bank4_Area_Data_for_Palaces_Type_A_Entrance", None, 0x82E5),
    (
        "bank4_Area_Data_for_Palaces_Type_A_Boss_Room_and_Crystal_Statue",
        None,
        0x831B,
    ),
    ("bank4_Area_Data_for_Palaces_Type_A", None, 0x861F),
    ("bank4_Area_Data_Palaces_Type_B0", None, 0xA0FC),
    ("bank4_Area_Data_Palaces_Type_B1", None, 0xA2F4),
    ("bank4_Area_Data_Palaces_Type_B2", None, 0xA440),
    ("bank4_Area_Data_Palaces_Type_B3", None, 0xA640),
    (
        "bank4_Enemy_Init_Routines_Crystal_Slot_and_Crystal",
        None,
        0x9A73,
    ),
    ("bank4_Enemy_Routines_Crystal", None, 0x9A8B),
    ("bank4_Enemy_Routines_Crystal_Slot", None, 0x9AD4),
    ("bank4_Related_to_placing_crystal_onto_statue", None, 0x9AEB),
    ("bank4_Crystal_Flying_Up", None, 0x9B2B),
    ("L9B47", None, 0x9B47),
    ("L9B56", None, 0x9B56),
    ("bank5_Area_Data_Great_Palace0", None, 0x834E),
    ("bank5_Area_Data_Great_Palace1", None, 0x861F),
    ("bank5_Area_Data_Great_Palace2", None, 0x8817),
    ("bank5_Area_Data_Great_Palace3", None, 0x89D8),
    (
        "bank5_Enemy_Init_Routines_Dark_Link_Battle_Trigger",
        None,
        0x9796,
    ),
    (
        "bank5_Enemy_Routines1_Dark_Link_Battle_Trigger",
        None,
        0x97C6,
    ),
    ("bank5_dark_link_AI_movement_maybe0", None, 0x98EB),
    ("bank5_Enemy_Routines1_Electric_Barrier", None, 0xA238),
    ("bank5_Enemy_Init_Routines_Thunderbird", None, 0xA33B),
    ("bank5_Enemy_Routines1_Thunderbird", None, 0xA359),
    ("bank5_Enemy_Routines2_Thunderbird", None, 0x9EBF),
    (
        "bank5_Enemy_Routines2_Dark_Link_Battle_Trigger",
        None,
        0xA472,
    ),
    ("bank5_change_tile_of_zelda_to_wake_her_up", None, 0x8D64),
    ("bank5_Ending_Text_Zelda_", None, 0x8DE1),
    ("bank5_Pointer_table_for_End_Credits", None, 0x9259),
    ("bank5_End_Credits", None, 0x927D),
    ("LB39E_triforce_emit", None, 0xB3A7),
];

/// Number of fixed-bank palace traps actually registered (none: all banked).
pub const PALACE_TRAP_COUNT: usize = 0;

// ---------------------------------------------------------------------------
// Small ram helpers (direct mirror access; no bus so this works with and
// without full CPU flag fidelity — flags are a reported gap).
// ---------------------------------------------------------------------------

fn r(ram: &[u8; 0x800], a: u16) -> u8 {
    ram[a as usize & 0x7FF]
}

fn w(ram: &mut [u8; 0x800], a: u16, v: u8) {
    let i = a as usize & 0x7FF;
    ram[i] = v;
}

/// Wrapping increment (6502 `INC`) at a mirrored address.
fn inc(ram: &mut [u8; 0x800], a: u16) {
    let i = a as usize & 0x7FF;
    ram[i] = ram[i].wrapping_add(1);
}

/// Enemy-slot selector (`$10`, clamped to the 6-slot window).
fn slot(game: &Game) -> usize {
    (r(&game.ram, 0x0010) as usize) % 6
}

// ---------------------------------------------------------------------------
// Palace shims (banked bodies, directly testable).
// ---------------------------------------------------------------------------

/// Palace map-set entry (bank 4 `$8523`/`$A000`): decode the `$56C` palace
/// number to its map set and record the `$0707` world byte in scratch
/// `$00` (1/2/5 → A/3, 3/4/6 → B/4). Unknown codes hold (no write).
pub fn pc_palace_entry(game: &mut Game) {
    let code = r(&game.ram, pal::ADDR_PALACE_CODE);
    if let Some(p) = pal::palace_of_number(code) {
        let set = pal::palace_map_set(p);
        w(&mut game.ram, 0x0000, pal::world_of_set(set));
    }
}

/// Key pickup (`LE7B5`, `$E7B5`): `INC $0793` (wraps) plus the boss-lock
/// clear (`$0728 = 0`) and palace-theme restore the listing performs for
/// boss-dropped keys.
pub fn pc_key_pickup(game: &mut Game) {
    if r(&game.ram, pal::ADDR_FREEZE) != 0 {
        w(&mut game.ram, pal::ADDR_FREEZE, 0);
    }
    let keys = pal::key_pickup(r(&game.ram, pal::ADDR_KEYS));
    w(&mut game.ram, pal::ADDR_KEYS, keys);
    // Palace-theme restore (`LDA #$02 : STA $EB`, `$E7B1`).
    w(&mut game.ram, 0x00EB, 0x02);
}

/// Locked-door use (`bank7_Enemy_Routines1_Locked_Door`, `$D991`): consumes
/// a key unless the magic key bypasses; records progress in `$AF,x`.
/// Touch/facing checks (`$D9BA-$D9E7`) are interp-only (gap: assumed
/// touching, like [`crate::sideview_traps::sv_locked_door`]).
pub fn pc_locked_door(game: &mut Game) {
    let s = slot(game) as u16;
    let magic = r(&game.ram, 0x078C) != 0;
    match pal::locked_door_consume(r(&game.ram, pal::ADDR_KEYS), magic) {
        None => {}
        Some(next) => {
            w(&mut game.ram, pal::ADDR_KEYS, next);
            w(&mut game.ram, 0x00AF + s, 1);
        }
    }
}

/// Item grant (`bank7_get_item`, `$E771`): classifies `$AF,x & $7F` via
/// [`player_magic::item_pickup`] and applies every arm — flag row
/// (`$0785,y |= 1`), key (`INC $0793` + `$0728` clear), containers
/// (`INC $0783`/`$0784`, the seven-magic Kasuto bit, pending refill
/// `code << 4` into `$070C`/`$070D`), jars (`$070C += …`), the doll
/// (`INC $0700`) and the flag-only pickups (child/trophy/medicine).
pub fn pc_item_grant(game: &mut Game) {
    let s = slot(game) as u16;
    let code = r(&game.ram, 0x00AF + s) & 0x7F;
    let boss_lock = r(&game.ram, pal::ADDR_FREEZE) != 0;
    let containers = r(&game.ram, player_magic::ADDR_MAG_CTR);
    match player_magic::item_pickup(code, containers, boss_lock) {
        player_magic::Pickup::Inventory { .. } => {
            let mut row = [0u8; 8];
            for (i, b) in row.iter_mut().enumerate() {
                *b = r(&game.ram, pal::ADDR_ITEMS + i as u16);
            }
            if pal::grant_palace_item(&mut row, code) {
                for (i, b) in row.iter().enumerate() {
                    w(&mut game.ram, pal::ADDR_ITEMS + i as u16, *b);
                }
            }
        }
        player_magic::Pickup::Key { unlock_boss } => {
            if unlock_boss {
                w(&mut game.ram, pal::ADDR_FREEZE, 0);
            }
            pc_key_pickup(game);
        }
        player_magic::Pickup::Container { magic, pending } => {
            // `INC $0775,x` with `X` = the item code lands on `$0783`
            // (magic) or `$0784` (life); the refill stages at `$06FE,x`,
            // which is `$070C`/`$070D` the same way (`$E7CD-$E7E8`).
            let (ctr_addr, add_addr) = if magic {
                (player_magic::ADDR_MAG_CTR, pal::ADDR_MAGIC_ADD)
            } else {
                (player_magic::ADDR_HEART_CTR, pal::ADDR_LIFE_ADD)
            };
            let (count, kasuto, _) =
                player_magic::container_pickup(r(&game.ram, ctr_addr), code, magic);
            w(&mut game.ram, ctr_addr, count);
            if kasuto {
                let v = r(&game.ram, player_magic::ADDR_SEVEN_FLAG)
                    | player_magic::SEVEN_CONTAINERS_BIT;
                w(&mut game.ram, player_magic::ADDR_SEVEN_FLAG, v);
            }
            w(&mut game.ram, add_addr, pending);
        }
        player_magic::Pickup::Jar { magic_add } => {
            // Both jar codes add their staged size to the magic refill
            // (`$E863-$E86C`); the add wraps like the 6502 `ADC`.
            let v = r(&game.ram, pal::ADDR_MAGIC_ADD).wrapping_add(magic_add);
            w(&mut game.ram, pal::ADDR_MAGIC_ADD, v);
        }
        player_magic::Pickup::Doll => {
            inc(&mut game.ram, player_magic::ADDR_LIVES);
        }
        player_magic::Pickup::Flag { byte, bit } => {
            let addr = 0x0700 | u16::from(byte);
            let v = r(&game.ram, addr) | bit;
            w(&mut game.ram, addr, v);
        }
    }
}

/// Crystal placement (`bank4_Related_to_placing_crystal_onto_statue`,
/// `$9AEB`): gates on decor/crystals/touch/grounded, then advances `$AF`,
/// locks input (`INC $DE`, `$80 = 3`), perches the crystal (`$2A = $A0`),
/// decrements `$0794`, and stores the carry-keeping slot index at
/// `$078C,y` (sounds `$EF = $08` / `$EB = $80`).
pub fn pc_crystal_place(game: &mut Game) {
    let s = slot(game) as u16;
    // Decor word `$C9` staged in scratch by the caller (ROM-gated PPU
    // mirror); synthetic tests set `$00C9` directly.
    let decor = r(&game.ram, 0x00C9);
    let left = r(&game.ram, pal::ADDR_CRYSTALS_LEFT);
    let touch = r(&game.ram, 0x00A8 + s) & 0x10 != 0;
    let grounded = r(&game.ram, 0x00A7) & 0x04 != 0;
    if !pal::crystal_place_gate(decor, left, touch, grounded) {
        return;
    }
    inc(&mut game.ram, 0x00AF + s);
    inc(&mut game.ram, 0x00DE);
    w(&mut game.ram, 0x0080, 0x03);
    w(&mut game.ram, 0x002A + s, pal::CRYSTAL_PLACE_Y);
    w(
        &mut game.ram,
        pal::ADDR_CRYSTALS_LEFT,
        pal::crystals_left_after(left),
    );
    w(&mut game.ram, 0x00EF, 0x08);
    w(&mut game.ram, 0x00EB, 0x80);
    let slot_idx = pal::crystal_slot_index(
        r(&game.ram, pal::ADDR_REGION),
        r(&game.ram, pal::ADDR_PALACE_CODE),
    );
    let addr = pal::crystal_flag_addr(slot_idx);
    w(&mut game.ram, addr, slot_idx);
}

/// Crystal flight (`bank4_Crystal_Flying_Up`, `$9B2B`): rises one px per
/// tick (`$81` mirrors Y); seating on `$62` latches `$0767`, advances
/// `$AF`, and plays the fanfare (`$EB = $40`, `$EC = $02`, Link speed 0).
pub fn pc_crystal_flight(game: &mut Game) {
    let s = slot(game) as u16;
    let (next, seated) = pal::crystal_flight_step(r(&game.ram, 0x002A + s));
    w(&mut game.ram, 0x002A + s, next);
    w(&mut game.ram, 0x0081 + s, next);
    if seated {
        w(&mut game.ram, pal::ADDR_CRYSTAL_TIMER, next);
        inc(&mut game.ram, 0x00AF + s);
        w(&mut game.ram, 0x00EB, 0x40);
        w(&mut game.ram, 0x00EC, 0x02);
        w(&mut game.ram, 0x0070, 0x00);
    }
}

/// Refill kick (`L9B47`, `$9B47`): stages `$FF` into `$070C`/`$070D` and
/// advances `$AF` when the boss key is not yet taken (`$07FB == 0`).
pub fn pc_crystal_refill(game: &mut Game) {
    let s = slot(game) as u16;
    if let Some(v) = pal::crystal_refill_pending(r(&game.ram, pal::ADDR_BEATEN)) {
        w(&mut game.ram, pal::ADDR_MAGIC_ADD, v);
        w(&mut game.ram, pal::ADDR_LIFE_ADD, v);
        inc(&mut game.ram, 0x00AF + s);
    }
}

/// Stone stamp (`bank7_Turn_Palaces_into_Stone_Bank_1`, `$E01B`): swaps
/// staged palace tiles (`$60-$63`) to rock (`$56-$59`) across the caller
/// window in `wram` (real pointer bytes at `$479F`/`$879F` are ROM-gated;
/// synthetic tests stage the window at `$7C00`). Records the swap count in
/// scratch `$02`.
pub fn pc_stone_stamp(game: &mut Game) {
    let mut n = 0u8;
    for i in 0x1C00..0x2000usize {
        let t = game.wram[i & 0x1FFF];
        let s = pal::stone_tile(t);
        if s != t {
            game.wram[i & 0x1FFF] = s;
            n = n.wrapping_add(1);
        }
    }
    w(&mut game.ram, 0x0002, n);
}

/// Barrier gate (`bank5_Enemy_Routines1_Electric_Barrier`, `$A238`): opens
/// the passage only with all six crystals placed; records `$AF = 3` +
/// `$80 = 3` on open (input lock `ROL $DE` is interp-only).
pub fn pc_barrier_gate(game: &mut Game) {
    let s = slot(game) as u16;
    let inp = pal::BarrierIn {
        aux: r(&game.ram, 0x00AF + s),
        crystals_left: r(&game.ram, pal::ADDR_CRYSTALS_LEFT),
        decor: r(&game.ram, 0x00C9),
        page: r(&game.ram, 0x003B),
        link_x: r(&game.ram, 0x004D),
    };
    if pal::barrier_gate(inp) == pal::BarrierGate::Open {
        w(&mut game.ram, 0x00AF + s, 0x03);
        w(&mut game.ram, 0x0080, 0x03);
    }
}

/// Thunderbird door (`bank5_Enemy_Routines1_Thunderbird` door half,
/// `$A359`): slams the doors (`INC $0728`, `$0504 = $90`) once Link leaves
/// page 0; combat proper delegates to the [`enemy_boss::thunderbird`]
/// kernel (wired by [`pc_thunder_wake`]).
pub fn pc_thunder_door(game: &mut Game) {
    let s = slot(game) as u16;
    match pal::thunder_door_gate(pal::ThunderDoorIn {
        frozen: r(&game.ram, pal::ADDR_FREEZE),
        page: r(&game.ram, pal::ADDR_PAGE),
    }) {
        pal::ThunderDoor::Slam => {
            inc(&mut game.ram, pal::ADDR_FREEZE);
            w(&mut game.ram, 0x0504 + s, pal::THUNDER_DOOR_TIMER);
        }
        pal::ThunderDoor::Combat | pal::ThunderDoor::Hold => {}
    }
}

/// Thunderbird wake (`$A36B`): Thunder striking (`$6E3F` sign via staged
/// scratch `$01`) wakes the bird through the
/// [`enemy_boss::thunderbird_awake`] kernel; records `$D9 = $0A` and the
/// `$80` music swap on the transition.
pub fn pc_thunder_wake(game: &mut Game) {
    let struck = (r(&game.ram, 0x0001) as i8) < 0;
    // Awake bit staged in scratch `$00` (caller mirrors `$6E3F` sign).
    let awake = r(&game.ram, 0x0000) != 0;
    let next = pal::thunder_wake(awake, struck);
    w(&mut game.ram, 0x0000, u8::from(next));
    if next && !awake {
        w(&mut game.ram, pal::ADDR_THUNDER_MOD, 0x0A);
        w(&mut game.ram, 0x00EB, 0x80);
    }
}

/// Thunderbird combat tick: palace-context pass-through to the
/// [`enemy_boss::thunderbird`] kernel (fire rate, flap, dormancy).
pub fn pc_thunder_tick(game: &mut Game) {
    let out = pal::thunder_tick(enemy_boss::ThunderbirdIn {
        awake: r(&game.ram, 0x0000) != 0,
        aux: r(&game.ram, 0x00AF + slot(game) as u16),
        frame: r(&game.ram, 0x0012),
        rng: r(&game.ram, 0x051B),
        thunder_struck: (r(&game.ram, 0x0001) as i8) < 0,
    });
    w(&mut game.ram, 0x0000, u8::from(out.awake));
    w(&mut game.ram, 0x0002, u8::from(out.fire));
    w(&mut game.ram, 0x0003, u8::from(out.flap));
}

/// Dark-Link setup (`L97DE`, `$97DE`): advances `$AF`; on later pages
/// freezes scrolling and — when grounded — plants Link (`$29 = $A0`,
/// anim 3) for the Triforce reveal.
pub fn pc_darklink_setup(game: &mut Game) {
    let s = slot(game) as u16;
    inc(&mut game.ram, 0x00AF + s);
    match pal::darklink_setup_gate(pal::DarkSetupIn {
        page: r(&game.ram, pal::ADDR_PAGE),
        grounded: r(&game.ram, 0x00A7) & 0x04 != 0,
    }) {
        pal::DarkSetup::Hold => {}
        pal::DarkSetup::Freeze => {
            inc(&mut game.ram, pal::ADDR_FREEZE);
            w(&mut game.ram, 0x0070, 0x00);
        }
        pal::DarkSetup::Plant => {
            inc(&mut game.ram, pal::ADDR_FREEZE);
            w(&mut game.ram, 0x0070, 0x00);
            w(&mut game.ram, 0x0029, 0xA0);
            w(&mut game.ram, 0x0080, 0x03);
        }
    }
}

/// Dark-Link phase dispatch (`$97C6` via `$63`): records the phase class in
/// scratch `$02` (0 setup, 1 triforce, 2 flash, 3 spawn, 4+ duel); the
/// flash arm additionally selects PPU macro `$0D` when `$074B == $81`.
/// Duel frames delegate to the [`enemy_boss::dark_link`] kernel (wired by
/// [`pc_darklink_duel`]).
pub fn pc_darklink_phase(game: &mut Game) {
    let phase = pal::dark_phase(r(&game.ram, pal::ADDR_DARK_PHASE));
    w(
        &mut game.ram,
        0x0002,
        match phase {
            pal::DarkPhase::Setup => 0,
            pal::DarkPhase::Triforce => 1,
            pal::DarkPhase::Flash => 2,
            pal::DarkPhase::Spawn => 3,
            pal::DarkPhase::Duel => 4,
        },
    );
    if phase == pal::DarkPhase::Flash && pal::darklink_spawn_gate(r(&game.ram, 0x074B)) {
        w(&mut game.ram, pal::ADDR_PPU_MACRO, pal::FLASH_SPAWN_MACRO);
    }
}

/// Dark-Link duel tick: palace-context pass-through to the
/// [`enemy_boss::dark_link`] kernel (mirror-drift, corner-crouch dice,
/// whiff punish — quirks preserved there).
pub fn pc_darklink_duel(game: &mut Game) {
    let out = pal::darklink_tick(enemy_boss::DarkLinkIn {
        link_speed: r(&game.ram, 0x0070),
        link_anim: r(&game.ram, 0x0080),
        link_y: r(&game.ram, 0x0029),
        self_y: r(&game.ram, 0x002A),
        frame: r(&game.ram, 0x0012),
        rng: r(&game.ram, 0x051B),
        atk: r(&game.ram, 0x0777),
    });
    w(&mut game.ram, 0x0002, out.speed);
    w(&mut game.ram, 0x0003, u8::from(out.crouch));
    w(&mut game.ram, 0x0004, u8::from(out.stab));
}

/// Triforce emit (`LB39E`, `$B3A7`): records the tile/attribute pair in
/// scratch `$02`/`$03` for the OAM layer.
pub fn pc_triforce(game: &mut Game) {
    let (tile, attr) = pal::triforce_emit();
    w(&mut game.ram, 0x0002, tile);
    w(&mut game.ram, 0x0003, attr);
}

/// Ending trigger (`STA/INC $076C`): Triforce claimed drives wake-Zelda
/// (`$076C = 3`); a second pulse rolls credits (`INC $076C` → 4).
/// Scratch `$02` mirrors the new state.
pub fn pc_ending(game: &mut Game) {
    let cur = r(&game.ram, pal::ADDR_GAME_STATE);
    let next = if cur == pal::STATE_INGAME {
        pal::wake_zelda()
    } else {
        pal::roll_credits(cur)
    };
    w(&mut game.ram, pal::ADDR_GAME_STATE, next);
    w(&mut game.ram, 0x0002, next);
}

// ---------------------------------------------------------------------------
// Registration.
// ---------------------------------------------------------------------------

/// Register palace traps on `game`.
///
/// Every [`PALACE_TRAPS`] entry is banked (`None`) and skipped (aliasing
/// caveat). Idempotent. The `pc_*` shims above are the testable surface.
pub fn register_palace_traps(game: &mut Game) {
    // Data-only today: every palace entry is slot-swapped bank 4/5 (None),
    // skipped like the banked SIDEVIEW_TRAPS/OVERWORLD_TRAPS entries
    // (aliasing caveat). Keep the loop over the table so a future
    // mapper-aware router has one surface to extend; touch `game` so the
    // no-op stays a real (idempotent) registration pass.
    let before = game.traps.len();
    for (_name, bank, _addr) in PALACE_TRAPS {
        if bank.is_some() {
            continue;
        }
    }
    debug_assert_eq!(game.traps.len(), before);
    let _ = (
        pc_palace_entry as fn(&mut Game),
        pc_key_pickup as fn(&mut Game),
        pc_locked_door as fn(&mut Game),
        pc_item_grant as fn(&mut Game),
        pc_crystal_place as fn(&mut Game),
        pc_crystal_flight as fn(&mut Game),
        pc_crystal_refill as fn(&mut Game),
        pc_stone_stamp as fn(&mut Game),
        pc_barrier_gate as fn(&mut Game),
        pc_thunder_door as fn(&mut Game),
        pc_thunder_wake as fn(&mut Game),
        pc_thunder_tick as fn(&mut Game),
        pc_darklink_setup as fn(&mut Game),
        pc_darklink_phase as fn(&mut Game),
        pc_darklink_duel as fn(&mut Game),
        pc_triforce as fn(&mut Game),
        pc_ending as fn(&mut Game),
    );
}
