//! ROM-gated: generated palaces load and connect in the running game.
//!
//! For a few palace styles and seeds, every room of every palace is loaded
//! through the game's own loader (scene registers, then game mode 0): the
//! load must settle into side-view play without an interpreter fault, and
//! the connection table the game copied into WRAM must be the one the
//! randomizer wrote. Then Link is put at a room edge and walks out; the
//! game must take him to the room the layout says is next door.
//!
//! Run with `Z2_ROM=/path/to/zelda2.nes cargo test -p z2-native --release
//! --test rando_palaces -- --ignored`.

use std::collections::BTreeSet;

use z2_native::app::{self, Emu, Features};
use z2_native::rando::RandoSpec;
use z2_rando::flags::{BossRoomsExit, Flags, ItemRoomCount, PalaceLength, PalaceStyle};
use z2_rando::palaces::rooms::{entrance_map, Group, GroupData, MAPS};
use z2_rando::rom::Rom;

const A: u8 = 0x01;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;
const LEFT: u8 = 0x40;
const RIGHT: u8 = 0x80;
const MODE: usize = 0x0736;
const MODE_SIDEVIEW: u8 = 0x0B;
const SCENE: usize = 0x0561;

fn rando_emu(seed: &str, flags: Flags) -> (Emu, z2_rando::Output) {
    let body = z2_assets::rom::open().expect("Z2_ROM must name the verified ROM");
    let spec = RandoSpec {
        seed: seed.to_string(),
        flags,
        spoiler_path: None,
        sprite_ips: None,
    };
    let out = spec.run(&body).expect("randomize");
    let feats = Features {
        rando: Some(spec.leak()),
        ..Features::default()
    };
    let emu = app::emu_from_rom_body_with(&body, 44_100, feats).expect("build rando emu");
    assert_eq!(emu.rom.body_crc32, out.crc32);
    (emu, out)
}

fn step(emu: &mut Emu, frames: usize, pad: u8) {
    app::step_frames(emu, &vec![pad; frames], None);
}

/// Title -> file select -> name -> load -> side-view play.
fn reach_gameplay(emu: &mut Emu) {
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
    assert_eq!(emu.game.exec_errors, 0, "menu/load path faulted");
    assert_eq!(emu.game.ram[MODE], MODE_SIDEVIEW, "reached side-view play");
}

/// Keep Link alive, give him the magic key (locked doors open) and the
/// fairy-proof basics so walking through a room is not stopped by damage.
fn buff_link(emu: &mut Emu) {
    let ram = &mut emu.game.ram;
    ram[0x0774] = 0xFF;
    ram[0x0773] = 0xFF;
    ram[0x0784] = 8;
    ram[0x0783] = 8;
    ram[0x0785] = 1; // candle
    ram[0x078C] = 1; // magic key
}

/// `(region, world, key area)` of palace `p`'s overworld spot.
fn palace_scene(p: u8) -> (u8, u8, u8) {
    match p {
        1 => (0, 3, 0x34),
        2 => (0, 3, 0x35),
        3 => (0, 4, 0x36),
        4 => (1, 4, 0x34),
        5 => (2, 3, 0x34),
        6 => (2, 4, 0x35),
        _ => (2, 5, 0x36),
    }
}

/// Load room `map` of palace `p`, entering on `page` facing right
/// (`from_top`: arriving by elevator from above or by falling).
fn load_room(emu: &mut Emu, p: u8, map: u8, page: u8, from_top: bool) {
    let (region, world, key) = palace_scene(p);
    let ram = &mut emu.game.ram;
    ram[0x0706] = region;
    ram[0x0707] = world;
    ram[0x0748] = key;
    ram[0x056C] = key.wrapping_sub(0x34);
    ram[SCENE] = map;
    ram[0x075C] = page;
    // The game sets the facing from the page bits the same way.
    ram[0x0701] = page & 1;
    ram[0x0709] = 0;
    ram[0x0704] = u8::from(from_top);
    ram[MODE] = 0;
    step(emu, 150, 0);
}

/// How the game would bring Link into `m`: the page and whether from the
/// top, read from the first connection byte that leads there.
fn entry(g: &GroupData, maps: &[u8], m: u8) -> (u8, bool) {
    for &src in maps {
        for (i, &b) in g.conn[usize::from(src)].iter().enumerate() {
            if target(b) == Some(m) && src != m {
                let page = b & 3;
                return match i {
                    3 => (page, false),
                    0 => (page, false),
                    1 => (page, true),
                    _ => (page, i == 2 && is_drop(g, src)),
                };
            }
        }
    }
    (0, false)
}

/// Whether `src`'s byte 2 is a hole rather than an elevator (the target
/// does not point back down).
fn is_drop(g: &GroupData, src: u8) -> bool {
    let b = g.conn[usize::from(src)][2];
    target(b).is_none_or(|t| target(g.conn[usize::from(t)][1]) != Some(src))
}

/// A live connection byte's target map.
fn target(b: u8) -> Option<u8> {
    (b < 0xFC && b != 0).then_some(b >> 2)
}

/// Rooms of palace `p` reachable from its entrance over the written
/// connection bytes.
fn palace_maps(g: &GroupData, p: u8) -> Vec<u8> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![entrance_map(p)];
    while let Some(m) = stack.pop() {
        if usize::from(m) >= MAPS || !seen.insert(m) {
            continue;
        }
        for &b in &g.conn[usize::from(m)] {
            if let Some(t) = target(b) {
                stack.push(t);
            }
        }
    }
    seen.into_iter().collect()
}

