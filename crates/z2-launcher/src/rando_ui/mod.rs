//! Randomizer option widgets, one file per randomizer module.
//!
//! Each `<module>.rs` has `pub fn ui(ui: &mut egui::Ui, f: &mut <Module>Flags)`
//! and owns every widget for that module's options. The helpers below keep
//! the files short and consistent: every option is one grid row with a label
//! and a control, and the tooltip explains it on both.
//!
//! When you add a field to a `z2_rando::flags` struct, add
//! its row in the matching file here.

use egui::Ui;
use z2_rando::flags::{FlagEnum, NesColor, Tri};

pub mod cosmetic;
pub mod drops;
pub mod enemies;
pub mod hints;
pub mod items;
pub mod overworld;
pub mod palaces;
pub mod qol;
pub mod spells;
pub mod start;
pub mod stats;
pub mod towns;

/// Section titles, one per randomizer module, in the order the tab shows them.
pub const MODULE_TITLES: [&str; 12] = [
    "Start",
    "Overworld",
    "Palaces",
    "Items",
    "Enemies",
    "Levels and stats",
    "Spells",
    "Drops",
    "Hints",
    "Towns",
    "Quality of life",
    "Cosmetics",
];

/// A two-column grid for one module's rows.
pub fn grid(ui: &mut Ui, id: &str, body: impl FnOnce(&mut Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([12.0, 6.0])
        .striped(true)
        .show(ui, body);
}

/// A sub-heading spanning the grid.
pub fn heading(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).strong());
    ui.end_row();
}

/// Checkbox row.
pub fn check(ui: &mut Ui, label: &str, tip: &str, v: &mut bool) {
    ui.label(label).on_hover_text(tip);
    ui.checkbox(v, "").on_hover_text(tip);
    ui.end_row();
}

/// Off / On / Random row.
pub fn tri(ui: &mut Ui, label: &str, tip: &str, v: &mut Tri) {
    ui.label(label).on_hover_text(tip);
    ui.horizontal(|ui| {
        for t in Tri::ALL {
            ui.selectable_value(v, *t, t.label()).on_hover_text(tip);
        }
    });
    ui.end_row();
}

/// Drop-down row over every variant of an option enum.
pub fn choice<T: FlagEnum>(ui: &mut Ui, label: &str, tip: &str, v: &mut T) {
    choice_from(ui, label, tip, v, T::all());
}

/// Drop-down row over `options` only (for options that offer a subset).
pub fn choice_from<T: FlagEnum>(ui: &mut Ui, label: &str, tip: &str, v: &mut T, options: &[T]) {
    ui.label(label).on_hover_text(tip);
    egui::ComboBox::from_id_salt(label)
        .selected_text(v.label_of())
        .width(220.0)
        .show_ui(ui, |ui| {
            for o in options {
                ui.selectable_value(v, *o, o.label_of());
            }
        })
        .response
        .on_hover_text(tip);
    ui.end_row();
}

/// Slider row for a count limited to `lo..=hi`.
pub fn range(ui: &mut Ui, label: &str, tip: &str, v: &mut u8, lo: u8, hi: u8) {
    ui.label(label).on_hover_text(tip);
    ui.add(egui::Slider::new(v, lo..=hi)).on_hover_text(tip);
    ui.end_row();
}

/// Paired min/max sliders that keep `min <= max`.
pub fn min_max(ui: &mut Ui, label: &str, tip: &str, min: &mut u8, max: &mut u8, lo: u8, hi: u8) {
    ui.label(label).on_hover_text(tip);
    ui.horizontal(|ui| {
        ui.label("min");
        ui.add(egui::Slider::new(min, lo..=hi)).on_hover_text(tip);
        ui.label("max");
        ui.add(egui::Slider::new(max, lo..=hi)).on_hover_text(tip);
    });
    if *min > *max {
        *max = *min;
    }
    ui.end_row();
}

/// NES palette colour picker row (Default, Random or `$00-$3F`).
pub fn color(ui: &mut Ui, label: &str, tip: &str, v: &mut NesColor) {
    let text = |c: NesColor| match c {
        NesColor::Default => "Default".to_string(),
        NesColor::Random => "Random".to_string(),
        NesColor::Color(i) => format!("${i:02X}"),
    };
    ui.label(label).on_hover_text(tip);
    egui::ComboBox::from_id_salt(label)
        .selected_text(text(*v))
        .width(120.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(v, NesColor::Default, "Default");
            ui.selectable_value(v, NesColor::Random, "Random");
            for i in 0..0x40u8 {
                ui.selectable_value(v, NesColor::Color(i), text(NesColor::Color(i)));
            }
        })
        .response
        .on_hover_text(tip);
    ui.end_row();
}
