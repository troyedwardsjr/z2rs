//! ROM-gated: randomized overworlds load and play.
//!
//! Each test builds a randomized game through the app's seam, starts a new
//! file, walks out of the North Palace onto the overworld, then walks the
//! map (a path computed from the patched map data) into a town and checks
//! that the game entered it.
//!
//! Run with `Z2_ROM=/path/to/zelda2.nes cargo test -p z2-native --test
//! rando_overworld -- --ignored --nocapture`.

use z2_native::app::{self, Emu, Features};
use z2_native::rando::RandoSpec;
use z2_rando::flags::{Biome, EncounterRate, Flags};
use z2_rando::overworld::loc;
use z2_rando::overworld::map::{self, Cont};
use z2_rando::overworld::terrain::Terrain;
use z2_rando::rom::Rom;

const A: u8 = 0x01;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;
const RIGHT: u8 = 0x80;

fn rom_body() -> Vec<u8> {
    z2_assets::rom::open().expect("Z2_ROM must name the verified ROM")
}

fn build(body: &[u8], seed: &str, flags: &Flags) -> Emu {
    let fs = flags.to_flag_string();
    let spec = RandoSpec::from_cli(Some(seed), Some(&fs), None, None)
        .unwrap()
        .unwrap()
        .leak();
    let feats = Features {
        rando: Some(spec),
        ..Features::default()
    };
    app::emu_from_rom_body_with(body, 44_100, feats).unwrap()
}

/// With `OW_PNG=<dir>`, save the current frame as `<dir>/<name>.png`.
fn snap(emu: &Emu, name: &str) {
    if let Some(dir) = std::env::var_os("OW_PNG") {
        let frame: &[u8; z2_native::app::FRAME_LEN] =
            emu.game.frame_indexed()[..].try_into().expect("frame size");
        let png = z2_ppu::encode_indexed_png(frame);
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        std::fs::write(path, png).expect("write png");
    }
}

fn step(emu: &mut Emu, frames: usize, input: u8) {
    app::step_frames(emu, &vec![input; frames], None);
}

fn start_game(emu: &mut Emu) {
    step(emu, 30, 0);
    step(emu, 5, START);
    step(emu, 20, 0);
    step(emu, 5, START);
    step(emu, 20, 0);
    for _ in 0..8 {
        step(emu, 2, A);
        step(emu, 8, 0);
    }
    for _ in 0..3 {
        step(emu, 2, SELECT);
        step(emu, 8, 0);
    }
    step(emu, 5, START);
    step(emu, 30, 0);
    step(emu, 5, START);
    step(emu, 170, 0);
    assert_eq!(emu.game.ram[0x0736], 0x0B, "side-view gameplay");
}

/// The West map and table as the patched game will load them.
fn west_of(out: &z2_rando::Output) -> (map::Grid, Vec<z2_rando::overworld::loc::Loc>) {
    let rom = Rom::from_body(&out.body).unwrap();
    let c = Cont::West;
    let ptr = rom.read_cpu_word(c.bank(), c.pointer_addr()).unwrap();
    let off = rom.cpu_offset(c.bank(), ptr).unwrap();
    let len = (rom.prg().len() - off).min(map::BIG_BUDGET);
    let d = map::decode(rom.read_slice(off, len).unwrap(), 75, Terrain::Water).unwrap();
    (d.grid, loc::read_table(&rom, c).unwrap())
}

