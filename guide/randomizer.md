# Randomizer

z2rs can build a randomized Zelda II from your own ROM. The overworld, palaces, items, enemies, experience tables and spells can all be shuffled or regenerated, and the game then runs from the new ROM image. Nothing is written to disk except the optional spoiler log. The randomized ROM only exists in memory while you play.

The options follow the community [Z2Randomizer](https://github.com/Ellendar/Z2Randomizer) by Digshake and Ellendar, so players who know that tool will recognise most of them. z2rs does not use any of its code or data. Every feature was written again for z2rs from a description of what it does, the 6502 patches are new code, and the rooms, maps and text all come from your ROM. Seeds are not compatible between the two tools: the same seed and flags give a different game in each.

## Playing a seed

In the desktop launcher, open the ROM randomizer tab and tick "Play a randomized game". Then:

1. Type a seed or press Random seed. Any text works as a seed.
2. Pick a preset (Beginner, Standard or Max rando), or set the options yourself in the sections below it.
3. Press Play.

The Flags field holds every option as one short string. Copy it with Copy flag string and paste it into someone else's launcher, and with the same seed they get the same game. Racers also need the same z2rs version, because a generator change gives a different game for the same seed and flags. The flag string `1` means no changes at all.

When you load a file, the file select screen shows a six character hash in place of the "SELECT" heading. Two players with the same hash have the same game.

From a terminal:

```sh
./z2rs --rom /path/to/zelda2.nes --seed "my seed" --rando-flags 1NCgCABhi_wgAiIgAAAACDddYAGBVAbFAgDG4UTIkRG4
./z2rs --rom /path/to/zelda2.nes --seed race1 --rando-flags FLAGS --rando-spoiler spoiler.txt
```

| Flag | What it does |
|---|---|
| `--seed TEXT` | the seed; used together with `--rando-flags` |
| `--rando-flags STRING` | the flag string from the launcher |
| `--rando-spoiler PATH` | write a spoiler log listing where everything ended up |
| `--sprite-ips PATH` | apply your own player sprite patch (see below) |

The browser build has a Randomizer box with the same seed and flag string fields. Android does not have the randomizer yet.

## Presets

| Preset | Flag string |
|---|---|
| Vanilla | `1` |
| Beginner | `1XCQAAD_8BgASKAAgUAAAIACEqIncQ` |
| Standard | `1NCgCABhi_wgAiIgAAAACDddYAGBVAbFAgDG4UTIkRG4` |
| Max rando | `1ZCgDABhi5fwgCqq_9LBgTLKFlLlMAAAG1KoCWpYtc2cAG2tqsTKmJjc` |

## What can be randomized

### Start

Starting items, spells, sword techniques, heart and magic containers, lives and attack, magic and life levels. Each can be fixed, shuffled or left as in the original game.

### Overworld

Locations can be shuffled on the original maps, or each continent can be generated from scratch: West and East Hyrule in several biomes (vanilla-like, islands, canyon, dry canyon, mountainous, caldera, volcano) and climates, Death Mountain as cave pockets inside the mountain, and Maze Island as a real maze. Continent sizes, how the continents connect, hidden Palace 6 and hidden New Kasuto, Bagu's woods, the river devil, boulders and encounter rates are all options.

### Palaces

Ten layout styles build new palaces out of the rooms in your ROM, including reconstructed, random walk, sequential, tower and mirrored layouts. Length, item rooms per palace, where boss rooms exit, the distance to Dark Link, Thunderbird, palace colours and how many palaces must be finished are options.

### Items

Palace and overworld items can be shuffled separately or together, with the P-bag caves, small items, extra keys and the experience in P-bags. Every seed is checked before you play it: the generator only accepts a layout that can be finished.

### Enemies and stats

Overworld and palace enemies can be shuffled, enemy and boss hit points scaled, sword immunity and experience stealing moved between enemies, and the experience needed per level, the level caps and the attack, magic and life tables changed.

### Drops

The small and large drop pools, how often enemies drop, and whether drops come at a fixed rhythm.

### Spells

Which wizard teaches which spell, Fire linked to another spell or replaced by Dash (run at double speed), up and down stab swapped, jump always on, permanent sword beam, life spell strength, and the enemy the Spell spell creates. Knockback can be randomized per enemy.

### Hints and towns

Townspeople can give item and spell hints, town signs can say what their wizard teaches, idle townsfolk get new lines, false walls and hidden jars can be shown, the New Kasuto magic container requirement can change, and wizard visits can be shortened.

### Quality of life

Faster text, fewer low health beeps or none, no flashing on death, faster spell casting, Up+Select on controller 1 for the save prompt, and a darker Thunderbird room.

### Looks

Tunic, skin, outline and shield colours, the sword beam graphic, shuffled enemy palettes and music on or off. Random colours come from the seed, so a shared seed looks the same for everyone.

## Your own sprite

`--sprite-ips`, or the sprite patch picker in the launcher's Cosmetics section, applies an IPS patch to Link's graphics. z2rs does not ship any character sprites. Only the graphics and Link's palette bytes in the patch are applied; anything else in it is skipped and listed in the log.

## Saves and online play

A randomized seed gets its own battery save and save state files, named after the seed hash, so it never overwrites your normal game or another seed.

For online co-op, both players need the same seed and flags. The game checks the randomized ROM during the connection handshake and refuses a mismatch.

## Enhancements on top of a seed

The Enhancements tab (see [enhancements.md](enhancements.md)) also has a small "Randomizer and start" group that works at run time. It can be combined with a seed. Its starting loadout, stat scaling and palette options apply on top of the randomized ROM. Its item shuffle is turned off while a seed is in use, because it assumes the original item locations.

## Not done yet

These options are accepted and saved but do nothing yet. The launcher marks them as not available, and no preset turns them on:

- spells, sword techniques and quest items as part of the item pool
- flute warp and a harder Carock
- the updated HUD and a steady HUD on lag frames
- custom music and extra room packs
- links between West Hyrule and Maze Island, and between East Hyrule and Death Mountain

About one in a hundred combinations of fully random options cannot produce a game and reports an error. Every preset works.
