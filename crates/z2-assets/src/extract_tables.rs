//! Section table for the asset extractor.
//!
//! Every record is a raw slice of the user's ROM image: `(file_offset, len)`
//! pairs with no decoding (RLE stays RLE — the game decodes it, because the
//! port must reproduce the decoder).
//!
//! # Offset convention
//!
//! `file_off` is the offset in the **headered iNES file** (16-byte `NES\x1a`
//! header included). This matches the Data Crystal ROM
//! map, and the `; 0xFILE $CPU` comments in `third_party/z2disassembly`.
//! [`crate::extract`] converts to body-relative indexes by subtracting the
//! 16-byte header before slicing the verified body returned by
//! [`crate::rom::open`].
//!
//! # Provenance key
//!
//! - `DC` — Data Crystal Zelda II ROM map / TBL
//!   (`datacrystal.tcrf.net`, "Overworld Map Data", "PPU Data",
//!   "Beginning Values", "Sound Data", text table `0xE390-0xEFCC`).
//! - `z2dis:<file>:<label>` — label in the vendored disassembly
//!   (`third_party/z2disassembly/src/prgN.asm`, FiendsOfTheElements, CC0).
//! - `DW` — Dwedit's sideview-area notes quoted on the Data Crystal ROM map
//!   (bank layout, pointer-table slots, SRAM copy semantics).
//!
//! Core-only (`core` + `alloc`): no `std` paths in this file so the extractor
//! core stays `no_std`-friendly and `wasm32`-compatible.

/// One raw record of the ROM image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionDef {
    /// Stable section id (see [`section_id`).
    pub id: u16,
    /// Short machine name (also the `assets.bin` lookup key by table order).
    pub name: &'static str,
    /// Offset in the headered iNES file (16-byte header included).
    pub file_off: u32,
    /// Length in bytes.
    pub len: u32,
    /// Disassembly / ROM-map label for this record.
    pub label: &'static str,
}

/// Stable `assets.bin` section ids, grouped by kind:
///
/// `0x01xx` CHR · `0x02xx` palettes/PPU · `0x03xx` overworld ·
/// `0x04xx..0x08xx` sideview banks 1-5 · `0x09xx` scalar tables ·
/// `0x0Axx` bank-6 music.
pub mod section_id {
    pub const CHR_ROM: u16 = 0x0100;

    pub const PAL_EXTERIOR: u16 = 0x0200;
    pub const PAL_INTERIOR: u16 = 0x0201;
    pub const PAL_OVERWORLD: u16 = 0x0202;
    pub const PAL_OVERWORLD_SPR: u16 = 0x0203;
    pub const PALACE_GFX_SET: u16 = 0x0204;
    pub const PALACE_PAL_PTR: u16 = 0x0205;

    pub const PPU_OVERWORLD: u16 = 0x0210;
    pub const PPU_WEST_FIELD: u16 = 0x0211;
    pub const PPU_EAST_FIELD: u16 = 0x0212;
    pub const PPU_TOWN_WIN: u16 = 0x0213;
    pub const PPU_TOWN_CLOUD: u16 = 0x0214;
    pub const PPU_TOWN_FLOOR: u16 = 0x0215;
    pub const PPU_TOWN_SKY: u16 = 0x0216;
    pub const PPU_TOWN_GROUND: u16 = 0x0217;

    pub const MAP_WEST: u16 = 0x0300;
    pub const MAP_DM: u16 = 0x0301;
    pub const MAP_EAST: u16 = 0x0302;
    pub const MAP_MAZE: u16 = 0x0303;

    pub const AREAS_WEST: u16 = 0x0310;
    pub const AREAS_DM: u16 = 0x0311;
    pub const AREAS_EAST: u16 = 0x0312;
    pub const AREAS_MAZE: u16 = 0x0313;

    pub const PATCH_WEST: u16 = 0x0320;
    pub const PATCH_EAST: u16 = 0x0321;

    pub const OWPTR_B1: u16 = 0x0330;
    pub const OWPTR_B2: u16 = 0x0331;

    pub const REGION_SELECTOR: u16 = 0x0340;
    pub const AREA_ENEMY_PTRSTAB: u16 = 0x0341;
    pub const KEY_AREAS: u16 = 0x0342;

    pub const SB1_MAPPTR_A: u16 = 0x0400;
    pub const SB1_ENEMYPTR_A: u16 = 0x0401;
    pub const SB1_CONN_A: u16 = 0x0402;
    pub const SB1_ENEMIES: u16 = 0x0403;
    pub const SB1_MAPPTR_B: u16 = 0x0404;
    pub const SB1_ENEMYPTR_B: u16 = 0x0405;
    pub const SB1_CONN_B: u16 = 0x0406;
    pub const SB1_BEHIND: u16 = 0x0407;
    pub const PRG_BANK_1: u16 = 0x04F0;