/// Shortest walk (row, col) from `from` to `to` over open terrain that
/// steps on no location tile except `to`.
fn path(
    g: &map::Grid,
    blocked: &[(usize, usize)],
    from: (usize, usize),
    to: (usize, usize),
) -> Option<Vec<(usize, usize)>> {
    let mut prev = vec![usize::MAX; 64 * 75];
    let mut q = std::collections::VecDeque::new();
    prev[from.0 * 64 + from.1] = from.0 * 64 + from.1;
    q.push_back(from);
    while let Some((r, c)) = q.pop_front() {
        if (r, c) == to {
            let mut v = vec![to];
            let mut k = to.0 * 64 + to.1;
            while k != from.0 * 64 + from.1 {
                k = prev[k];
                v.push((k / 64, k % 64));
            }
            v.reverse();
            return Some(v);
        }
        if (r, c) != from && blocked.contains(&(r, c)) {
            continue;
        }
        for (dr, dc) in [(0i32, 1i32), (0, -1), (1, 0), (-1, 0)] {
            let (nr, nc) = (r as i32 + dr, c as i32 + dc);
            if !(0..75).contains(&nr) || !(0..64).contains(&nc) {
                continue;
            }
            let (nr, nc) = (nr as usize, nc as usize);
            if prev[nr * 64 + nc] != usize::MAX || !g.get(nr, nc).is_open() {
                continue;
            }
            prev[nr * 64 + nc] = r * 64 + c;
            q.push_back((nr, nc));
        }
    }
    None
}

/// Walk Link along `p` (closed loop on `$73`/`$74`). Returns false if a
/// step did not happen.
fn walk(emu: &mut Emu, p: &[(usize, usize)]) -> bool {
    for w in p.windows(2) {
        let (a, b) = (w[0], w[1]);
        let input = if b.1 > a.1 {
            0x80
        } else if b.1 < a.1 {
            0x40
        } else if b.0 > a.0 {
            0x20
        } else {
            0x10
        };
        let want = (b.0 as u8 + 30, b.1 as u8);
        let mut ok = false;
        for _ in 0..80 {
            step(emu, 1, input);
            let r = &emu.game.ram;
            if r[0x0736] != 0x05 || (r[0x73], r[0x74]) == want {
                ok = true;
                break;
            }
        }
        if !ok {
            return false;
        }
        if emu.game.ram[0x0736] != 0x05 {
            return true;
        }
        // Let the scroll settle on the tile.
        step(emu, 16, 0);
    }
    true
}

/// Start, leave the North Palace, walk into a West town; returns the town
/// slot entered.
fn enter_a_town(seed: &str, f: &Flags) -> u8 {
    let body = rom_body();
    let out = z2_rando::randomize(&body, seed, f).unwrap();
    let (g, table) = west_of(&out);
    let mut emu = build(&body, seed, f);
    start_game(&mut emu);
    for _ in 0..900 {
        step(&mut emu, 1, RIGHT);
        if emu.game.ram[0x0736] == 0x05 {
            break;
        }
    }
    for i in 0..40 {
        step(&mut emu, 1, 0);
        let r = &emu.game.ram;
        if r[0x0736] != 0x05 {
            println!(
                "{i}: mode {:02X} region {} y {} x {} slot {}",
                r[0x736], r[0x706], r[0x73], r[0x74], r[0x748]
            );
        }
    }
    assert_eq!(emu.game.ram[0x0736], 0x05, "{seed}: on the overworld");
    let here = (
        usize::from(emu.game.ram[0x73] - 30),
        usize::from(emu.game.ram[0x74]),
    );
    let np = table[0].pos().unwrap();
    assert!(
        here.0.abs_diff(np.0) + here.1.abs_diff(np.1) <= 1,
        "{seed}: Link stands by the North Palace ({here:?} vs {np:?})"
    );
    let spots: Vec<(usize, usize)> = table
        .iter()
        .filter_map(|l| if l.raw_y == 0 { None } else { l.pos() })
        .collect();
    // Prefer a town or palace; any location proves the entry path too.
    let preferred = [45usize, 47, 51, 48, 49, 52, 53, 54];
    let mut best: Option<(u8, Vec<(usize, usize)>)> = None;
    for pass in 0..2 {
        for (t, l) in table.iter().enumerate() {
            if t == 0 || l.raw_y == 0 || (pass == 0 && !preferred.contains(&t)) {
                continue;
            }
            let Some(tp) = l.pos() else { continue };
            if let Some(p) = path(&g, &spots, here, tp) {
                if best.as_ref().is_none_or(|(_, b)| p.len() < b.len()) {
                    best = Some((t as u8, p));
                }
            }
        }
        if best.is_some() {
            break;
        }
    }
    let (town, p) = best.unwrap_or_else(|| panic!("{seed}: nothing reachable on foot"));
    snap(&emu, &format!("{seed}-overworld"));
    println!("{seed}: walking {} tiles to town slot {town}", p.len());
    assert!(walk(&mut emu, &p), "{seed}: walk got stuck");
    step(&mut emu, 120, 0);
    let r = &emu.game.ram;
    assert_ne!(r[0x0736], 0x05, "{seed}: left the overworld");
    snap(&emu, &format!("{seed}-entered"));
    // The game keeps the slot minus its entrance index (the slot of the
    // area's first entrance).
    let want = town - table[usize::from(town)].entrance;
    assert_eq!(r[0x0748], want, "{seed}: entered slot {town}");
    step(&mut emu, 300, 0);
    assert_eq!(emu.game.exec_errors, 0, "{seed}");
    town
}

