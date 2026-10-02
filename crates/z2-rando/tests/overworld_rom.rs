//! ROM-gated checks for the `overworld` module: run the randomizer with
//! overworld options over several seeds and read the patched maps, tables
//! and reveal data back.
//!
//! `Z2_ROM=/path/to/zelda2.nes cargo test -p z2-rando --test overworld_rom
//! -- --ignored`

use z2_rando::flags::{Biome, EncounterRate, Flags, Tri};
use z2_rando::overworld::continent::{Continent, Reveal};
use z2_rando::overworld::loc::{self, Class};
use z2_rando::overworld::map::{self, Cont, MAP_W};
use z2_rando::overworld::terrain::Terrain;
use z2_rando::overworld::{hidden, write};
use z2_rando::rom::Rom;
use z2_rando::{randomize, Output};

const SEEDS: [&str; 6] = ["1", "2", "race", "hello", "z2rs", "999999"];

fn body() -> Vec<u8> {
    z2_assets::rom::open().expect("Z2_ROM must name the verified ROM")
}

/// Decode a continent's map as the game will load it.
fn loaded_map(rom: &Rom, c: Cont) -> map::Decoded {
    let ptr = rom.read_cpu_word(c.bank(), c.pointer_addr()).unwrap();
    let off = rom.cpu_offset(c.bank(), ptr).unwrap();
    let len = (rom.prg().len() - off).min(map::BIG_BUDGET);
    let rows = if ptr == c.new_map_addr() {
        map::MAP_ROWS
    } else {
        c.vanilla_rows()
    };
    map::decode(rom.read_slice(off, len).unwrap(), rows, Terrain::Water).unwrap()
}

fn window(rom: &Rom) -> u16 {
    if rom.read_cpu(0, 0x87F8).unwrap() == 0x7A {
        map::BIG_WINDOW
    } else {
        map::VANILLA_WINDOW
    }
}

