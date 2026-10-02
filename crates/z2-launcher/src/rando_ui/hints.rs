//! Hint options.

use egui::Ui;
use z2_rando::flags::HintFlags;

use super::{check, choice, grid, tri};

/// Widgets for [`HintFlags`].
pub fn ui(ui: &mut Ui, f: &mut HintFlags) {
    grid(ui, "rando-hints", |ui| {
        choice(
            ui,
            "Helpful hints",
            "Four townsfolk tell you where four important items are (Old Kasuto's \
             wall always has one). By continent: they name the region only. \
             Towns separate: they name the town or the palace. Other advice-givers \
             say they know nothing.",
            &mut f.helpful_hints,
        );
        tri(
            ui,
            "Spell item hints",
            "The people who ask for the trophy, medicine, child, water and mirror \
             say what their town's wizard gives; the two closed stab-teacher \
             doors say what the knight inside teaches.",
            &mut f.spell_item_hints,
        );
        tri(
            ui,
            "Town name hints",
            "Each wizard town's sign names what its wizard gives.",
            &mut f.town_name_hints,
        );
        check(
            ui,
            "Spoiler seed",
            "Marks the seed as a spoiler seed; this changes the generated game. \
             The spoiler log itself is chosen above.",
            &mut f.generate_spoiler,
        );
        check(
            ui,
            "Reveal walkthrough walls",
            "False walls and floors you can walk through are drawn as plain \
             background, so they show as gaps.",
            &mut f.reveal_walkthrough_walls,
        );
        check(
            ui,
            "Reveal hidden jars",
            "Spots you can strike for a hidden jar (or enemy) show an orange jar.",
            &mut f.reveal_hidden_jars,
        );
    });
}
