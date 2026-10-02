//! Palace options: styles, lengths, rooms, bosses and palettes.

use egui::Ui;
use z2_rando::flags::{PalaceFlags, PalaceStyle};

use super::{check, choice, choice_from, grid, heading, min_max, tri};

const NORMAL_STYLES: &[PalaceStyle] = &[
    PalaceStyle::Vanilla,
    PalaceStyle::Shuffled,
    PalaceStyle::Sequential,
    PalaceStyle::RandomWalk,
    PalaceStyle::VanillaWeighted,
    PalaceStyle::Tower,
    PalaceStyle::Mirror,
    PalaceStyle::Reconstructed,
    PalaceStyle::ReconstructedLoopy,
    PalaceStyle::Chaos,
    PalaceStyle::RandomAll,
    PalaceStyle::RandomPerPalace,
];
const GP_STYLES: &[PalaceStyle] = &[
    PalaceStyle::Vanilla,
    PalaceStyle::Shuffled,
    PalaceStyle::Sequential,
    PalaceStyle::RandomWalk,
    PalaceStyle::Tower,
    PalaceStyle::Reconstructed,
    PalaceStyle::ReconstructedLoopy,
    PalaceStyle::Chaos,
    PalaceStyle::Random,
];

/// Widgets for [`PalaceFlags`].
pub fn ui(ui: &mut Ui, f: &mut PalaceFlags) {
    grid(ui, "rando-palaces", |ui| {
        heading(ui, "Layout");
        choice_from(
            ui,
            "Palace style",
            "How palaces 1-6 are built from the rooms in your ROM. Vanilla keeps them; Vanilla shuffle swaps rooms that connect the same way; the others build new palaces room by room: Reconstructed (free graph), Loopy (no dead ends, loops), Chaos (some one-way passages), Random walk, Vanilla-weighted, Sequential, Tower (mostly elevators) and Mirror (symmetric map). Random picks never choose Loopy or Chaos.",
            &mut f.normal_style,
            NORMAL_STYLES,
        );
        choice_from(
            ui,
            "Great Palace style",
            "How the Great Palace is built from its own rooms (same styles; no Vanilla-weighted or Mirror).",
            &mut f.gp_style,
            GP_STYLES,
        );
        choice(
            ui,
            "Palace length",
            "Room count of palaces 1-6: Short 50-65%, Medium 60-80%, Full 85-115% (vanilla length for Vanilla and Vanilla shuffle), Random 50-115% of the vanilla count. Vanilla-based styles are shortened by splicing out straight rooms.",
            &mut f.normal_length,
        );
        choice(
            ui,
            "Great Palace length",
            "Room count of the Great Palace (same choices, at most 61 rooms).",
            &mut f.gp_length,
        );
        choice(
            ui,
            "Boss rooms exit to",
            "Whether walking right out of a boss room leaves the palace or leads to more palace (the statue in front of the exit is removed). Not for Vanilla, Vanilla shuffle or Tower palaces.",
            &mut f.boss_rooms_exit,
        );
        choice(
            ui,
            "Minimum path to Dark Link",
            "Fewest rooms between the Great Palace entrance and Dark Link (8, 12, or 20; 16 for Reconstructed). Generated Great Palaces only.",
            &mut f.dark_link_min_distance,
        );
        choice(
            ui,
            "Item rooms per palace",
            "How many item rooms each palace has. Extra item rooms are copies of other palaces' item rooms; Random shows 1 to 3 depending on length (Vanilla style keeps at most 1, Vanilla shuffle 2). Palaces keep at least one when palace items are not shuffled.",
            &mut f.item_rooms_per_palace,
        );
        choice(
            ui,
            "Drops connect to",
            "Where the rooms below a hole must lead back to: the entrance, the entrance or a boss room, either (Balanced picks per palace), or anywhere.",
            &mut f.drop_style,
        );
        min_max(
            ui,
            "Palaces to complete",
            "Crystals to place before the Great Palace opens (vanilla 6); one number is rolled between the two.",
            &mut f.palaces_to_complete_min,
            &mut f.palaces_to_complete_max,
            0,
            6,
        );
        heading(ui, "Rooms");
        check(
            ui,
            "Random styles allow vanilla",
            "Random styles may pick Vanilla or Vanilla shuffle.",
            &mut f.random_styles_allow_vanilla,
        );
        check(
            ui,
            "No duplicate rooms (layout)",
            "In generated palaces, no two rooms of a palace share a layout.",
            &mut f.no_duplicate_rooms_by_layout,
        );
        check(
            ui,
            "No duplicate rooms (layout and enemies)",
            "In generated palaces, no two rooms of a palace share a layout and enemy set.",
            &mut f.no_duplicate_rooms_by_enemies,
        );
        tri(
            ui,
            "Include original rooms",
            "Use the rooms from your ROM in generated palaces. Without a room pack this stays on.",
            &mut f.include_vanilla_rooms,
        );
        tri(
            ui,
            "Include extra rooms (set A)",
            "Rooms from a room pack you supply. z2rs ships no room packs, and loading packs is not implemented yet.",
            &mut f.include_extra_rooms_a,
        );
        tri(
            ui,
            "Include extra rooms (set B)",
            "Rooms from a second room pack you supply (not implemented yet).",
            &mut f.include_extra_rooms_b,
        );
        check(
            ui,
            "Blocking rooms anywhere",
            "Rooms built around an ability (jump, fairy, glove, stabs) may appear in any palace instead of only where that ability is expected.",
            &mut f.blocking_rooms_in_any_palace,
        );
        check(
            ui,
            "Remove long dead ends",
            "Leave out long dead-end rooms (needs room-pack tags; no effect on the original rooms).",
            &mut f.remove_long_dead_ends,
        );
        check(
            ui,
            "Include expert rooms",
            "Harder rooms join the pool (needs room-pack tags; no effect on the original rooms).",
            &mut f.include_expert_rooms,
        );
        heading(ui, "Gameplay");
        check(
            ui,
            "Restart at palaces",
            "A game over (or Up+A) restarts at the palace entrance. Not implemented yet.",
            &mut f.restart_at_palaces_on_game_over,
        );
        tri(
            ui,
            "50/50 statues",
            "Statues alternate between a red jar and a red Iron Knuckle. Not implemented yet.",
            &mut f.global_5050_jar_drop,
        );
        check(
            ui,
            "Reduce dripper variance",
            "A blue drip is guaranteed after a run of red ones. Not implemented yet.",
            &mut f.reduce_dripper_variance,
        );
        check(
            ui,
            "Random boss item drop",
            "Bosses drop a random small item instead of a key. Not implemented yet.",
            &mut f.randomize_boss_item_drop,
        );
        check(
            ui,
            "Harder Carock",
            "Carock fires varied beams. Not implemented yet.",
            &mut f.hard_bosses,
        );
        tri(
            ui,
            "Thunderbird required",
            "Dark Link can only be reached past Thunderbird (generated Great Palaces are built that way).",
            &mut f.thunderbird_required,
        );
        check(
            ui,
            "Remove Thunderbird",
            "No Thunderbird in the Great Palace (ignored when Thunderbird is required or the Great Palace is Vanilla).",
            &mut f.remove_thunderbird,
        );
        check(
            ui,
            "Aggressive Thunderbird",
            "Thunderbird starts weaker and in its aggressive phase (applied by the enemies module).",
            &mut f.aggressive_thunderbird,
        );
        check(
            ui,
            "Change palace palettes",
            "New brick, window and curtain colours for every palace.",
            &mut f.change_palace_palettes,
        );
    });
}