/// Every check that holds for any overworld output.
fn check_output(vanilla: &Rom, out: &Output, what: &str) {
    let rom = Rom::from_body(&out.body).unwrap();
    let win = window(&rom);
    for c in Cont::ALL {
        let d = loaded_map(&rom, c);
        if d.len > map::VANILLA_BUDGET {
            assert_eq!(
                win,
                map::BIG_WINDOW,
                "{what}: {} needs the big copy",
                c.name()
            );
        }
        let table = loc::read_table(&rom, c).unwrap();
        // Every location shown on the map is in the playable map and does
        // not share its tile with another one (twins excepted).
        let mut seen = std::collections::BTreeMap::new();
        for s in loc::slots(c) {
            let l = table[usize::from(s.slot)];
            if l.raw_y == 0 || l.raw_y == 0x7F {
                continue;
            }
            let (r, col) = l
                .pos()
                .unwrap_or_else(|| panic!("{what}: {} off map", s.name));
            assert!(r < map::MAP_ROWS && col < MAP_W);
            if let Some(prev) = seen.insert((r, col), s.slot) {
                assert!(
                    c == Cont::East && [22u8, 28].contains(&prev) && [22u8, 28].contains(&s.slot),
                    "{what}: {} slots {prev} and {} share ({r},{col})",
                    c.name(),
                    s.slot
                );
            }
            let t = d.grid.get(r, col);
            assert!(
                t != Terrain::Mountain && t != Terrain::Water,
                "{what}: {} {} sits on {t:?}",
                c.name(),
                s.name
            );
        }
    }
    // Stone tables point at palace runs.
    for (bank, n) in [(1u8, 3u16), (2, 4)] {
        for k in 0..n {
            let id = rom.read_cpu(bank, 0x8797 + k).unwrap();
            let ptr = rom.read_cpu_word(bank, 0x878F + 2 * k).unwrap();
            if id == 0xFF || ptr == 0 || (bank == 1 && k == 3) {
                continue;
            }
            if bank == 2 && k == 2 && rom.read_cpu(2, 0x87A5).unwrap() != 2 {
                continue;
            }
            let c = match (bank, k) {
                (1, _) => Cont::West,
                (2, 3) => Cont::Maze,
                _ => Cont::East,
            };
            let d = loaded_map(&rom, c);
            let off = usize::from(ptr - win);
            let byte = rom
                .read(
                    rom.cpu_offset(
                        c.bank(),
                        rom.read_cpu_word(c.bank(), c.pointer_addr()).unwrap(),
                    )
                    .unwrap()
                        + off,
                )
                .unwrap();
            assert_eq!(byte >> 4, 0, "{what}: stone {bank}/{k} hits a long run");
            let tile = (0..map::MAP_ROWS * MAP_W).find(|&i| d.run_of[i] == off);
            let tile = tile.unwrap_or_else(|| panic!("{what}: stone {bank}/{k} outside the map"));
            // The tile is a palace slot's tile (palace 1-6 per the id).
            let (tr, tc) = (tile / MAP_W, tile % MAP_W);
            let table = loc::read_table(&rom, c).unwrap();
            let slot = [52usize, 53, 54]
                .into_iter()
                .find(|&s| {
                    let l = table[s];
                    l.pos() == Some((tr, tc))
                })
                .or_else(|| {
                    // A hidden palace slot keeps a zero row in the table
                    // (flute data at $DF66/$DF68, hammer at $DF67/$DF69).
                    (0..2u16).find_map(|k| {
                        let hid = usize::from(rom.read_cpu(7, 0xDF66 + k).unwrap());
                        let y = rom.read_cpu(7, 0xDF68 + k).unwrap() & 0x7F;
                        (c == Cont::East
                            && hid < table.len()
                            && usize::from(y) == tr + 30
                            && table[hid].x as usize == tc)
                            .then_some(hid)
                    })
                });
            assert!(
                slot.is_some(),
                "{what}: stone {bank}/{k} at ({tr},{tc}) is not a palace spot; palace slots {:?}; flute {:02X} {:02X}",
                [52usize, 53, 54].map(|s| (s, table[s].raw_y, table[s].x)),
                rom.read_cpu(7, 0xDF66).unwrap(),
                rom.read_cpu(7, 0xDF68).unwrap()
            );
        }
    }
    // Raft docks as bank 0 knows them: every raft on a map is one of the
    // two dock spots.
    let docks = [
        (
            rom.read_cpu(0, 0x8528).unwrap(),
            rom.read_cpu(0, 0x852A).unwrap(),
        ),
        (
            rom.read_cpu(0, 0x8529).unwrap(),
            rom.read_cpu(0, 0x852B).unwrap(),
        ),
    ];
    for c in Cont::ALL {
        let l = loc::read_table(&rom, c).unwrap()[41];
        let on = l.raw_y != 0 && l.raw_y != 0x7F && loc::info(c, 41).is_some();
        let used = on && l.external && (l.world & 3) != 3;
        if used && c != Cont::DeathMountain && c != Cont::Maze {
            assert!(
                docks.contains(&(l.x, l.raw_y)),
                "{what}: {} raft dock listed",
                c.name()
            );
        }
    }
    // Hidden location data agrees with the East map and table.
    let east = loc::read_table(&rom, Cont::East).unwrap();
    let d = loaded_map(&rom, Cont::East);
    let call_y = rom.read_cpu(2, 0x8372).unwrap();
    if call_y != 0xFF {
        let slot = usize::from(rom.read_cpu(7, 0xDF66).unwrap());
        let y = rom.read_cpu(7, 0xDF68).unwrap() & 0x7F;
        assert_eq!(east[slot].raw_y, 0, "{what}: flute location starts hidden");
        assert_eq!(call_y + 2, y, "{what}: call spot two rows above");
        assert_eq!(rom.read_cpu(2, 0x8378).unwrap(), east[slot].x);
        let (r, col) = (usize::from(y - 30), usize::from(east[slot].x));
        let run = rom
            .read(
                rom.cpu_offset(2, rom.read_cpu_word(2, 0x8508).unwrap())
                    .unwrap()
                    + d.run_of[r * MAP_W + col],
            )
            .unwrap();
        assert_eq!(
            run,
            rom.read_cpu(7, 0xDF60).unwrap(),
            "{what}: flute cover run"
        );
        let call = d.grid.get(r - 2, col);
        assert!(call.is_open(), "{what}: call spot on {call:?}");
    }
    let _ = vanilla;
}

