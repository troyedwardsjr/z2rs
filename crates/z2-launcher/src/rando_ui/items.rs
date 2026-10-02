//! Item placement options.

use egui::Ui;
use z2_rando::flags::ItemFlags;

use super::{check, grid, tri};

/// Widgets for [`ItemFlags`].
pub fn ui(ui: &mut Ui, f: &mut ItemFlags) {
    grid(ui, "rando-items", |ui| {
        tri(
            ui,
            "Shuffle palace items",
            "The six palace items (and any extra item-room contents) trade places between palaces. Every seed is checked to be beatable.",
            &mut f.shuffle_palace_items,
        );
        tri(
            ui,
            "Shuffle overworld items",
            "Items in caves, on map tiles, the Maze Island drops and New Kasuto's tower and basement trade places. Every seed is checked to be beatable.",
            &mut f.shuffle_overworld_items,
        );
        tri(
            ui,
            "Mix overworld and palace items",
            "Palace and overworld items share one pool, so any of them can be anywhere. Needs both shuffles on.",
            &mut f.mix_overworld_and_palace_items,
        );
        tri(
            ui,
            "Include P-bag caves",
            "The three P-bag caves join the overworld pool (needs the overworld shuffle).",
            &mut f.include_pbag_caves,
        );
        tri(
            ui,
            "Include spells",
            "Wizards give items and spells join the pool. Not available yet: needs a town NPC patch that z2rs does not have, so this is ignored.",
            &mut f.include_spells,
        );
        tri(
            ui,
            "Include sword techniques",
            "The stab teachers give items and the stabs join the pool. Not available yet (needs the town NPC patch), so this is ignored.",
            &mut f.include_sword_techs,
        );
        tri(
            ui,
            "Include town quest items",
            "Bagu's note, the mirror and the water join the pool. Not available yet (needs the town NPC patch), so this is ignored.",
            &mut f.include_quest_items,
        );
        check(
            ui,
            "Prevent spell item chains",
            "A wizard never asks for a spell item to hand out another spell item. Only matters once town rewards can be shuffled.",
            &mut f.prevent_spell_item_chains,
        );
        check(
            ui,
            "Shuffle small items",
            "Bags, jars and 1-ups in caves and towns trade places, and every palace small item (keys included) is re-rolled.",
            &mut f.shuffle_small_items,
        );
        tri(
            ui,
            "Start with spell items",
            "Start with the trophy, medicine, child, water and mirror; their usual spots get small items instead.",
            &mut f.start_with_spell_items,
        );
        tri(
            ui,
            "Shuffle P-bag amounts",
            "Each P-bag size moves up to two steps on the experience ladder (roughly 20-150, 50-300, 100-500 and 200-1000).",
            &mut f.shuffle_pbag_amounts,
        );
        tri(
            ui,
            "Palaces contain extra keys",
            "Every small item in a palace becomes a key.",
            &mut f.palaces_contain_extra_keys,
        );
        check(
            ui,
            "Allow duplicate key items",
            "Spare copies of key items (glove, magic key, raft, boots, hammer, flute) may replace small-item spots in the shuffled pool.",
            &mut f.allow_important_item_duplicates,
        );
    });
}
