//! The "Enhancements (ZALiA-inspired)" launcher section.
//!
//! The widgets live in the shared `z2-enh-ui` crate, which the game's
//! in-game options overlay (`O` key) draws too, so both always show the same
//! options with the same tooltips.

use crate::settings::Settings;

/// Draw the section. `force_open` opens every group (tests use it so every
/// control renders).
pub fn enhancements_section(ui: &mut egui::Ui, s: &mut Settings, force_open: bool) {
    z2_enh_ui::enhancements_section(
        ui,
        &mut s.enhancements,
        &mut s.display_enh,
        z2_enh_ui::Opts {
            force_open,
            gameplay_editable: true,
            rom_randomized: s.rando.enabled,
        },
    );
}
