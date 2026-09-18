//! Bank-7 trap registry.
//!
//! [`register_bank7_traps`] installs one trap per verified fixed-bank
//! routine. All targets live in the fixed bank (`$C000-$FFFF`), so the
//! 16-bit trap keys never alias other banks (see `traps.rs` M1 caveat).
//! Deliberately untrapped (documented in their modules):
//!
//! * `bank7_reset` (`$FF70`) — the RESET vector has no caller; `Cpu::reset`
//!   fires the trap *then* re-runs the ASM bytes (double execution). The
//!   prologue effects live in `bank7_reset::reset_prefix` as a library.
//! * `bank7_PowerON_code` (`$C000`) — entered by `JMP`; a `JMP` trap would
//!   pop a return address that was never pushed.
//! * `bank7_NMI_Entry_Point` (`$C07B`) — its bank-5/6/0 callees wait on
//!   vblank/sprite-0/sound state owned by the PPU and APU models; the terminating
//!   spans are ported in `bank7_nmi` as libraries instead.
//! * Inline NMI blocks (timers/RNG at `$C169-$C1B0`, input latch at
//!   `$C132-$C137`) — no `JSR`/`JMP` entry; covered by `bank7_timers` /
//!   `bank7_input` libraries and the mode dispatcher.

use crate::game::Game;

/// Register every verified bank-7 trap on `game` (idempotent: re-registering
/// an address replaces the entry with an identical one).
pub fn register_bank7_traps(game: &mut Game) {
    // MMC1 serial switchers (bank7_mmc1).
    game.trap_register(
        "ConfigureMMC1",
        Some(7),
        0xFF9D,
        crate::bank7_mmc1::configure_mmc1,
    );
    game.trap_register("SwapCHR", Some(7), 0xFFB1, crate::bank7_mmc1::swap_chr);
    game.trap_register(
        "SwapToPRG0",
        Some(7),
        0xFFC5,
        crate::bank7_mmc1::swap_to_prg0,
    );
    game.trap_register(
        "SwapToSavedPRG",
        Some(7),
        0xFFC9,
        crate::bank7_mmc1::swap_to_saved_prg,
    );
    game.trap_register("SwapPRG", Some(7), 0xFFCC, crate::bank7_mmc1::swap_prg);
    // Controller input (bank7_input).
    game.trap_register(
        "Controllers_Input",
        Some(7),
        0xD346,
        crate::bank7_input::read_controllers,
    );
    game.trap_register(
        "Controllers_Input_Capture",
        Some(7),
        0xD367,
        crate::bank7_input::capture_controller,
    );
    // Memory init + sprite visibility (bank7_mem).
    game.trap_register(
        "Reset_Memory_Ranges",
        Some(7),
        0xD281,
        crate::bank7_mem::reset_memory_ranges,
    );
    game.trap_register(
        "Set_Memory_300_4FF_and_00_DF_to_Zero",
        Some(7),
        0xD29C,
        crate::bank7_mem::clear_0300_04ff_and_zeropage,
    );
    game.trap_register(
        "Remove_All_Sprites",
        Some(7),
        0xD24C,
        crate::bank7_mem::remove_all_sprites,
    );
    game.trap_register(
        "Remove_All_Sprites_except_Sprite0",
        Some(7),
        0xD250,
        crate::bank7_mem::remove_sprites_keep_sprite0,
    );
    game.trap_register(
        "Erase_Name_Table_1",
        Some(7),
        0xD261,
        crate::bank7_mem::erase_name_table_1,
    );
    game.trap_register(
        "Erase_Name_Table_0",
        Some(7),
        0xD263,
        crate::bank7_mem::erase_name_table_0,
    );
    game.trap_register(
        "Erase_Name_Tables_0and1",
        Some(7),
        0xD266,
        crate::bank7_mem::erase_both_name_tables,
    );
    game.trap_register(
        "Fill_Screen_F4",
        Some(7),
        0xD2BE,
        crate::bank7_mem::fill_screen,
    );
    // PPU update queue (bank7_ppu_queue).
    game.trap_register(
        "LD2EC_PPU_Queue_Drain",
        Some(7),
        0xD2EC,
        crate::bank7_ppu_queue::drain_update_queue,
    );
    game.trap_register(
        "code52_Sideview_Palette",
        Some(7),
        0xFD82,
        crate::bank7_ppu_queue::load_sideview_palette,
    );
    // Trampolines + change detectors (bank7_dispatch).
    game.trap_register(
        "JmpToRoutine_073D",
        Some(7),
        0xD382,
        crate::bank7_dispatch::jump_routine_073d,
    );
    game.trap_register(
        "PullAddr_JMP",
        Some(7),
        0xD385,
        crate::bank7_dispatch::jump_indexed_table,
    );
    game.trap_register(
        "LD168_Mode_Change",
        Some(7),
        0xD168,
        crate::bank7_dispatch::detect_game_mode_change,
    );
    game.trap_register(
        "LD174_Stage_Change",
        Some(7),
        0xD174,
        crate::bank7_dispatch::detect_boot_stage_change,
    );
    game.trap_register(
        "LD158_Macro_Select",
        Some(7),
        0xD158,
        crate::bank7_dispatch::store_ppu_macro_selector,
    );
    game.trap_register(
        "LD15C_Dialog_Change",
        Some(7),
        0xD15C,
        crate::bank7_dispatch::detect_dialog_change,
    );
}

/// Number of traps [`register_bank7_traps`] installs.
pub const BANK7_TRAP_COUNT: usize = 23;
