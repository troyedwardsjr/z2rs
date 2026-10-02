# Enhancements

z2rs has a set of optional changes to the game, many of them inspired by ZALiA (Zelda Again: Link is Adventuresome), HoverBat's fan remake of Zelda II. ZALiA's code is not used: it served only as a list of ideas, and every option was written for z2rs on top of the original game's own code.

Everything is off by default. With every option off you get the original game, unchanged.

## Where to set them

The desktop launcher has an Enhancements tab. It saves your choices and passes them to the game when you press Play. Its ZALiA preset button turns on the options that match ZALiA's defaults.

In the game, press `O` (or LB+RB+Y on a gamepad) to open the options menu. It has three tabs: Game, Gameplay and Graphics. Offline, the menu pauses and mutes the game while it is open. Changes are saved to `z2-native.json` when you close the menu or quit.

From a terminal, `--enh-json` takes the gameplay options as JSON and `--display-enh-json` the display options, either inline or as `@PATH` to a file.

The browser and Android builds do not have these options yet.

## Gameplay options

### Text and HUD

- Dialogue speed from the original pace up to instant text.
- Show the Shield spell as PROTECT in the pause menu.

### Quality of life

- Continue from the palace entrance or the last town you visited instead of the North Palace.
- Keep part of your experience after a game over.
- Start each continue with 3 lives plus one per Life Doll found.
- Enter towns from the side you approach them.
- Wise men refill your magic when they teach a spell.
- Learn spells without the magic container requirement.
- A softlock escape on the overworld: hold Select+A+B while paused for three seconds.

### Engine fixes

- Fixes for the level-up window softlocks.
- No skipped sword checks while Link is flashing after a hit.
- Jumps that behave the same whichever way Link faces.
- A shield hitbox that matches on both sides.
- Crumbling floors that react under both feet.
- Experience drain that stops cleanly at zero.

### Enemies and bosses

- Iron Knuckles that come for you, Ra with less health, a fix for the Stalfos upward thrust, Wizzrobes that teleport across the whole room, fairer Magos.
- Bosses that wait a moment before their first attack, a fairer Helmethead, a Carock that stays vulnerable longer, and a Barba that aims at you.

### Abilities

- Double jump.
- Stab frenzy: hold B to keep stabbing.
- A longer sword reach.
- Damage reduction.
- Slow magic regeneration.
- Reflect works on more kinds of shots.
- Run faster while holding B.
- A rescue fairy that saves you once per room from lava or water.
- Flute warp: Select+B on the overworld takes you to the next town you have visited.

### Randomizer and start

- Starting levels, spells, items, sword techniques and containers.
- Enemy hit points, enemy damage, experience, level costs and spell costs scaled by up to 25% either way, with a seeded spread per enemy.
- Palette randomization.
- A shuffle of the eight major items.

This group changes the game while it runs. The full ROM randomizer is described in [randomizer.md](randomizer.md). The two can be used together; see that guide for how they combine.

### Cheats

- Invincibility (lava still kills), infinite magic, infinite lives and maximum stats.

## Display options

These change only what you see and hear, so they are never part of the online co-op check:

- a quest timer
- screen shake when a boss dies or Thunder is cast
- the colour of the background flash, including no flash or a slow pulse
- brightness, saturation, bloom, blur and scanlines
- separate music and sound effect volumes (`M` mutes music and `N` mutes sound effects)
- fewer low health beeps
- developer overlays: hitboxes, position, hit points and frame count

## Online play, movies and save states

Gameplay options must match for online co-op. Players with different options cannot connect. During online play the gameplay options in the in-game menu are read-only, but display options can still be changed.

Movie playback ignores the gameplay options saved in the config, because they would make the movie go out of sync. An `--enh-json` given on the command line still applies.

Save states include the enhancement state, and save states from earlier versions still load.

## What is not included

Some parts of ZALiA need new levels, art or music, so z2rs does not have them: the new areas and palaces, new items as items (their effects are available as the options above), the extra spells, the second quest Ganon fight, the soundtracks and community skins, the full dungeon and room randomizer, the pause menu maps, and the seasonal modes.