#[test]
#[ignore = "needs Z2_ROM"]
fn vanilla_data_reads_back() {
    let rom = Rom::from_body(&body()).unwrap();
    for c in Cont::ALL {
        let k = Continent::vanilla(&rom, c).unwrap();
        // Encoding the vanilla grid and decoding it again is lossless.
        let e = map::encode(&k.grid, &[]);
        let d = map::decode(&e.bytes, map::MAP_ROWS, Terrain::Water).unwrap();
        assert_eq!(d.grid, k.grid, "{}", c.name());
        assert!(e.bytes.len() <= map::VANILLA_BUDGET, "{}", c.name());
        assert!(k.pos(0).is_some() || c != Cont::West);
    }
    let east = Continent::vanilla(&rom, Cont::East).unwrap();
    assert_eq!(east.hidden[53], Some(Reveal::Flute));
    assert_eq!(east.hidden[49], Some(Reveal::Hammer));
    assert_eq!(east.pos(53), Some((72, 45)));
    assert_eq!(east.pos(49), Some((51, 61)));
    assert_eq!(east.icon[53], Terrain::Palace);
    assert_eq!(east.cover[53], Terrain::Desert);
    assert_eq!(east.cover[49], Terrain::Forest);
    // The reveal data we compute for the vanilla state is the vanilla data.
    let mut copy = rom.clone();
    hidden::write(&mut copy, &east, false).unwrap();
    assert_eq!(copy.body(), rom.body(), "reveal data round trip");
    // Rewriting an unchanged world changes nothing.
    let conts: Vec<Continent> = Cont::ALL
        .iter()
        .map(|&c| Continent::vanilla(&rom, c).unwrap())
        .collect();
    let mut copy = rom.clone();
    write::write_all(&mut copy, &conts, false, true).unwrap();
    assert_eq!(copy.body(), rom.body(), "no-op write");
    // The palace metatile is TL, BL, TR, BR.
    assert_eq!(
        hidden::metatile(&rom, Terrain::Palace).unwrap(),
        [0x60, 0x61, 0x62, 0x63]
    );
    // Palace tables: a re-encoded West keeps its stone pointers on the
    // palace runs.
    let mut conts2 = conts.clone();
    conts2[0].table_changed = true;
    let mut copy = rom.clone();
    write::write_all(&mut copy, &conts2, false, false).unwrap();
    assert_eq!(
        copy.read_cpu_word(1, 0x8508).unwrap(),
        Cont::West.new_map_addr()
    );
    for k in 0..2u16 {
        assert_eq!(
            copy.read_cpu_word(2, 0x878F + 2 * k).unwrap(),
            rom.read_cpu_word(2, 0x878F + 2 * k).unwrap(),
            "East stone entries untouched"
        );
    }
    let _ = Class::Start;
}

fn shuffle_all() -> Flags {
    let mut f = Flags::default();
    f.overworld.west_biome = Biome::VanillaShuffle;
    f.overworld.east_biome = Biome::VanillaShuffle;
    f.overworld.dm_biome = Biome::VanillaShuffle;
    f.overworld.maze_biome = Biome::VanillaShuffle;
    f
}

#[test]
#[ignore = "needs Z2_ROM"]
fn vanilla_shuffle_seeds_hold_together() {
    let b = body();
    let vanilla = Rom::from_body(&b).unwrap();
    for (k, seed) in SEEDS.iter().enumerate() {
        let mut f = shuffle_all();
        f.overworld.shuffle_hidden_locations = if k % 2 == 0 { Tri::On } else { Tri::Off };
        f.overworld.palaces_swap_continents = if k % 3 == 0 { Tri::On } else { Tri::Off };
        f.overworld.shuffle_great_palace = Tri::On;
        let out = randomize(&b, seed, &f).unwrap_or_else(|e| panic!("{seed}: {e}"));
        check_output(&vanilla, &out, seed);
        assert_ne!(out.body, b, "{seed}: something changed");
        let again = randomize(&b, seed, &f).unwrap();
        assert_eq!(again.body, out.body, "{seed}: deterministic");
    }
}

#[test]
#[ignore = "needs Z2_ROM"]
fn encounter_options_patch_the_tables() {
    let b = body();
    let mut f = Flags::default();
    f.overworld.encounter_rate = EncounterRate::None;
    let out = randomize(&b, "x", &f).unwrap();
    let rom = Rom::from_body(&out.body).unwrap();
    assert_eq!(rom.read_cpu(0, 0x8284).unwrap(), 0x60);
    let mut f = Flags::default();
    f.overworld.shuffle_encounters = Tri::On;
    let out = randomize(&b, "y", &f).unwrap();
    let rom = Rom::from_body(&out.body).unwrap();
    let van = Rom::from_body(&b).unwrap();
    for bank in [1u8, 2] {
        let mut a: Vec<u8> = (0..10)
            .map(|i| rom.read_cpu(bank, 0x8409 + i).unwrap())
            .collect();
        let mut v: Vec<u8> = (0..10)
            .map(|i| van.read_cpu(bank, 0x8409 + i).unwrap())
            .collect();
        a.sort_unstable();
        v.sort_unstable();
        assert_eq!(a, v);
    }
}