#[test]
#[ignore = "needs Z2_ROM"]
fn shuffled_west_walks_into_a_town() {
    for seed in ["walk", "town", "z2rs"] {
        let mut f = Flags::default();
        f.overworld.west_biome = Biome::VanillaShuffle;
        f.overworld.east_biome = Biome::VanillaShuffle;
        f.overworld.encounter_rate = EncounterRate::None;
        enter_a_town(seed, &f);
    }
}

#[test]
#[ignore = "needs Z2_ROM"]
fn generated_west_walks_into_a_town() {
    let biomes = [
        Biome::Vanillalike,
        Biome::Islands,
        Biome::Canyon,
        Biome::Mountainous,
        Biome::Caldera,
    ];
    for (k, b) in biomes.into_iter().enumerate() {
        let mut f = Flags::default();
        f.overworld.west_biome = b;
        f.overworld.east_biome = Biome::Vanillalike;
        f.overworld.dm_biome = Biome::Vanillalike;
        f.overworld.maze_biome = Biome::Vanillalike;
        f.overworld.encounter_rate = EncounterRate::None;
        enter_a_town(&format!("gen{k}"), &f);
    }
}

/// Start, leave the North Palace and stand on the overworld; returns the
/// emulator and Link's tile.
fn on_overworld(body: &[u8], seed: &str, f: &Flags) -> (Emu, (usize, usize)) {
    let mut emu = build(body, seed, f);
    start_game(&mut emu);
    for _ in 0..900 {
        step(&mut emu, 1, RIGHT);
        if emu.game.ram[0x0736] == 0x05 {
            break;
        }
    }
    step(&mut emu, 40, 0);
    assert_eq!(emu.game.ram[0x0736], 0x05, "{seed}: on the overworld");
    let here = (
        usize::from(emu.game.ram[0x73] - 30),
        usize::from(emu.game.ram[0x74]),
    );
    (emu, here)
}