/// With `Z2_RANDO_SHOTS=DIR` (outside the work tree: frames come from the
/// ROM), save a PNG of the screen for eyeballing.
fn shot(emu: &Emu, name: &str) {
    if let Some(dir) = std::env::var_os("Z2_RANDO_SHOTS") {
        let png = z2_ppu::encode_indexed_png(emu.game.frame_indexed());
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        std::fs::write(path, png).expect("write shot");
    }
}

struct Tally {
    rooms: usize,
    exits_tried: usize,
    exits_ok: usize,
}

fn tour(seed: &str, flags: Flags) -> Tally {
    let (mut emu, out) = rando_emu(seed, flags);
    let rom = Rom::from_body(&out.body).unwrap();
    reach_gameplay(&mut emu);
    let mut t = Tally {
        rooms: 0,
        exits_tried: 0,
        exits_ok: 0,
    };
    for p in 1..=7u8 {
        let group = Group::of_palace(p);
        let g = GroupData::read(&rom, group).unwrap();
        let maps = palace_maps(&g, p);
        assert!(
            maps.len() >= 8,
            "{seed}: palace {p} has {} rooms",
            maps.len()
        );
        for &m in &maps {
            buff_link(&mut emu);
            let (page, top) = entry(&g, &maps, m);
            load_room(&mut emu, p, m, page, top);
            let what = format!("{seed}: palace {p} map {m}");
            // A room entered by falling plays the fall first (and may fall
            // on through its own hole).
            for _ in 0..30 {
                if emu.game.ram[MODE] == MODE_SIDEVIEW {
                    break;
                }
                step(&mut emu, 20, 0);
            }
            assert_eq!(emu.game.exec_errors, 0, "{what}: load faulted");
            assert_eq!(emu.game.ram[MODE], MODE_SIDEVIEW, "{what}: settled");
            if !top {
                assert_eq!(emu.game.ram[SCENE], m, "{what}: scene");
            }
            let w = 0x0AFC + 4 * usize::from(m);
            assert_eq!(
                &emu.game.wram[w..w + 4],
                &g.conn[usize::from(m)][..],
                "{what}: WRAM connection bytes"
            );
            t.rooms += 1;
            shot(&emu, &format!("{seed}-p{p}-m{m}"));
            // Walk out of the right edge (enter on the last page) and out
            // of the left edge (enter on the first page).
            let c = g.conn[usize::from(m)];
            for (byte, page, pad) in [(3usize, 3u8, RIGHT), (0, 0, LEFT)] {
                let Some(next) = target(c[byte]) else {
                    continue;
                };
                buff_link(&mut emu);
                load_room(&mut emu, p, m, page, false);
                t.exits_tried += 1;
                for _ in 0..240 {
                    buff_link(&mut emu);
                    step(&mut emu, 1, pad);
                    if emu.game.ram[SCENE] != m {
                        break;
                    }
                }
                step(&mut emu, 120, 0);
                assert_eq!(emu.game.exec_errors, 0, "{what}: walking out faulted");
                if emu.game.ram[SCENE] == next {
                    t.exits_ok += 1;
                } else if emu.game.ram[SCENE] != m {
                    panic!(
                        "{what}: walked {} into map {} but the layout says {next}",
                        if pad == RIGHT { "right" } else { "left" },
                        emu.game.ram[SCENE]
                    );
                }
            }
        }
    }
    assert_eq!(emu.game.exec_errors, 0);
    t
}

fn tall_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(128 * 1024 * 1024)
        .spawn(body)
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

fn flags(style: PalaceStyle, gp: PalaceStyle) -> Flags {
    let mut f = Flags::default();
    f.palaces.normal_style = style;
    f.palaces.gp_style = gp;
    f.palaces.normal_length = PalaceLength::Random;
    f.palaces.item_rooms_per_palace = ItemRoomCount::RandomIncludeZero;
    f.palaces.boss_rooms_exit = BossRoomsExit::RandomPerPalace;
    f
}

fn check(seed: &'static str, f: Flags) {
    tall_stack(move || {
        let t = tour(seed, f);
        eprintln!(
            "{seed}: {} rooms loaded, {}/{} edge exits walked to the right room",
            t.rooms, t.exits_ok, t.exits_tried
        );
        assert!(t.rooms > 60);
        // Locked doors, statues and enemies can stop a walk; most must
        // still arrive, and none may arrive in the wrong room.
        assert!(
            t.exits_ok * 3 >= t.exits_tried * 2,
            "{seed}: too few exits worked"
        );
    });
}

#[test]
#[ignore = "needs Z2_ROM"]
fn reconstructed_palaces_connect_in_game() {
    check(
        "palaces-recon",
        flags(PalaceStyle::Reconstructed, PalaceStyle::Reconstructed),
    );
}

#[test]
#[ignore = "needs Z2_ROM"]
fn grid_palaces_connect_in_game() {
    check(
        "palaces-grid",
        flags(PalaceStyle::RandomPerPalace, PalaceStyle::Tower),
    );
}

#[test]
#[ignore = "needs Z2_ROM"]
fn shuffled_palaces_connect_in_game() {
    check(
        "palaces-shuffle",
        flags(PalaceStyle::Shuffled, PalaceStyle::Shuffled),
    );
}

#[test]
#[ignore = "needs Z2_ROM"]
fn vanilla_palaces_tour_baseline() {
    check("palaces-vanilla", Flags::default());
}