#[test]
#[ignore = "needs Z2_ROM"]
fn unhidden_vanilla_east_shows_its_locations() {
    let b = body();
    let mut f = Flags::default();
    f.overworld.hide_palace = Tri::Off;
    f.overworld.hide_kasuto = Tri::Off;
    let out = randomize(&b, "open", &f).unwrap();
    let rom = Rom::from_body(&out.body).unwrap();
    let t = loc::read_table(&rom, Cont::East).unwrap();
    assert_eq!(t[53].raw_y, 102);
    assert_eq!(t[49].raw_y, 81);
    let d = loaded_map(&rom, Cont::East);
    assert_eq!(d.grid.get(72, 45), Terrain::Palace);
    assert_eq!(d.grid.get(51, 61), Terrain::Town);
    assert_eq!(rom.read_cpu(2, 0x8372).unwrap(), 0xFF);
}

const LAND_BIOMES: [Biome; 7] = [
    Biome::Vanillalike,
    Biome::Islands,
    Biome::Canyon,
    Biome::DryCanyon,
    Biome::Mountainous,
    Biome::Caldera,
    Biome::Volcano,
];

/// Print the West and East maps of an output (with `OW_DUMP=1`).
fn dump(out: &Output, what: &str) {
    if std::env::var_os("OW_DUMP").is_none() {
        return;
    }
    let rom = Rom::from_body(&out.body).unwrap();
    for c in Cont::ALL {
        let d = loaded_map(&rom, c);
        let table = loc::read_table(&rom, c).unwrap();
        let mut g = d.grid.dump(map::MAP_ROWS);
        let mut lines: Vec<Vec<char>> = g.lines().map(|l| l.chars().collect()).collect();
        for s in loc::slots(c) {
            if let Some((r, col)) = table[usize::from(s.slot)].pos() {
                if table[usize::from(s.slot)].raw_y != 0 {
                    lines[r][col] = '#';
                }
            }
        }
        g = lines
            .iter()
            .map(|l| l.iter().collect::<String>() + "\n")
            .collect();
        println!("== {what}: {} ({} bytes)\n{g}", c.name(), d.len);
    }
}

#[test]
#[ignore = "needs Z2_ROM"]
fn generated_land_biomes_hold_together() {
    let b = body();
    let vanilla = Rom::from_body(&b).unwrap();
    for biome in LAND_BIOMES {
        for seed in &SEEDS[..3] {
            let mut f = Flags::default();
            f.overworld.west_biome = biome;
            f.overworld.east_biome = biome;
            let what = format!("{biome:?}/{seed}");
            let out = randomize(&b, seed, &f).unwrap_or_else(|e| panic!("{what}: {e}"));
            dump(&out, &what);
            check_output(&vanilla, &out, &what);
        }
    }
}

/// Diagnostics: how often single land attempts fail, and why (prints with
/// `--nocapture`).
#[test]
#[ignore = "needs Z2_ROM"]
fn land_attempt_failure_reasons() {
    use z2_rando::overworld::gen::{core, land};
    use z2_rando::overworld::Resolved;
    let rom = Rom::from_body(&body()).unwrap();
    for cont in [Cont::West, Cont::East] {
        let base = Continent::vanilla(&rom, cont).unwrap();
        for biome in LAND_BIOMES {
            let mut o = Resolved::vanilla();
            o.biome[cont.index()] = biome;
            let cl = core::climate(
                z2_rando::flags::Climate::Classic,
                &[1; 16],
                Terrain::Water,
                cont == Cont::East,
            );
            let mut rng = z2_rando::rng::Rng::new(7);
            let mut why = std::collections::BTreeMap::new();
            let mut ok = 0;
            let t = std::time::Instant::now();
            for _ in 0..60 {
                match land::generate(&base, &o, &mut rng, cl.clone(), 75, 64) {
                    Ok(c) => {
                        ok += 1;
                        if ok == 1 && std::env::var_os("OW_DUMP").is_some() {
                            println!("{}", c.grid.dump(75));
                        }
                    }
                    Err(e) => *why.entry(e.to_string()).or_insert(0) += 1,
                }
            }
            println!(
                "{} {biome:?}: {ok}/60 ok in {:?}; {why:?}",
                cont.name(),
                t.elapsed()
            );
        }
    }
}

