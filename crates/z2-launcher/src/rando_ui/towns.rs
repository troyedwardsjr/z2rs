//! Town options.

use egui::Ui;
use z2_rando::flags::TownFlags;

use super::{check, grid};

/// Widgets for [`TownFlags`].
pub fn ui(ui: &mut Ui, f: &mut TownFlags) {
    grid(ui, "rando-towns", |ui| {
        check(
            ui,
            "Random New Kasuto requirement",
            "The old woman's basement in New Kasuto opens after 5, 6 or 7 magic \
             containers (vanilla 7); her line states the number.",
            &mut f.randomize_new_kasuto_jar_requirements,
        );
        check(
            ui,
            "Shorter wizard visits",
            "Doors to the wizards, the downward-stab knight and the New Kasuto \
             basement skip the room in between, both ways. The townsfolk who \
             lead you there still have to open the door.",
            &mut f.shorten_wizards,
        );
    });
}
