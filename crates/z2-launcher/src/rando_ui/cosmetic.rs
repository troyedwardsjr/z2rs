//! Cosmetic options (never change the generated game, the seed or the hash
//! code). The character sprite is your own IPS file, picked at the top of
//! this section.

use egui::Ui;
use z2_rando::flags::CosmeticFlags;

use super::{check, choice, color, grid, heading};

/// The sprite-patch row: a path field and a Browse button. Returns `true`
/// when Browse was clicked (the caller opens the file dialog).
pub fn sprite_patch(ui: &mut Ui, path: &mut String) -> bool {
    let tip = "Your own player-sprite patch (.ips made for the original ROM). z2rs \
               ships no sprites. Only its graphics and Link's colours are used; \
               anything else in the patch is ignored and reported in the spoiler log.";
    let mut browse = false;
    egui::Grid::new("rando-sprite")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Sprite patch (IPS)").on_hover_text(tip);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(path)
                        .hint_text("none (your own .ips file)")
                        .desired_width(240.0),
                )
                .on_hover_text(tip);
                if ui.button("Browse…").clicked() {
                    browse = true;
                }
                if !path.is_empty() && ui.button("Clear").clicked() {
                    path.clear();
                }
            });
            ui.end_row();
        });
    browse
}

/// Widgets for [`CosmeticFlags`].
pub fn ui(ui: &mut Ui, f: &mut CosmeticFlags) {
    grid(ui, "rando-cosmetic", |ui| {
        heading(ui, "Colours");
        color(
            ui,
            "Tunic",
            "Link's tunic colour. Default keeps the ROM's (or your sprite patch's); \
             Random picks one from the seed, different from the other three colours.",
            &mut f.tunic,
        );
        color(
            ui,
            "Skin",
            "Link's skin colour (Default, Random from the seed, or an NES colour).",
            &mut f.skin_tone,
        );
        color(
            ui,
            "Outline",
            "Link's outline colour (Default, Random from the seed, or an NES colour).",
            &mut f.tunic_outline,
        );
        color(
            ui,
            "Shield tunic",
            "Tunic colour while the Shield spell is on (vanilla red, $16).",
            &mut f.shield_tunic,
        );
        choice(
            ui,
            "Beam sprite",
            "Graphic of the sword beam, taken from another sprite in your ROM. \
             Random picks one from the seed.",
            &mut f.beam_sprite,
        );
        check(
            ui,
            "Shuffle sprite palettes",
            "Give each area's enemy palettes a different hue (from the seed). \
             Brightness is kept, so nothing turns invisible; Link and items keep \
             their colours.",
            &mut f.shuffle_sprite_palettes,
        );
        check(
            ui,
            "Sprite patch changes items",
            "Let your sprite patch also redraw item graphics (key, containers, \
             P-bag, jar, the eight items). Off keeps the original items.",
            &mut f.change_item_sprites,
        );
        check(
            ui,
            "New townsfolk lines",
            "Replace stock townsfolk lines with z2rs's own text.",
            &mut f.community_text,
        );
        heading(ui, "Music");
        check(
            ui,
            "Disable music",
            "Background music plays silently (same timing); sound effects stay.",
            &mut f.disable_music,
        );
        check(
            ui,
            "Custom music",
            "Use your own music library. Not available yet: the original music \
             plays.",
            &mut f.randomize_music,
        );
        check(
            ui,
            "Mix custom and original",
            "With custom music: mix your tracks with the original ones (not \
             available yet).",
            &mut f.mix_custom_and_original_music,
        );
        check(
            ui,
            "Diverse music",
            "With custom music: include tracks in more varied styles (not \
             available yet).",
            &mut f.include_diverse_music,
        );
        check(
            ui,
            "Skip stream-unsafe music",
            "With custom music: leave out tracks marked unsafe for streaming (not \
             available yet).",
            &mut f.disable_unsafe_music,
        );
    });
}