#[test]
#[ignore = "needs Z2_ROM"]
fn generated_dm_and_maze_hold_together() {
    use z2_rando::flags::{DmSize, MazeSize};
    let b = body();
    let vanilla = Rom::from_body(&b).unwrap();
    for (k, size) in [DmSize::Large, DmSize::Medium, DmSize::Small, DmSize::Tiny]
        .into_iter()
        .enumerate()
    {
        for seed in &SEEDS[..2] {
            let mut f = Flags::default();
            f.overworld.dm_biome = [Biome::Vanillalike, Biome::Islands][k % 2];
            f.overworld.maze_biome = Biome::Vanillalike;
            f.overworld.dm_size = size;
            f.overworld.maze_size = [MazeSize::Large, MazeSize::Medium, MazeSize::Small][k % 3];
            let what = format!("DM {size:?}/{seed}");
            let out = randomize(&b, seed, &f).unwrap_or_else(|e| panic!("{what}: {e}"));
            dump(&out, &what);
            check_output(&vanilla, &out, &what);
        }
    }
}

/// Whole presets (every module on) still produce sound overworlds.
#[test]
#[ignore = "needs Z2_ROM"]
fn presets_produce_sound_overworlds() {
    use z2_rando::flags::Preset;
    let b = body();
    let vanilla = Rom::from_body(&b).unwrap();
    for p in [Preset::Standard, Preset::MaxRando] {
        for seed in SEEDS {
            let what = format!("{p:?}/{seed}");
            let t = std::time::Instant::now();
            let out = randomize(&b, seed, &p.flags()).unwrap_or_else(|e| panic!("{what}: {e}"));
            println!("{what}: {} attempts, {:?}", out.attempts, t.elapsed());
            check_output(&vanilla, &out, &what);
        }
    }
}