    pub const SB2_MAPPTR_A: u16 = 0x0500;
    pub const SB2_ENEMYPTR_A: u16 = 0x0501;
    pub const SB2_CONN_A: u16 = 0x0502;
    pub const SB2_ENEMIES: u16 = 0x0503;
    pub const SB2_MAPPTR_B: u16 = 0x0504;
    pub const SB2_ENEMYPTR_B: u16 = 0x0505;
    pub const SB2_CONN_B: u16 = 0x0506;
    pub const SB2_BEHIND: u16 = 0x0507;
    pub const PRG_BANK_2: u16 = 0x05F0;

    pub const SB3_MAPPTR_A: u16 = 0x0600;
    pub const SB3_ENEMYPTR_A: u16 = 0x0601;
    pub const SB3_CONN_A: u16 = 0x0602;
    pub const SB3_ENEMIES: u16 = 0x0603;
    pub const SB3_BGPTR: u16 = 0x0604;
    pub const DLG_IDX2: u16 = 0x0610;
    pub const DLG_IDX3: u16 = 0x0611;
    pub const DLG_IDX4: u16 = 0x0612;
    pub const DIALOG: u16 = 0x0613;
    pub const PRG_BANK_3: u16 = 0x06F0;

    pub const SB4_MAPPTR_A: u16 = 0x0700;
    pub const SB4_ENEMYPTR_A: u16 = 0x0701;
    pub const SB4_CONN_A: u16 = 0x0702;
    pub const SB4_ENEMIES: u16 = 0x0703;
    pub const SB4_MAPPTR_B: u16 = 0x0704;
    pub const SB4_ENEMYPTR_B: u16 = 0x0705;
    pub const SB4_CONN_B: u16 = 0x0706;
    pub const SB4_BEHIND: u16 = 0x0707;
    pub const PRG_BANK_4: u16 = 0x07F0;

    pub const SB5_MAPPTR_A: u16 = 0x0800;
    pub const SB5_ENEMYPTR_A: u16 = 0x0801;
    pub const SB5_CONN_A: u16 = 0x0802;
    pub const SB5_ENEMIES: u16 = 0x0803;
    pub const SB5_BEHIND: u16 = 0x0807;
    pub const PRG_BANK_5: u16 = 0x08F0;

    pub const GAMEOVER_TEXT: u16 = 0x0900;
    pub const LEVELUP_TILES: u16 = 0x0901;
    pub const SPELLMENU_TEXT: u16 = 0x0902;
    pub const SPELL_COST: u16 = 0x0903;
    pub const SPELL_FX: u16 = 0x0904;
    pub const SPELL_PTR: u16 = 0x0905;
    pub const EXP_HI: u16 = 0x0906;
    pub const EXP_LO: u16 = 0x0907;

    pub const LEVEL_BG_GRASS: u16 = 0x0910;
    pub const LEVEL_BG_FOREST: u16 = 0x0911;
    pub const LEVEL_BG_BAG: u16 = 0x0912;

    pub const BEGIN_VALUES: u16 = 0x0920;
    pub const MAINMENU_CURSOR: u16 = 0x0921;

    pub const LIVES: u16 = 0x0930;
    pub const ITEM_PRESENCE: u16 = 0x0931;
    pub const DROP_PROB: u16 = 0x0932;
    pub const DEFEAT_COUNT: u16 = 0x0933;
    pub const VULN_BITS: u16 = 0x0934;
    pub const ITEM_SPR_MAP: u16 = 0x0935;
    pub const ITEM_TILE_MAP: u16 = 0x0936;
    pub const SAVE_SIG: u16 = 0x0937;

    pub const MUSIC_NOTETABLES: u16 = 0x0A00;
    pub const MUSIC_SONG0: u16 = 0x0A01;
    pub const MUSIC_SONG1: u16 = 0x0A02;
    pub const MUSIC_SONG2: u16 = 0x0A03;
    pub const MUSIC_SONG3: u16 = 0x0A04;
    pub const MUSIC_SONG4: u16 = 0x0A05;
}

use section_id::*;