#[test]
#[ignore = "needs Z2_ROM"]
fn generated_raft_crosses_to_the_east() {
    let body = rom_body();
    let mut tested = 0;
    for k in 0..20 {
        if tested == 2 {
            break;
        }
        let seed = &format!("raft{k}");
        let mut f = Flags::default();
        f.overworld.west_biome = Biome::Vanillalike;
        f.overworld.east_biome = Biome::Islands;
        f.overworld.encounter_rate = EncounterRate::None;
        f.start.start_with_raft = true;
        let out = z2_rando::randomize(&body, seed, &f).unwrap();
        let (g, table) = west_of(&out);
        let rom = Rom::from_body(&out.body).unwrap();
        let east = loc::read_table(&rom, Cont::East).unwrap();
        let (mut emu, here) = on_overworld(&body, seed, &f);
        let spots: Vec<(usize, usize)> = table
            .iter()
            .filter_map(|l| if l.raw_y == 0 { None } else { l.pos() })
            .collect();
        let raft = table[41].pos().expect("West raft placed");
        let Some(p) = path(&g, &spots, here, raft) else {
            println!("{seed}: raft not reachable on foot without items, skipped");
            continue;
        };
        tested += 1;
        println!("{seed}: {} tiles to the raft at {raft:?}", p.len());
        assert!(walk(&mut emu, &p), "{seed}: walk got stuck");
        // Sail.
        let mut arrived = false;
        let mut rode = false;
        for _ in 0..1200 {
            step(&mut emu, 1, 0);
            let r = &emu.game.ram;
            rode |= r[0x0736] == 0x17;
            if r[0x0706] == 2 && r[0x0736] == 0x05 {
                arrived = true;
                break;
            }
        }
        assert!(arrived, "{seed}: reached East Hyrule");
        assert!(rode, "{seed}: by raft ride");
        step(&mut emu, 200, 0);
        let r = &emu.game.ram;
        let at = (usize::from(r[0x73]) - 30, usize::from(r[0x74]));
        let dock = east[41].pos().unwrap();
        println!("{seed}: landed at {at:?}, East raft at {dock:?}");
        assert!(
            at.0.abs_diff(dock.0) + at.1.abs_diff(dock.1) <= 2,
            "{seed}: landed by the East raft"
        );
        assert_eq!(emu.game.exec_errors, 0);
    }
    assert_eq!(tested, 2, "found two seeds to sail");
}

/// A map too big for the vanilla 896-byte copy switches the loader to the
/// 1408-byte copy at `$7A00`; the game must still load and walk it.
#[test]
#[ignore = "needs Z2_ROM"]
fn big_map_loads_through_the_larger_copy() {
    use z2_rando::flags::Climate;
    let body = rom_body();
    let mut found = false;
    for k in 0..30 {
        let mut f = Flags::default();
        f.overworld.west_biome = Biome::Islands;
        f.overworld.west_climate = Climate::Chaos;
        f.overworld.encounter_rate = EncounterRate::None;
        let seed = format!("big{k}");
        let out = z2_rando::randomize(&body, &seed, &f).unwrap();
        let rom = Rom::from_body(&out.body).unwrap();
        if rom.read_cpu(0, 0x87F8).unwrap() != 0x7A {
            continue;
        }
        println!("{seed}: uses the larger copy");
        found = true;
        enter_a_town(&seed, &f);
        break;
    }
    assert!(found, "no seed needed the larger copy");
}

/// Which ported routines a fully generated overworld turns off (they then
/// run as ROM code); printed for the record, and the hidden-location exit
/// port must stay on (its operands are read from the ROM).
#[test]
#[ignore = "needs Z2_ROM"]
fn generated_overworld_keeps_the_ports() {
    let body = rom_body();
    let mut f = Flags::default();
    f.overworld.west_biome = Biome::Vanillalike;
    f.overworld.east_biome = Biome::Vanillalike;
    f.overworld.dm_biome = Biome::Vanillalike;
    f.overworld.maze_biome = Biome::Vanillalike;
    f.overworld.shuffle_hidden_locations = z2_rando::flags::Tri::On;
    let emu = build(&body, "ports", &f);
    println!("untrapped: {:X?}", emu.rom.untrapped);
    assert!(!emu.rom.untrapped.contains(&0xCCB3));
    assert!(!emu.rom.untrapped.contains(&0xDF79));
}

