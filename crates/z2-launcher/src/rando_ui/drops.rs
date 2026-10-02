//! Enemy drop options.

use egui::Ui;
use z2_rando::flags::{DropFlags, DropPool};

use super::{check, grid, heading};

fn pool(ui: &mut Ui, who: &str, p: &mut DropPool) {
    for (label, v) in p.entries_mut() {
        let tip = format!("{who} enemies can drop: {label}.");
        check(ui, label, &tip, v);
    }
}

/// Widgets for [`DropFlags`].
pub fn ui(ui: &mut Ui, f: &mut DropFlags) {
    grid(ui, "rando-drops", |ui| {
        check(
            ui,
            "Shuffle drop frequency",
            "An item drops every 4 to 8 kills instead of every 6.",
            &mut f.shuffle_drop_frequency,
        );
        check(
            ui,
            "Randomize drops",
            "Each unticked item below has a 50% chance to join its pool.",
            &mut f.randomize_drops,
        );
        check(
            ui,
            "Standardize drops",
            "Drops follow a fixed sequence for the seed.",
            &mut f.standardize_drops,
        );
        heading(ui, "Small enemy pool");
        pool(ui, "Small", &mut f.small_pool);
        heading(ui, "Large enemy pool");
        pool(ui, "Large", &mut f.large_pool);
    });
}
