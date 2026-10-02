//! Levels and stats options.

use egui::Ui;
use z2_rando::flags::StatsFlags;

use super::{check, choice, grid, heading, range};

/// Widgets for [`StatsFlags`].
pub fn ui(ui: &mut Ui, f: &mut StatsFlags) {
    grid(ui, "rando-stats", |ui| {
        heading(ui, "Experience");
        check(
            ui,
            "Shuffle attack experience",
            "Attack level-up costs vary by up to 25%.",
            &mut f.shuffle_attack_exp,
        );
        check(
            ui,
            "Shuffle magic experience",
            "Magic level-up costs vary by up to 25%.",
            &mut f.shuffle_magic_exp,
        );
        check(
            ui,
            "Shuffle life experience",
            "Life level-up costs vary by up to 25%.",
            &mut f.shuffle_life_exp,
        );
        heading(ui, "Level caps");
        range(
            ui,
            "Attack cap",
            "Highest attack level; later level-ups give a life instead.",
            &mut f.attack_level_cap,
            1,
            8,
        );
        range(
            ui,
            "Magic cap",
            "Highest magic level.",
            &mut f.magic_level_cap,
            1,
            8,
        );
        range(
            ui,
            "Life cap",
            "Highest life level.",
            &mut f.life_level_cap,
            1,
            8,
        );
        check(
            ui,
            "Scale costs to caps",
            "With a cap below 8, the remaining levels are spread over the whole original cost curve (the last level costs what level 8 did).",
            &mut f.scale_level_requirements_to_cap,
        );
        heading(ui, "Effectiveness");
        choice(
            ui,
            "Attack effectiveness",
            "Sword damage per attack level.",
            &mut f.attack_effectiveness,
        );
        choice(
            ui,
            "Magic effectiveness",
            "Spell costs per magic level. The logic accounts for the new costs when it decides which spells you can cast.",
            &mut f.magic_effectiveness,
        );
        choice(
            ui,
            "Life effectiveness",
            "Damage Link takes per life level.",
            &mut f.life_effectiveness,
        );
    });
}