/// The full extraction table, in deterministic `assets.bin` order.
///
/// Lengths for ranged records follow the Data Crystal "from-to" convention
/// (inclusive end, `len = end - start + 1`); label-bounded records run up to
/// the next disassembly label's first data byte.
pub const SECTION_TABLE: &[SectionDef] = &[
    // ---- 128 KiB CHR ROM (32 x 4 KiB pages) ----
    SectionDef {
        id: CHR_ROM,
        name: "chr_rom",
        file_off: 0x020010,
        len: 0x020000,
        label: "CHR-ROM banks (iNES CHR region; rip-chr.sh input)",
    },
    // ---- Palettes ----
    SectionDef {
        id: PAL_EXTERIOR,
        name: "pal_exterior",
        file_off: 0x010480,
        len: 96,
        label: "DC: Palettes for Outside the 6 Palaces ($10480-$104DF)",
    },
    SectionDef {
        id: PAL_INTERIOR,
        name: "pal_interior",
        file_off: 0x013F10,
        len: 96,
        label: "DC: Palettes for the 6 Palaces Interiors ($13F10-$13F6F)",
    },
    SectionDef {
        id: PAL_OVERWORLD,
        name: "pal_overworld",
        file_off: 0x01C46B,
        len: 16,
        label: "DC: Palette for Overworld ($1C46B-$1C47A)",
    },
    SectionDef {
        id: PAL_OVERWORLD_SPR,
        name: "pal_overworld_spr",
        file_off: 0x01C47B,
        len: 16,
        label: "DC: Sprite pallet set for Overworld ($1C47B-$1C48A)",
    },
    SectionDef {
        id: PALACE_GFX_SET,
        name: "palace_gfx_set",
        file_off: 0x01CD3A,
        len: 11,
        label: "DC: Graphics Set per palace ($1CD3A-$1CD44); \
                 z2dis:prg7:bank7_Graphics_Bank_for_Palaces_Palace_1..",
    },
    SectionDef {
        id: PALACE_PAL_PTR,
        name: "palace_pal_ptr",
        file_off: 0x01CD45,
        len: 11,
        label: "DC: Palette Pointer per palace ($1CD45-$1CD4F)",
    },
    // ---- PPU tile-mapping data ----
    SectionDef {
        id: PPU_OVERWORLD,
        name: "ppu_overworld",
        file_off: 0x0007B3,
        len: 0x40,
        label: "DC: PPU data for Towns/Caves/.../River Devil ($7B3-$7F2)",
    },
    SectionDef {
        id: PPU_WEST_FIELD,
        name: "ppu_west_field",
        file_off: 0x0044EB,
        len: 4,
        label: "DC: PPU data for Grass on Battlefield, West ($44EB-$44EE)",
    },
    SectionDef {
        id: PPU_EAST_FIELD,
        name: "ppu_east_field",
        file_off: 0x0084DF,
        len: 16,
        label: "DC: PPU data for Leaves/Hanging Leaves/Grass, East ($84DF-$84EE)",
    },
    SectionDef {
        id: PPU_TOWN_WIN,
        name: "ppu_town_win",
        file_off: 0x00C3F6,
        len: 4,
        label: "DC: PPU data for Top/Bottom of Exterior Windows ($C3F6-$C3FD)",
    },
    SectionDef {
        id: PPU_TOWN_CLOUD,
        name: "ppu_town_cloud",
        file_off: 0x00C44E,
        len: 20,
        label: "DC: PPU data for Clouds/tunnel (left/right/angle/wall) ($C44E-$C461)",
    },
    SectionDef {
        id: PPU_TOWN_FLOOR,
        name: "ppu_town_floor",
        file_off: 0x00C4E2,
        len: 4,
        label: "DC: PPU data for floors inside buildings ($C4E2-$C4E5)",
    },
    SectionDef {
        id: PPU_TOWN_SKY,
        name: "ppu_town_sky",
        file_off: 0x00C6B2,
        len: 24,
        label: "DC: PPU data for Sky/Basements/Bushs ($C6B2-$C6C9)",
    },
    SectionDef {
        id: PPU_TOWN_GROUND,
        name: "ppu_town_ground",
        file_off: 0x00C6FE,
        len: 12,
        label: "DC: PPU data for Ground/Walls in Basements ($C6FE-$C709)",
    },
    // ---- RLE overworld maps (RAW; game decodes) ----
    SectionDef {
        id: MAP_WEST,
        name: "map_west",
        file_off: 0x00506C,
        len: 801,
        label: "DC: West Hyrule ($506C-$538C); \
                 z2dis:prg1:bank1_West_Hyrule_Overworld_Map_Data",
    },
    SectionDef {
        id: MAP_DM,
        name: "map_dm",
        file_off: 0x00665C,
        len: 743,
        label: "DC: Death Mountain ($665C-$6942); \
                 z2dis:prg1:bank1_Death_Mountain_Overworld_Map_Data",
    },
    SectionDef {
        id: MAP_EAST,
        name: "map_east",
        file_off: 0x009056,
        len: 794,
        label: "DC: East Hyrule ($9056-$936F); \
                 z2dis:prg2:bank2_East_Hyrule_Overworld_Map_Data",
    },
    SectionDef {
        id: MAP_MAZE,
        name: "map_maze",
        file_off: 0x00A65C,
        len: 743,
        label: "DC: Maze Island ($A65C-$A942); \
                 z2dis:prg2:bank2_Maze_Island_Overworld_Map_Data",
    },
    // ---- Overworld areas (63 areas x 4 strided bytes: Y / X / map / world) ----
    SectionDef {
        id: AREAS_WEST,
        name: "areas_west",
        file_off: 0x00462F,
        len: 252,
        label: "DC: Overworld Areas, West Hyrule ($462F-$472A)",
    },
    SectionDef {
        id: AREAS_DM,
        name: "areas_dm",
        file_off: 0x00610C,
        len: 252,
        label: "DC: Overworld Areas, Death Mountain ($610C-$6207)",
    },
    SectionDef {
        id: AREAS_EAST,
        name: "areas_east",
        file_off: 0x00862F,
        len: 252,
        label: "DC: Overworld Areas, East Hyrule ($862F-$8727)",
    },
    SectionDef {
        id: AREAS_MAZE,
        name: "areas_maze",
        file_off: 0x00A10C,
        len: 252,
        label: "DC: Overworld Areas, Maze Island ($A10C-$A207)",
    },
    // ---- Palace-completion SRAM patch pointers ----
    SectionDef {
        id: PATCH_WEST,
        name: "patch_west",
        file_off: 0x00479F,
        len: 8,
        label: "DC: Palace Pointers, West/DM ($479F-$47A5)",
    },
    SectionDef {
        id: PATCH_EAST,
        name: "patch_east",
        file_off: 0x00879F,
        len: 8,
        label: "DC: Palace Pointers, East/Maze ($879F-$87A7)",
    },
    // ---- Overworld map pointer tables ----
    SectionDef {
        id: OWPTR_B1,
        name: "owptr_b1",
        file_off: 0x004518,
        len: 4,
        label: "z2dis:prg1:Pointer_table_for_Overworld_Map_Data",
    },
    SectionDef {
        id: OWPTR_B2,
        name: "owptr_b2",
        file_off: 0x008518,
        len: 4,
        label: "z2dis:prg2:bank2_Pointer_table_for_Overworld_Map_Data",
    },
    // ---- Region / key-area index tables (bank 7) ----
    SectionDef {
        id: REGION_SELECTOR,
        name: "region_selector",
        file_off: 0x01CD37,
        len: 8,
        label: "z2dis:prg7:bank7_Region_Overworld_Map_Pointer_Offset_Selector",
    },
    SectionDef {
        id: AREA_ENEMY_PTRSTAB,
        name: "area_enemy_ptrstab",
        file_off: 0x01C4D3,
        len: 8,
        label: "z2dis:prg7:bank7_Pointer_table_for_Area_and_Enemy_Pointers",
    },
    SectionDef {
        id: KEY_AREAS,
        name: "key_areas",
        file_off: 0x01CD33,
        len: 4,
        label: "z2dis:prg7:bank7_Pointer_table_for_Key_Areas_Data",
    },
    // ---- Sideview bank 1 (West Hyrule / Death Mountain) ----
    SectionDef {
        id: SB1_MAPPTR_A,
        name: "sb1_mapptr_a",
        file_off: 0x004533,
        len: 126,
        label: "z2dis:prg1:bank1_Area_Pointers_West_Hyrule ($8523, 63 maps)",
    },
    SectionDef {
        id: SB1_ENEMYPTR_A,
        name: "sb1_enemyptr_a",
        file_off: 0x0045B1,
        len: 126,
        label: "z2dis:prg1:bank1_Enemy_Pointers__West_Hyrule ($85A1)",
    },
    SectionDef {
        id: SB1_CONN_A,
        name: "sb1_conn_a",
        file_off: 0x00472B,
        len: 252,
        label: "z2dis:prg1:bank1_West_Hyrule__Room_Connectivity_Data ($871B)",
    },
    SectionDef {
        id: SB1_ENEMIES,
        name: "sb1_enemies",
        file_off: 0x0048B0,
        len: 1024,
        label: "z2dis:prg1:bank1_Enemy_Data__West_Hyrule_and_Death_Mountain \
                 ($88A0; copied to SRAM $7000-$73FF by \
                 LOOP_load_enemy_data_to_ram7000_7CFF)",
    },
    SectionDef {
        id: SB1_MAPPTR_B,
        name: "sb1_mapptr_b",
        file_off: 0x006010,
        len: 126,
        label: "z2dis:prg1:bank1_Area_Pointers_Death_Mountain ($A000, 63 maps)",
    },
    SectionDef {
        id: SB1_ENEMYPTR_B,
        name: "sb1_enemyptr_b",
        file_off: 0x00608E,
        len: 126,
        label: "z2dis:prg1:bank1_Enemy_Pointers_Death_Mountain ($A07E)",
    },
    SectionDef {
        id: SB1_CONN_B,
        name: "sb1_conn_b",
        file_off: 0x006208,
        len: 252,
        label: "z2dis:prg1:bank1_Room_Connectivity_Data ($A1F8)",
    },
    SectionDef {
        id: SB1_BEHIND,
        name: "sb1_behind",
        file_off: 0x004010,
        len: 14,
        label: "DW: behind-map pointers 1-7 ($8000; 7th word unused=0)",
    },
    SectionDef {
        id: PRG_BANK_1,
        name: "prg_bank_1",
        file_off: 0x004010,
        len: 16384,
        label: "PRG bank 1 image ($8000-$BFFF): sideview map bodies + code",
    },
    // ---- Sideview bank 2 (East Hyrule / Maze Island) ----
    SectionDef {
        id: SB2_MAPPTR_A,
        name: "sb2_mapptr_a",
        file_off: 0x008533,
        len: 126,
        label: "z2dis:prg2:bank2_Area_Pointers_East_Hyrule ($8523, 63 maps)",
    },
    SectionDef {
        id: SB2_ENEMYPTR_A,
        name: "sb2_enemyptr_a",
        file_off: 0x0085B1,
        len: 126,
        label: "z2dis:prg2:bank2_Enemy_Pointers__East_Hyrule ($85A1)",
    },
    SectionDef {
        id: SB2_CONN_A,
        name: "sb2_conn_a",
        file_off: 0x00872B,
        len: 252,
        label: "z2dis:prg2:bank2_Room_Connectivity_Data ($871B)",
    },
    SectionDef {
        id: SB2_ENEMIES,
        name: "sb2_enemies",
        file_off: 0x0088B0,
        len: 1024,
        label: "z2dis:prg2:bank2_Enemy_Data_East_Hyrule_and_Maze_Island \
                 ($88A0; copied to SRAM $7000-$73FF)",
    },
    SectionDef {
        id: SB2_MAPPTR_B,
        name: "sb2_mapptr_b",
        file_off: 0x00A010,
        len: 126,
        label: "z2dis:prg2:bank2_Area_Pointers_Maze_Island ($A000, 63 maps)",
    },
    SectionDef {
        id: SB2_ENEMYPTR_B,
        name: "sb2_enemyptr_b",
        file_off: 0x00A08E,
        len: 126,
        label: "z2dis:prg2:bank2_Enemy_Pointers_Maze_Island ($A07E)",
    },
    SectionDef {
        id: SB2_CONN_B,
        name: "sb2_conn_b",
        file_off: 0x00A208,
        len: 252,
        label: "z2dis:prg2:bank2_Maze_Island_Room_Connectivity_Data ($A1F8)",
    },
    SectionDef {
        id: SB2_BEHIND,
        name: "sb2_behind",
        file_off: 0x008010,
        len: 14,
        label: "DW: behind-map pointers 1-7 ($8000; 7th word unused=0)",
    },
    SectionDef {
        id: PRG_BANK_2,
        name: "prg_bank_2",
        file_off: 0x008010,
        len: 16384,
        label: "PRG bank 2 image ($8000-$BFFF): sideview map bodies + code",
    },
    // ---- Sideview bank 3 (towns; single set) + dialog ----
    SectionDef {
        id: SB3_MAPPTR_A,
        name: "sb3_mapptr_a",
        file_off: 0x00C533,
        len: 126,
        label: "z2dis:prg3:bank3_Area_Pointers__Towns ($8523, 63 maps)",
    },
    SectionDef {
        id: SB3_ENEMYPTR_A,
        name: "sb3_enemyptr_a",
        file_off: 0x00C5B1,
        len: 126,
        label: "z2dis:prg3:bank3_Enemy_Pointers__Towns ($85A1)",
    },
    SectionDef {
        id: SB3_CONN_A,
        name: "sb3_conn_a",
        file_off: 0x00C72B,
        len: 252,
        label: "z2dis:prg3:bank3_Room_Connectivity_Data_size_unknown ($871B; \
                 63x4 assumed like other banks)",
    },
    SectionDef {
        id: SB3_ENEMIES,
        name: "sb3_enemies",
        file_off: 0x00C8B0,
        len: 1024,
        label: "z2dis:prg3:bank3_Enemy_Data_Towns_ ($88A0; copied to SRAM $7000-$73FF)",
    },
    SectionDef {
        id: SB3_BGPTR,
        name: "sb3_bgptr",
        file_off: 0x00C010,
        len: 4,
        label: "z2dis:prg3:bank3_Pointer_table_for_Background_Areas_Data \
                 ($8000 slot holds bg pointers on bank 3)",
    },
    SectionDef {
        id: DLG_IDX2,
        name: "dlg_idx2",
        file_off: 0x00E2AC,
        len: 64,
        label: "z2dis:prg3:bank3_related_to_dialog_indexes2",
    },
    SectionDef {
        id: DLG_IDX3,
        name: "dlg_idx3",
        file_off: 0x00E2EC,
        len: 100,
        label: "z2dis:prg3:bank3_related_to_dialog_indexes3 (to indexes4)",
    },
    SectionDef {
        id: DLG_IDX4,
        name: "dlg_idx4",
        file_off: 0x00E350,
        len: 64,
        label: "z2dis:prg3:bank3_related_to_dialog_indexes4 (to text table)",
    },
    SectionDef {
        id: DIALOG,
        name: "dialog",
        file_off: 0x00E390,
        len: 3133,
        label: "DC TBL: dialog text ($E390-$EFCC); \
                 z2dis:prg3:bank3_Dialogs_Text_Table",
    },
    SectionDef {
        id: PRG_BANK_3,
        name: "prg_bank_3",
        file_off: 0x00C010,
        len: 16384,
        label: "PRG bank 3 image ($8000-$BFFF): town map bodies + text + code",
    },
    // ---- Sideview bank 4 (palaces 1,2,5 / 3,4,6) ----
    SectionDef {
        id: SB4_MAPPTR_A,
        name: "sb4_mapptr_a",
        file_off: 0x010533,
        len: 126,
        label: "z2dis:prg4:bank4_Area_Pointers_Palaces_Type_A ($8523, 63 maps)",
    },
    SectionDef {
        id: SB4_ENEMYPTR_A,
        name: "sb4_enemyptr_a",
        file_off: 0x0105B1,
        len: 126,
        label: "z2dis:prg4:bank4_Enemy_Pointers_Palaces_Type_A ($85A1)",
    },
    SectionDef {
        id: SB4_CONN_A,
        name: "sb4_conn_a",
        file_off: 0x01072B,
        len: 252,
        label: "z2dis:prg4:bank4_Room_Connectivity_Data_for_Palaces_Type_A_ ($871B)",
    },
    SectionDef {
        id: SB4_ENEMIES,
        name: "sb4_enemies",
        file_off: 0x0108B0,
        len: 1024,
        label: "z2dis:prg4:bank4_Enemy_Data_for_Palaces_Type_A_B \
                 ($88A0; copied to SRAM $7000-$73FF)",
    },
    SectionDef {
        id: SB4_MAPPTR_B,
        name: "sb4_mapptr_b",
        file_off: 0x012010,
        len: 126,
        label: "z2dis:prg4:bank4_Area_Pointers_Palaces_Type_B_ ($A000, 63 maps)",
    },
    SectionDef {
        id: SB4_ENEMYPTR_B,
        name: "sb4_enemyptr_b",
        file_off: 0x01208E,
        len: 126,
        label: "z2dis:prg4:bank4_Enemy_Pointers_Palaces_Type_B ($A07E)",
    },
    SectionDef {
        id: SB4_CONN_B,
        name: "sb4_conn_b",
        file_off: 0x012208,
        len: 252,
        label: "z2dis:prg4:bank4_Room_Connectivity_Data_for_Palaces_Type_B ($A1F8)",
    },
    SectionDef {
        id: SB4_BEHIND,
        name: "sb4_behind",
        file_off: 0x010010,
        len: 14,
        label: "DW: $8000 slot, palace key-area aliases ($861F/$A0FC/...); RAW",
    },
    SectionDef {
        id: PRG_BANK_4,
        name: "prg_bank_4",
        file_off: 0x010010,
        len: 16384,
        label: "PRG bank 4 image ($8000-$BFFF): palace map bodies + code",
    },
    // ---- Sideview bank 5 (Great Palace; single set) ----
    SectionDef {
        id: SB5_MAPPTR_A,
        name: "sb5_mapptr_a",
        file_off: 0x014533,
        len: 126,
        label: "z2dis:prg5:bank5_Area_Pointers_Great_Palace ($8523, 63 maps)",
    },
    SectionDef {
        id: SB5_ENEMYPTR_A,
        name: "sb5_enemyptr_a",
        file_off: 0x0145B1,
        len: 126,
        label: "z2dis:prg5:bank5_Enemy_Pointers_Great_Palace ($85A1)",
    },
    SectionDef {
        id: SB5_CONN_A,
        name: "sb5_conn_a",
        file_off: 0x01472B,
        len: 252,
        label: "z2dis:prg5:bank5_Room_Connectivity_Data ($871B)",
    },
    SectionDef {
        id: SB5_ENEMIES,
        name: "sb5_enemies",
        file_off: 0x0148B0,
        len: 1024,
        label: "z2dis:prg5:bank5_Enemy_Data_Great_Palace \
                 ($88A0; copied to SRAM $7000-$73FF)",
    },
    SectionDef {
        id: SB5_BEHIND,
        name: "sb5_behind",
        file_off: 0x014010,
        len: 14,
        label: "DW: $8000 slot, all zero on bank 5 (no behind maps); RAW",
    },
    SectionDef {
        id: PRG_BANK_5,
        name: "prg_bank_5",
        file_off: 0x014010,
        len: 16384,
        label: "PRG bank 5 image ($8000-$BFFF): Great Palace map bodies + code",
    },
    // ---- Bank-0 text / spell / experience tables ----
    SectionDef {
        id: GAMEOVER_TEXT,
        name: "gameover_text",
        file_off: 0x000010,
        len: 31,
        label: "z2dis:prg0:Tables_for_Game_Over_screen_text",
    },
    SectionDef {
        id: LEVELUP_TILES,
        name: "levelup_tiles",
        file_off: 0x001BBA,
        len: 112,
        label: "z2dis:prg0:LevelUp_Pane_tile_mappings (to SpellMenu pane)",
    },
    SectionDef {
        id: SPELLMENU_TEXT,
        name: "spellmenu_text",
        file_off: 0x001C2A,
        len: 310,
        label: "z2dis:prg0:SpellMenu_Pane__SpellText",
    },
    SectionDef {
        id: SPELL_COST,
        name: "spell_cost",
        file_off: 0x000D8B,
        len: 64,
        label: "z2dis:prg0:Table_for_Magic_Needed_for_Spells (8 spells x 8 levels)",
    },
    SectionDef {
        id: SPELL_FX,
        name: "spell_fx",
        file_off: 0x000DCB,
        len: 8,
        label: "z2dis:prg0:Table_for_Spell_effects",
    },
    SectionDef {
        id: SPELL_PTR,
        name: "spell_ptr",
        file_off: 0x000E58,
        len: 16,
        label: "DC: Magic Spell Effect Pointers ($E58-$E67)",
    },
    SectionDef {
        id: EXP_HI,
        name: "exp_hi",
        file_off: 0x001669,
        len: 24,
        label: "z2dis:prg0:Table_for_levelup_experience_high (Atk/Mag/Life)",
    },
    SectionDef {
        id: EXP_LO,
        name: "exp_lo",
        file_off: 0x001681,
        len: 24,
        label: "z2dis:prg0:Table_for_levelup_experience_low (Atk/Mag/Life)",
    },
    // ---- Bank-1 background level data (also inside prg_bank_1) ----
    SectionDef {
        id: LEVEL_BG_GRASS,
        name: "level_bg_grass",
        file_off: 0x004C4C,
        len: 28,
        label: "DC: Grass Area level data ($4C4C-$4C67); \
                 z2dis:prg1:bank1_Background_Areas_Data",
    },
    SectionDef {
        id: LEVEL_BG_FOREST,
        name: "level_bg_forest",
        file_off: 0x004C64,
        len: 21,
        label: "DC: Forest Area level data ($4C64-$4C78)",
    },
    SectionDef {
        id: LEVEL_BG_BAG,
        name: "level_bg_bag",
        file_off: 0x004C8C,
        len: 27,
        label: "DC: 50 point bag level data ($4C8C-$4CA6)",
    },
    // ---- Bank-5 init / menu data ----
    SectionDef {
        id: BEGIN_VALUES,
        name: "begin_values",
        file_off: 0x017AF7,
        len: 28,
        label: "DC: Beginning Values ($17AF7-$17B12: spells/items/techs)",
    },
    SectionDef {
        id: MAINMENU_CURSOR,
        name: "mainmenu_cursor",
        file_off: 0x017339,
        len: 93,
        label: "DC: Main Menu cursor positions ($17339-$17395)",
    },
    // ---- Bank-7 item / enemy-param / save tables ----
    SectionDef {
        id: LIVES,
        name: "lives",
        file_off: 0x01C369,
        len: 1,
        label: "DC: Number of lives to begin game ($1C369)",
    },
    SectionDef {
        id: ITEM_PRESENCE,
        name: "item_presence",
        file_off: 0x01C275,
        len: 32,
        label: "z2dis:prg7:bank7_Pointer_table_for_Item_Presence (16 words)",
    },
    SectionDef {
        id: DROP_PROB,
        name: "drop_prob",
        file_off: 0x01E880,
        len: 16,
        label: "z2dis:prg7:bank7_Table_for_Probability_for_Item_given_by_killed_enemy",
    },
    SectionDef {
        id: DEFEAT_COUNT,
        name: "defeat_count",
        file_off: 0x01E8B0,
        len: 1,
        label: "DC: Number of enemies to defeat for exp. bag or magic jar ($1E8B0)",
    },
    SectionDef {
        id: VULN_BITS,
        name: "vuln_bits",
        file_off: 0x01E910,
        len: 2,
        label: "DC: Type of vulnerability Bit (red) / Bot (blue) ($1E910-$1E911)",
    },
    SectionDef {
        id: ITEM_SPR_MAP,
        name: "item_spr_map",
        file_off: 0x01EE61,
        len: 46,
        label: "z2dis:prg7:bank7_table_item_sprites_map_to_graphic_image (to LEE7F)",
    },
    SectionDef {
        id: ITEM_TILE_MAP,
        name: "item_tile_map",
        file_off: 0x01EE90,
        len: 46,
        label: "z2dis:prg7: Tile Mappings for Items (2E bytes)",
    },
    SectionDef {
        id: SAVE_SIG,
        name: "save_sig",
        file_off: 0x01FFEF,
        len: 17,
        label: "SRAM init signature `LEGEND OF ZELDA2 (last 17 PRG bytes)",
    },
    // ---- Bank-6 music / SFX data ----
    SectionDef {
        id: MUSIC_NOTETABLES,
        name: "music_notetables",
        file_off: 0x018027,
        len: 248,
        label: "DC: Note Tables ($18027-$1811E); z2dis:prg6:Bank6__UNKNOWN_0",
    },
    SectionDef {
        id: MUSIC_SONG0,
        name: "music_song0",
        file_off: 0x0184EA,
        len: 690,
        label: "z2dis:prg6:Index_Table_0/Phrase_Order_Table_0/Song_Data_0_* \
                 (to Bank6__UNUSED_0)",
    },
    SectionDef {
        id: MUSIC_SONG1,
        name: "music_song1",
        file_off: 0x01A010,
        len: 970,
        label: "z2dis:prg6:Index_Table_1/Phrase_Order_Table_1/Song_Data_1_* \
                 (DC: Overworld $1A01A / Battleground $1A027 strings inside)",
    },
    SectionDef {
        id: MUSIC_SONG2,
        name: "music_song2",
        file_off: 0x01A3DA,
        len: 613,
        label: "z2dis:prg6:Index_Table_2/Phrase_Order_Table_2/Song_Data_2_*",
    },
    SectionDef {
        id: MUSIC_SONG3,
        name: "music_song3",
        file_off: 0x01A63F,
        len: 791,
        label: "z2dis:prg6:Index_Table_3/Phrase_Order_Table_3/Song_Data_3_*",
    },
    SectionDef {
        id: MUSIC_SONG4,
        name: "music_song4",
        file_off: 0x01A946,
        len: 2003,
        label: "z2dis:prg6:Index_Table_4/Phrase_Order_Table_4/Song_Data_4_* \
                 (to Bank6__UNUSED_2; incl. SFX phrases)",
    },
];