/// Shuffled connections: walk onto a West raft dock or bridge that now
/// leads somewhere new and check Link arrives on the matching connector.
#[test]
#[ignore = "needs Z2_ROM"]
fn shuffled_connector_takes_link_across() {
    use z2_rando::flags::ContinentConnections;
    let body = rom_body();
    let mut tested = 0;
    for k in 0..60 {
        if tested == 2 {
            break;
        }
        let seed = format!("conn{k}");
        let mut f = Flags::default();
        f.overworld.west_biome = Biome::Vanillalike;
        f.overworld.east_biome = Biome::Vanillalike;
        f.overworld.dm_biome = Biome::Vanillalike;
        f.overworld.maze_biome = Biome::Vanillalike;
        f.overworld.continent_connections = ContinentConnections::TransportationShuffle;
        f.overworld.encounter_rate = EncounterRate::None;
        f.start.start_with_raft = true;
        let out = z2_rando::randomize(&body, &seed, &f).unwrap();
        let (g, table) = west_of(&out);
        let rom = Rom::from_body(&out.body).unwrap();
        // A raft or bridge on West that does not lead East (or a bridge).
        let Some(slot) = [41usize, 40].into_iter().find(|&s| {
            let l = table[s];
            l.raw_y != 0 && l.raw_y != 0x7F && l.external && (s == 40 || l.world & 3 != 2)
        }) else {
            continue;
        };
        let dest_region = table[slot].world & 3;
        let dest = if dest_region == 1 {
            Cont::DeathMountain
        } else {
            Cont::East
        };
        let dest_t = loc::read_table(&rom, dest).unwrap();
        let spots: Vec<(usize, usize)> = table
            .iter()
            .filter_map(|l| if l.raw_y == 0 { None } else { l.pos() })
            .collect();
        let (mut emu, here) = on_overworld(&body, &seed, &f);
        let target = table[slot].pos().unwrap();
        let Some(p) = path(&g, &spots, here, target) else {
            continue;
        };
        tested += 1;
        println!(
            "{seed}: West slot {slot} -> {} ({} tiles)",
            dest.name(),
            p.len()
        );
        assert!(walk(&mut emu, &p), "{seed}: walk got stuck");
        let mut arrived = false;
        for _ in 0..1500 {
            step(&mut emu, 1, 0);
            let r = &emu.game.ram;
            if r[0x0706] == dest_region && r[0x0736] == 0x05 {
                arrived = true;
                break;
            }
        }
        assert!(arrived, "{seed}: reached {}", dest.name());
        step(&mut emu, 200, 0);
        snap(&emu, &format!("{seed}-arrived"));
        let r = &emu.game.ram;
        let at = (usize::from(r[0x73]) - 30, usize::from(r[0x74]));
        let want = dest_t[slot].pos().unwrap();
        println!("{seed}: landed at {at:?}, connector at {want:?}");
        assert!(
            at.0.abs_diff(want.0) + at.1.abs_diff(want.1) <= 2,
            "{seed}: landed by the connector"
        );
        assert_eq!(emu.game.exec_errors, 0);
    }
    assert_eq!(tested, 2, "found two seeds to test");
}

