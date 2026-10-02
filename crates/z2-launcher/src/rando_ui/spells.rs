//! Spell options.

use egui::Ui;
use z2_rando::flags::SpellFlags;

use super::{check, choice, grid, tri};

/// Widgets for [`SpellFlags`].
pub fn ui(ui: &mut Ui, f: &mut SpellFlags) {
    grid(ui, "rando-spells", |ui| {
        check(
            ui,
            "Shuffle Life refill",
            "The Life spell heals 1 to 5 bars instead of 3 (one amount per seed).",
            &mut f.shuffle_life_refill,
        );
        tri(
            ui,
            "Shuffle spell locations",
            "Wizards teach different spells. The spell menu follows the towns (Rauru's spell is first), and the costs move with the spells. Off when spells are in the item pool.",
            &mut f.shuffle_spell_locations,
        );
        tri(
            ui,
            "No magic container requirements",
            "Wizards teach without checking magic containers (vanilla: the town number, 1 for Rauru up to 8 for Old Kasuto).",
            &mut f.disable_magic_container_requirements,
        );
        tri(
            ui,
            "Randomize Spell spell enemy",
            "The Spell spell turns enemies into another small enemy instead of a Bot.",
            &mut f.randomize_spell_spell_enemy,
        );
        tri(
            ui,
            "Swap upstab and downstab",
            "Mido teaches upstab and Darunia downstab. Off when sword techniques are in the item pool.",
            &mut f.swap_up_and_down_stab,
        );
        choice(
            ui,
            "Fire spell",
            "Normal; linked (casting Fire also casts a random other spell, and the other way round); or replaced by Dash (double running speed while active, no fireballs).",
            &mut f.fire_option,
        );
        check(
            ui,
            "Jump always on",
            "Link always jumps as high as with the Jump spell.",
            &mut f.jump_always_on,
        );
        check(
            ui,
            "Dash always on",
            "Link always runs at Dash speed (twice the normal speed).",
            &mut f.dash_always_on,
        );
        check(
            ui,
            "Permanent sword beam",
            "The sword beam fires at any health.",
            &mut f.permanent_beam_sword,
        );
        choice(
            ui,
            "Flute warp",
            "Not implemented yet: the option is kept in flag strings but has no effect.",
            &mut f.flute_warp,
        );
    });
}
