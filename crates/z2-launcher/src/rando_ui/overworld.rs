//! Overworld options: continent sizes, shapes, terrain, locations,
//! connections and encounters.

use egui::Ui;
use z2_rando::flags::{Biome, Climate, OverworldFlags};

use super::{check, choice, choice_from, grid, heading, tri};

const WEST_BIOMES: &[Biome] = &[
    Biome::Vanilla,
    Biome::VanillaShuffle,
    Biome::Vanillalike,
    Biome::Islands,
    Biome::Canyon,
    Biome::Caldera,
    Biome::Mountainous,
    Biome::RandomNoVanillaOrShuffle,
    Biome::RandomNoVanilla,
    Biome::Random,
];
const EAST_BIOMES: &[Biome] = &[
    Biome::Vanilla,
    Biome::VanillaShuffle,
    Biome::Vanillalike,
    Biome::Islands,
    Biome::Canyon,
    Biome::Volcano,
    Biome::Mountainous,
    Biome::RandomNoVanillaOrShuffle,
    Biome::RandomNoVanilla,
    Biome::Random,
];
const MAZE_BIOMES: &[Biome] = &[
    Biome::Vanilla,
    Biome::VanillaShuffle,
    Biome::Vanillalike,
    Biome::Random,
];
const DM_CLIMATES: &[Climate] = &[
    Climate::Classic,
    Climate::Chaos,
    Climate::Wetlands,
    Climate::GreatLakes,
    Climate::Scrubland,
    Climate::Random,
];