/// The flute reveal on a generated East Hyrule: sail over, walk to the call
/// spot, play the flute, walk into the revealed location.
#[test]
#[ignore = "needs Z2_ROM"]
fn generated_flute_reveal_works() {
    let body = rom_body();
    let mut tested = 0;
    for k in 0..300 {
        if tested == 2 {
            break;
        }
        let seed = format!("flute{k}");
        let mut f = Flags::default();
        f.overworld.west_biome = Biome::Vanillalike;
        f.overworld.east_biome = Biome::Vanillalike;
        f.overworld.encounter_rate = EncounterRate::None;
        f.start.start_with_raft = true;
        f.start.start_with_flute = true;
        let out = z2_rando::randomize(&body, &seed, &f).unwrap();
        let rom = Rom::from_body(&out.body).unwrap();
        let (gw, west) = west_of(&out);
        let east = loc::read_table(&rom, Cont::East).unwrap();
        let ge = {
            let ptr = rom.read_cpu_word(2, 0x8508).unwrap();
            let off = rom.cpu_offset(2, ptr).unwrap();
            let len = (rom.prg().len() - off).min(map::BIG_BUDGET);
            map::decode(rom.read_slice(off, len).unwrap(), 75, Terrain::Water)
                .unwrap()
                .grid
        };
        let slot = usize::from(rom.read_cpu(7, 0xDF66).unwrap());
        let raw_y = rom.read_cpu(7, 0xDF68).unwrap() & 0x7F;
        let spot = (usize::from(raw_y) - 30, usize::from(east[slot].x));
        let call = (spot.0 - 2, spot.1);
        let wspots: Vec<(usize, usize)> = west
            .iter()
            .filter_map(|l| if l.raw_y == 0 { None } else { l.pos() })
            .collect();
        let espots: Vec<(usize, usize)> = east
            .iter()
            .filter_map(|l| if l.raw_y == 0 { None } else { l.pos() })
            .collect();
        let (mut emu, here) = on_overworld(&body, &seed, &f);
        let Some(p1) = path(&gw, &wspots, here, west[41].pos().unwrap()) else {
            continue;
        };
        let dock = east[41].pos().unwrap();
        // Landing tile: next to the dock on the land side.
        let Some(p2) = DIRS4
            .iter()
            .filter_map(|&(dr, dc)| {
                let s = ((dock.0 as i32 + dr) as usize, (dock.1 as i32 + dc) as usize);
                path(&ge, &espots, s, call)
            })
            .min_by_key(Vec::len)
        else {
            continue;
        };
        tested += 1;
        println!(
            "{seed}: raft {} tiles, then {} tiles to the call spot {call:?}",
            p1.len(),
            p2.len()
        );
        assert!(walk(&mut emu, &p1), "{seed}: walk to the raft");
        for _ in 0..1500 {
            step(&mut emu, 1, 0);
            if emu.game.ram[0x0706] == 2 && emu.game.ram[0x0736] == 0x05 {
                break;
            }
        }
        step(&mut emu, 120, 0);
        let r = &emu.game.ram;
        let at = (usize::from(r[0x73]) - 30, usize::from(r[0x74]));
        let p2 = path(&ge, &espots, at, call).expect("path from the landing");
        assert!(walk(&mut emu, &p2), "{seed}: walk to the call spot");
        assert_eq!(
            emu.game.wram[0x0A00 + slot],
            0,
            "{seed}: hidden before the flute"
        );
        step(&mut emu, 4, 0x02);
        step(&mut emu, 200, 0);
        snap(&emu, &format!("{seed}-revealed"));
        let row = emu.game.wram[0x0A00 + slot];
        assert_eq!(row & 0x7F, raw_y, "{seed}: the flute revealed the location");
        // Walk down into it.
        assert!(
            walk(&mut emu, &[call, (call.0 + 1, call.1), spot]),
            "{seed}: walk in"
        );
        step(&mut emu, 120, 0);
        let r = &emu.game.ram;
        assert_ne!(r[0x0736], 0x05, "{seed}: entered");
        assert_eq!(
            usize::from(r[0x0748]),
            slot - usize::from(east[slot].entrance),
            "{seed}: the revealed slot"
        );
        assert_eq!(emu.game.exec_errors, 0);
    }
    assert_eq!(tested, 2, "found two seeds to test");
}

const DIRS4: [(i32, i32); 4] = [(0, 1), (0, -1), (1, 0), (-1, 0)];