/// Shuffled connections: every connector leads to a continent that leads
/// back through the same connector type.
#[test]
#[ignore = "needs Z2_ROM"]
fn shuffled_connections_pair_up() {
    use z2_rando::flags::ContinentConnections;
    let b = body();
    let vanilla = Rom::from_body(&b).unwrap();
    for mode in [
        ContinentConnections::TransportationShuffle,
        ContinentConnections::AnythingGoes,
    ] {
        for seed in &SEEDS[..4] {
            let mut f = Flags::default();
            f.overworld.west_biome = Biome::Vanillalike;
            f.overworld.east_biome = Biome::Islands;
            f.overworld.dm_biome = Biome::Vanillalike;
            f.overworld.maze_biome = Biome::Vanillalike;
            f.overworld.continent_connections = mode;
            let what = format!("{mode:?}/{seed}");
            let out = randomize(&b, seed, &f).unwrap_or_else(|e| panic!("{what}: {e}"));
            check_output(&vanilla, &out, &what);
            let rom = Rom::from_body(&out.body).unwrap();
            let region = |c: Cont| match c {
                Cont::West => 0u8,
                Cont::DeathMountain | Cont::Maze => 1,
                Cont::East => 2,
            };
            let mut ends = vec![0usize; 4];
            for c in Cont::ALL {
                let t = loc::read_table(&rom, c).unwrap();
                for slot in 40..=43usize {
                    let l = t[slot];
                    if l.raw_y == 0 || l.raw_y == 0x7F || !l.external {
                        continue;
                    }
                    ends[slot - 40] += 1;
                    let dest = l.world & 3;
                    assert_ne!(
                        dest,
                        region(c),
                        "{what}: {} slot {slot} leads home",
                        c.name()
                    );
                    assert_eq!(usize::from(l.map), slot, "{what}: connector area");
                }
            }
            assert_eq!(ends, vec![2; 4], "{what}: every connector has two ends");
            println!(
                "{what}: {}",
                out.spoiler
                    .lines()
                    .filter(|l| l.contains("Connection"))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
    }
}

/// Many random overworld option mixes: every one generates, holds together
/// and is deterministic.
#[test]
#[ignore = "needs Z2_ROM"]
fn random_option_mixes_hold_together() {
    use z2_rando::flags::{
        Climate, ContinentConnections, ContinentSize, DmSize, FlagEnum, LessImportantLocations,
        MazeSize, RiverDevilBlocker,
    };
    use z2_rando::rng::Rng;
    let b = body();
    let vanilla = Rom::from_body(&b).unwrap();
    let mut rng = Rng::new(2024);
    let biomes = [
        Biome::Vanilla,
        Biome::VanillaShuffle,
        Biome::Vanillalike,
        Biome::Islands,
        Biome::Canyon,
        Biome::DryCanyon,
        Biome::Mountainous,
        Biome::Volcano,
        Biome::Caldera,
        Biome::Random,
    ];
    let tri = |rng: &mut Rng| [Tri::Off, Tri::On, Tri::Random][rng.index(3)];
    let n: usize = std::env::var("OW_MIXES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let mut slowest = std::time::Duration::ZERO;
    for k in 0..n {
        let mut f = Flags::default();
        let o = &mut f.overworld;
        o.west_biome = biomes[rng.index(biomes.len())];
        o.east_biome = biomes[rng.index(biomes.len())];
        o.dm_biome = biomes[rng.index(biomes.len())];
        o.maze_biome = [
            Biome::Vanilla,
            Biome::VanillaShuffle,
            Biome::Vanillalike,
            Biome::Random,
        ][rng.index(4)];
        o.west_size = [
            ContinentSize::Large,
            ContinentSize::Medium,
            ContinentSize::Small,
        ][rng.index(3)];
        o.east_size = [
            ContinentSize::Large,
            ContinentSize::Medium,
            ContinentSize::Small,
        ][rng.index(3)];
        o.dm_size = [DmSize::Large, DmSize::Medium, DmSize::Small, DmSize::Tiny][rng.index(4)];
        o.maze_size = [MazeSize::Large, MazeSize::Medium, MazeSize::Small][rng.index(3)];
        o.west_climate = <Climate as FlagEnum>::all()[rng.index(7)];
        o.east_climate = <Climate as FlagEnum>::all()[rng.index(7)];
        o.dm_climate = <Climate as FlagEnum>::all()[rng.index(7)];
        o.good_boots = tri(&mut rng);
        o.continent_connections = [
            ContinentConnections::Normal,
            ContinentConnections::TransportationShuffle,
            ContinentConnections::AnythingGoes,
        ][rng.index(3)];
        o.less_important_locations = [
            LessImportantLocations::BlendIn,
            LessImportantLocations::Isolate,
            LessImportantLocations::Remove,
            LessImportantLocations::Random,
        ][rng.index(4)];
        o.generate_bagu_woods = tri(&mut rng);
        o.restrict_connection_cave_shuffle = tri(&mut rng);
        o.allow_connection_caves_blocked = rng.coin();
        o.river_devil_blocker = [
            RiverDevilBlocker::Path,
            RiverDevilBlocker::Cave,
            RiverDevilBlocker::Siege,
            RiverDevilBlocker::Random,
        ][rng.index(4)];
        o.east_rocks = tri(&mut rng);
        o.hide_palace = tri(&mut rng);
        o.hide_kasuto = tri(&mut rng);
        o.shuffle_hidden_locations = tri(&mut rng);
        o.palaces_swap_continents = tri(&mut rng);
        o.shuffle_great_palace = tri(&mut rng);
        o.encounter_rate = [
            EncounterRate::None,
            EncounterRate::Half,
            EncounterRate::Normal,
            EncounterRate::Random,
        ][rng.index(4)];
        o.shuffle_encounters = tri(&mut rng);
        o.legacy_vanilla_shuffled_locations = rng.coin();
        let seed = format!("mix{k}");
        let t = std::time::Instant::now();
        let out =
            randomize(&b, &seed, &f).unwrap_or_else(|e| panic!("{seed} {:?}: {e}", f.overworld));
        slowest = slowest.max(t.elapsed());
        if t.elapsed() > std::time::Duration::from_secs(5) {
            println!(
                "{seed}: {:?} over {} attempts: {:?}\n  {:?}",
                t.elapsed(),
                out.attempts,
                f.overworld,
                out.log
            );
        }
        check_output(&vanilla, &out, &seed);
        if k % 8 == 0 {
            let again = randomize(&b, &seed, &f).unwrap();
            assert_eq!(again.body, out.body, "{seed}: deterministic");
        }
    }
    println!("{n} mixes, slowest {slowest:?}");
}