/// Widgets for [`OverworldFlags`].
pub fn ui(ui: &mut Ui, f: &mut OverworldFlags) {
    grid(ui, "rando-overworld", |ui| {
        heading(ui, "Shapes");
        choice(
            ui,
            "West size",
            "Size of a generated West Hyrule: Large 64x75, Medium 52x52, Small 44x44. Vanilla maps keep their size.",
            &mut f.west_size,
        );
        choice(
            ui,
            "East size",
            "Size of a generated East Hyrule: Large 64x75, Medium 52x52, Small 44x44. Vanilla maps keep their size.",
            &mut f.east_size,
        );
        choice(
            ui,
            "Death Mountain size",
            "Size of a generated Death Mountain. Smaller sizes drop cave pairs: Large 37 caves, Medium 27, Small 17, Tiny 9.",
            &mut f.dm_size,
        );
        choice(
            ui,
            "Maze Island size",
            "Size of a generated Maze Island (23, 19 or 15 tiles square); each step down drops one trap tile.",
            &mut f.maze_size,
        );
        choice_from(
            ui,
            "West biome",
            "Vanilla keeps the map. Vanilla shuffle moves the locations around the vanilla map (linked caves move as pairs). The others generate new terrain: Vanilla-like (a mountain ridge and a river), Islands (water lines split the land), Canyon (a river valley between mountains), Caldera (a crater in a central mountain, reached by a cave, with a palace inside), Mountainous (mountain lines). Random picks one (optionally never vanilla).",
            &mut f.west_biome,
            WEST_BIOMES,
        );
        choice_from(
            ui,
            "East biome",
            "As for West Hyrule, with Volcano instead of Caldera: the Great Palace sits inside a central mountain at the end of a lava path. On generated maps the Great Palace is always in a mountain-walled lava basin.",
            &mut f.east_biome,
            EAST_BIOMES,
        );
        choice_from(
            ui,
            "Death Mountain biome",
            "Vanilla, Vanilla shuffle (the caves trade places in pairs), or generated: pockets of open ground inside solid mountain, joined only by caves.",
            &mut f.dm_biome,
            WEST_BIOMES,
        );
        choice_from(
            ui,
            "Maze Island biome",
            "Vanilla, Vanilla shuffle, or generated: a new maze of corridors with the palace on a small plaza.",
            &mut f.maze_biome,
            MAZE_BIOMES,
        );
        choice(
            ui,
            "West climate",
            "Terrain mix of a generated West Hyrule: Classic (even mix), Vanilla-weighted (like the original map), Chaos (many small patches), Wetlands (swamp and water), Great lakes (big lakes, grass and forest), Scrubland (open desert and grass).",
            &mut f.west_climate,
        );
        choice(
            ui,
            "East climate",
            "Terrain mix of a generated East Hyrule (see West climate).",
            &mut f.east_climate,
        );
        choice_from(
            ui,
            "Death Mountain climate",
            "Terrain mix of the open pockets of a generated Death Mountain.",
            &mut f.dm_climate,
            DM_CLIMATES,
        );
        tri(
            ui,
            "Boots walk on all water",
            "Generated water is shallow: with the boots Link can walk on any of it, not only the marked paths.",
            &mut f.good_boots,
        );
        check(
            ui,
            "Legacy shuffled locations",
            "On vanilla-shuffled maps, tiles keep their original look, so a town may hide behind a cave icon.",
            &mut f.legacy_vanilla_shuffled_locations,
        );
        heading(ui, "Locations and connections");
        choice(
            ui,
            "Continent connectors",
            "Normal: the vanilla links. Transportation shuffle: the same continents are linked, but which link is the raft, the bridge or a cave is random. Anything goes: each of the four connectors links a random pair (West-Death Mountain, West-East or East-Maze Island; all three appear). Continents with vanilla maps keep their connectors.",
            &mut f.continent_connections,
        );
        choice(
            ui,
            "Less important locations",
            "On non-vanilla maps: Blend in hides minor tiles (jars, fairies, small bags) in terrain of their own kind, Isolate gives each its own clearing, Remove takes them off the map.",
            &mut f.less_important_locations,
        );
        tri(
            ui,
            "Generate Bagu's woods",
            "Bagu's house and the lost-woods tiles in a forest of their own on a generated West map.",
            &mut f.generate_bagu_woods,
        );
        tri(
            ui,
            "Restrict connection caves",
            "On generated maps, the two ends of a passthrough cave face away from each other with a mountain between them; off, each end goes anywhere.",
            &mut f.restrict_connection_cave_shuffle,
        );
        check(
            ui,
            "Connection caves can be blocked",
            "Rocks (hammer) may also block passthrough and continent caves, not only other caves.",
            &mut f.allow_connection_caves_blocked,
        );
        choice(
            ui,
            "River devil blocks",
            "On a generated East Hyrule the river devil (flute) blocks a bridge or road, a cave mouth, or surrounds a town.",
            &mut f.river_devil_blocker,
        );
        tri(
            ui,
            "East rock blocks",
            "On a generated East Hyrule a rock (hammer) blocks a path or a cave.",
            &mut f.east_rocks,
        );
        tri(
            ui,
            "Three-eyed rock location",
            "A location stays hidden until the flute is played two tiles above it, between three boulders (vanilla: palace 6). Off shows it on the map.",
            &mut f.hide_palace,
        );
        tri(
            ui,
            "Hidden town tile",
            "A location stays hidden under a forest tile until the hammer is used on it (vanilla: New Kasuto). Off shows it on the map.",
            &mut f.hide_kasuto,
        );
        tri(
            ui,
            "Shuffle hidden locations",
            "On a non-vanilla East Hyrule, any East town, palace, item or minor tile may be the hidden one.",
            &mut f.shuffle_hidden_locations,
        );
        tri(
            ui,
            "Palaces swap continents",
            "Palaces 1-6 trade places between the palace spots of all continents.",
            &mut f.palaces_swap_continents,
        );
        tri(
            ui,
            "Include Great Palace",
            "The Great Palace joins that trade (needs palaces swapping continents).",
            &mut f.shuffle_great_palace,
        );
        heading(ui, "Encounters");
        choice(
            ui,
            "Encounter rate",
            "How often overworld enemies appear: None (trap tiles still fight), Half, Normal, or Random (rolled separately for West Hyrule, Death Mountain / Maze Island and East Hyrule).",
            &mut f.encounter_rate,
        );
        tri(
            ui,
            "Shuffle encounter terrains",
            "Which battle scene each terrain leads to (desert, grass, forest, swamp, graveyard; north and south).",
            &mut f.shuffle_encounters,
        );
        check(
            ui,
            "Unsafe path encounters",
            "Road scenes join the shuffle.",
            &mut f.allow_unsafe_path_encounters,
        );
        check(
            ui,
            "Include lava",
            "Lava scenes (East Hyrule) join the shuffle.",
            &mut f.include_lava_in_encounter_shuffle,
        );
    });
}