/// The hammer reveal on a generated East Hyrule.
#[test]
#[ignore = "needs Z2_ROM"]
fn generated_hammer_reveal_works() {
    let body = rom_body();
    let mut tested = 0;
    for k in 0..300 {
        if tested == 2 {
            break;
        }
        let seed = format!("hammer{k}");
        let mut f = Flags::default();
        f.overworld.west_biome = Biome::Vanillalike;
        f.overworld.east_biome = Biome::Vanillalike;
        f.overworld.encounter_rate = EncounterRate::None;
        f.start.start_with_raft = true;
        f.start.start_with_hammer = true;
        let out = z2_rando::randomize(&body, &seed, &f).unwrap();
        let rom = Rom::from_body(&out.body).unwrap();
        let (gw, west) = west_of(&out);
        let east = loc::read_table(&rom, Cont::East).unwrap();
        let ge = {
            let ptr = rom.read_cpu_word(2, 0x8508).unwrap();
            let off = rom.cpu_offset(2, ptr).unwrap();
            let len = (rom.prg().len() - off).min(map::BIG_BUDGET);
            map::decode(rom.read_slice(off, len).unwrap(), 75, Terrain::Water)
                .unwrap()
                .grid
        };
        let slot = usize::from(rom.read_cpu(7, 0xDF67).unwrap());
        let row = usize::from(rom.read_cpu(7, 0xDF9C).unwrap());
        let spot = (row, usize::from(east[slot].x));
        let wspots: Vec<(usize, usize)> = west
            .iter()
            .filter_map(|l| if l.raw_y == 0 { None } else { l.pos() })
            .collect();
        let espots: Vec<(usize, usize)> = east
            .iter()
            .filter_map(|l| if l.raw_y == 0 { None } else { l.pos() })
            .collect();
        let (mut emu, here) = on_overworld(&body, &seed, &f);
        let Some(p1) = path(&gw, &wspots, here, west[41].pos().unwrap()) else {
            continue;
        };
        let dock = east[41].pos().unwrap();
        // Approach so the last step faces the hidden tile.
        let mut best: Option<Vec<(usize, usize)>> = None;
        for (dr, dc) in DIRS4 {
            let n = (spot.0 as i32 - dr, spot.1 as i32 - dc);
            let n2 = (spot.0 as i32 - 2 * dr, spot.1 as i32 - 2 * dc);
            if n.0 < 0 || n.1 < 0 || n2.0 < 0 || n2.1 < 0 {
                continue;
            }
            let (n, n2) = ((n.0 as usize, n.1 as usize), (n2.0 as usize, n2.1 as usize));
            if !ge.get(n.0, n.1).is_open() || espots.contains(&n) {
                continue;
            }
            for (sr, sc) in DIRS4 {
                let s = ((dock.0 as i32 + sr) as usize, (dock.1 as i32 + sc) as usize);
                if let Some(mut p) = path(&ge, &espots, s, n2) {
                    p.push(n);
                    if best.as_ref().is_none_or(|b| p.len() < b.len()) {
                        best = Some(p);
                    }
                }
            }
        }
        let Some(_) = best else { continue };
        tested += 1;
        assert!(walk(&mut emu, &p1), "{seed}: walk to the raft");
        for _ in 0..1500 {
            step(&mut emu, 1, 0);
            if emu.game.ram[0x0706] == 2 && emu.game.ram[0x0736] == 0x05 {
                break;
            }
        }
        step(&mut emu, 120, 0);
        let r = &emu.game.ram;
        let at = (usize::from(r[0x73]) - 30, usize::from(r[0x74]));
        // Re-plan from where Link landed.
        let mut plan = None;
        for (dr, dc) in DIRS4 {
            let n = ((spot.0 as i32 - dr) as usize, (spot.1 as i32 - dc) as usize);
            let n2 = (
                (spot.0 as i32 - 2 * dr) as usize,
                (spot.1 as i32 - 2 * dc) as usize,
            );
            if !ge.get(n.0, n.1).is_open() || espots.contains(&n) {
                continue;
            }
            if let Some(mut p) = path(&ge, &espots, at, n2) {
                p.push(n);
                if plan
                    .as_ref()
                    .is_none_or(|b: &Vec<(usize, usize)>| p.len() < b.len())
                {
                    plan = Some(p);
                }
            }
        }
        let p2 = plan.expect("path to the hidden tile");
        println!("{seed}: {} tiles to the hidden tile {spot:?}", p2.len());
        assert!(walk(&mut emu, &p2), "{seed}: walk to the hidden tile");
        assert_eq!(
            emu.game.wram[0x0A00 + slot],
            0,
            "{seed}: hidden before the hammer"
        );
        snap(&emu, &format!("{seed}-before"));
        println!(
            "{seed}: facing {:02X} at ({}, {})",
            emu.game.ram[0x0562], emu.game.ram[0x73], emu.game.ram[0x74]
        );
        step(&mut emu, 4, 0x01);
        step(&mut emu, 120, 0);
        snap(&emu, &format!("{seed}-revealed"));
        assert_ne!(
            emu.game.wram[0x0A00 + slot],
            0,
            "{seed}: the hammer revealed it"
        );
        assert_eq!(emu.game.exec_errors, 0);
    }
    assert_eq!(tested, 2, "found two seeds to test");
}

