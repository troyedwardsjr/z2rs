//! Enemy options: shuffles, hit points, experience and immunities.

use egui::Ui;
use z2_rando::flags::{EnemyFlags, EnemyLife};

use super::{check, choice, choice_from, grid, tri};

const BOSS_HP: &[EnemyLife] = &[
    EnemyLife::Vanilla,
    EnemyLife::Narrow,
    EnemyLife::Medium,
    EnemyLife::MediumHigh,
    EnemyLife::High,
];

/// Widgets for [`EnemyFlags`].
pub fn ui(ui: &mut Ui, f: &mut EnemyFlags) {
    grid(ui, "rando-enemies", |ui| {
        tri(
            ui,
            "Shuffle overworld enemies",
            "Encounter enemies are shuffled within their continent.",
            &mut f.shuffle_overworld_enemies,
        );
        tri(
            ui,
            "Shuffle palace enemies",
            "Room enemies are shuffled within their palace group.",
            &mut f.shuffle_palace_enemies,
        );
        tri(
            ui,
            "Mix large and small enemies",
            "Small and large ground enemies may replace each other (flyers stay flyers, generators stay generators).",
            &mut f.mix_large_and_small,
        );
        check(
            ui,
            "Generators always match",
            "Left and right spawners in a room spawn the same enemy.",
            &mut f.generators_always_match,
        );
        choice(
            ui,
            "Dripper enemy",
            "What the palace drippers spawn.",
            &mut f.dripper_enemy,
        );
        choice(
            ui,
            "Enemy HP",
            "Range regular enemy hit points are scaled by (each enemy type gets its own roll, capped at 255).",
            &mut f.enemy_hp,
        );
        choice_from(
            ui,
            "Boss HP",
            "Range boss hit points are scaled by (Great Palace bosses excluded).",
            &mut f.boss_hp,
            BOSS_HP,
        );
        check(
            ui,
            "Shuffle experience stealers",
            "Which enemies steal experience when they hit you (the same number per area as in the original).",
            &mut f.shuffle_xp_stealers,
        );
        check(
            ui,
            "Shuffle stolen amount",
            "Stolen experience is 50-150% of the original.",
            &mut f.shuffle_xp_stolen_amount,
        );
        choice(
            ui,
            "Sword immunity",
            "Which enemies can only be hurt with Fire. Shuffle keeps how many there are; the conditional choice removes them all when Fire is replaced by Dash or tied to Fairy.",
            &mut f.sword_immunity,
        );
        choice(
            ui,
            "Experience drops",
            "How much experience enemies give.",
            &mut f.xp_drops,
        );
        check(
            ui,
            "Randomize knockback",
            "Chaotic knockback when Link is hit.",
            &mut f.randomize_knockback,
        );
    });
}