/// Verified quirks: `(section_id, word_index, value)` entries exempt from the
/// normal SRAM-range check on enemy-pointer tables.
///
/// Currently exactly one: bank 2 set A map #21's enemy pointer is the ROM
/// address `$8CF4` (verified against the reference ROM and the vendored
/// disassembly word at file `0x85DB`); every other enemy pointer ROM-wide
/// lands in SRAM `$7000-$73FF`.
pub const ENEMY_PTR_QUIRKS: &[(u16, usize, u16)] = &[(SB2_ENEMYPTR_A, 21, 0x8CF4)];

/// Data Crystal text-table alphabet for [`DIALOG`] validation.
///
/// Text bytes (`$DA-$F3` letters/digits, punctuation, `$F4` space) plus the
/// control codes `$FD` (next line), `$FE` (delay), `$FF` (end talk).
/// (Letter case, `l`/`m`/`x`/dagger variants per the TBL page.)
pub const DIALOG_ALPHABET: &[u8] = &[
    0x32, 0x34, 0x36, 0x9C, 0xCE, 0xCF, 0xD0, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9,
    0xDA, 0xDB, 0xDC, 0xDD, 0xDE, 0xDF, 0xE0, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9,
    0xEA, 0xEB, 0xEC, 0xED, 0xEE, 0xEF, 0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF7, 0xF8, 0xF9, 0xFC,
    0xFD, 0xFE, 0xFF,
];

/// Look up a section by machine name.
#[must_use]
pub fn find_by_name(name: &str) -> Option<&'static SectionDef> {
    SECTION_TABLE.iter().find(|d| d.name == name)
}

/// Look up a section by id.
#[must_use]
pub fn find_by_id(id: u16) -> Option<&'static SectionDef> {
    SECTION_TABLE.iter().find(|d| d.id == id)
}