/// Random encounter rates per region: on West Hyrule, a "None" roll means
/// no wandering enemies ever spawn while Link walks; "Normal" spawns some.
#[test]
#[ignore = "needs Z2_ROM"]
fn per_region_encounter_rates_in_game() {
    let body = rom_body();
    let mut seen = [false; 2];
    for k in 0..40 {
        let seed = format!("rate{k}");
        let mut f = Flags::default();
        f.overworld.west_biome = Biome::Vanillalike;
        f.overworld.encounter_rate = EncounterRate::Random;
        let out = z2_rando::randomize(&body, &seed, &f).unwrap();
        let Some(line) = out
            .spoiler
            .lines()
            .find(|l| l.starts_with("Encounter rate:"))
        else {
            continue;
        };
        let west_none = line.contains("West None");
        let west_normal = line.contains("West Normal");
        let mixed = !(line.contains("None,") && line.contains("None, East None"));
        if !(west_none || west_normal) || seen[usize::from(west_normal)] || !mixed {
            continue;
        }
        let (g, table) = west_of(&out);
        let (mut emu, here) = on_overworld(&body, &seed, &f);
        let spots: Vec<(usize, usize)> = table
            .iter()
            .filter_map(|l| if l.raw_y == 0 { None } else { l.pos() })
            .collect();
        // Pace between Link's tile and an open neighbour.
        let Some(next) = DIRS4.iter().find_map(|&(dr, dc)| {
            let n = ((here.0 as i32 + dr) as usize, (here.1 as i32 + dc) as usize);
            (g.get(n.0, n.1).is_open() && !spots.contains(&n)).then_some(n)
        }) else {
            continue;
        };
        let Some(next2) = DIRS4.iter().find_map(|&(dr, dc)| {
            let n = ((next.0 as i32 + dr) as usize, (next.1 as i32 + dc) as usize);
            (n != here && g.get(n.0, n.1).is_open() && !spots.contains(&n)).then_some(n)
        }) else {
            continue;
        };
        walk(&mut emu, &[here, next]);
        let mut spawned = 0;
        for i in 0..30 {
            let (a, b) = if i % 2 == 0 {
                (next, next2)
            } else {
                (next2, next)
            };
            walk(&mut emu, &[a, b]);
            for _ in 0..40 {
                step(&mut emu, 1, 0);
                if (0x82..0x8A).any(|a| emu.game.ram[a] != 0) {
                    spawned += 1;
                }
            }
            if emu.game.ram[0x0736] != 0x05 {
                println!(
                    "{seed}: left the overworld after {i} paces (mode {:02X})",
                    emu.game.ram[0x0736]
                );
                break;
            }
        }
        println!("{seed}: {line}: {spawned} frames with enemies");
        if west_none {
            assert_eq!(spawned, 0, "{seed}: no enemies with rate None");
        } else {
            assert!(spawned > 0, "{seed}: enemies with rate Normal");
        }
        seen[usize::from(west_normal)] = true;
        if seen == [true, true] {
            break;
        }
    }
    assert_eq!(seen, [true, true], "found both cases");
}
