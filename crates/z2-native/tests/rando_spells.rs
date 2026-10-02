//! ROM-gated gameplay checks for the randomizer's spell options and the
//! original 6502 features (`z2_rando::spells`, `z2_rando::asm_features`).
//!
//! Each test builds the randomized game through the same seam the app uses
//! (`emu_from_rom_body_with` + `RandoSpec`), starts a new file, waits for
//! side-view gameplay at the North Palace and then drives the game with
//! scripted input while reading RAM observables:
//!
//! * `$70` Link's horizontal speed (signed), `$3B:$4D` his X position;
//! * `$076F` the active-spell bits, `$0773` magic, `$0749` menu cursor,
//!   `$074A` last cast slot + 1, `$077B+` spells owned;
//! * `$070D` pending life refill.
//!
//! Run with `Z2_ROM=/path/to/zelda2.nes cargo test -p z2-native --test
//! rando_spells -- --ignored`.

use z2_native::app::{self, Emu, Features};
use z2_native::rando::RandoSpec;
use z2_rando::flags::{FireOption, Flags, Tri};

const A: u8 = 0x01;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;
const RIGHT: u8 = 0x80;
const LEFT: u8 = 0x40;

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

fn step(emu: &mut Emu, frames: usize, input: u8) {
    app::step_frames(emu, &vec![input; frames], None);
}

/// Title -> register a name -> load it -> settled in the first room.
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
    assert_eq!(emu.game.exec_errors, 0);
    assert_eq!(emu.game.ram[0x0736], 0x0B, "side-view gameplay");
}

fn x_pos(emu: &Emu) -> i32 {
    i32::from(emu.game.ram[0x3B]) * 256 + i32::from(emu.game.ram[0x4D])
}

/// Hold `input` for `frames` frames; returns every speed byte seen.
fn hold(emu: &mut Emu, frames: usize, input: u8, keep_bits: Option<u8>) -> Vec<u8> {
    let mut v = Vec::with_capacity(frames);
    for _ in 0..frames {
        if let Some(b) = keep_bits {
            emu.game.ram[0x076F] |= b;
        }
        step(emu, 1, input);
        v.push(emu.game.ram[0x70]);
    }
    v
}

fn max_right(v: &[u8]) -> u8 {
    v.iter().copied().filter(|&s| s < 0x80).max().unwrap_or(0)
}

#[test]
#[ignore = "needs Z2_ROM"]
fn dash_spell_doubles_running_speed_and_wears_off_cleanly() {
    let body = rom_body();
    let mut f = Flags::default();
    f.spells.fire_option = FireOption::ReplaceWithDash;
    let mut emu = build(&body, "dash", &f);
    start_game(&mut emu);

    // Walk left first so there is room to run right in the start room.
    hold(&mut emu, 40, LEFT, None);
    step(&mut emu, 30, 0);
    let normal = hold(&mut emu, 60, RIGHT, None);
    assert_eq!(max_right(&normal), 0x18, "vanilla cap without the spell");
    step(&mut emu, 40, 0);
    hold(&mut emu, 90, LEFT, None);
    step(&mut emu, 40, 0);

    // Fire bit set = Dash active.
    let x0 = x_pos(&emu);
    let dash = hold(&mut emu, 50, RIGHT, Some(0x10));
    assert_eq!(max_right(&dash), 0x30, "dash cap: {dash:02X?}");
    assert!(x_pos(&emu) > x0, "moved right");
    assert_eq!(emu.game.exec_errors, 0);

    // The spell wears off mid-run: speed drops back to the walking cap
    // (clamped), never creeping past it or wrapping negative.
    emu.game.ram[0x076F] &= !0x10;
    let after = hold(&mut emu, 30, RIGHT, None);
    assert!(
        after.iter().all(|&s| s <= 0x18),
        "speed after the spell: {after:02X?}"
    );
    assert_eq!(*after.last().unwrap(), 0x18);
    assert_eq!(emu.game.exec_errors, 0);
}

#[test]
#[ignore = "needs Z2_ROM"]
fn vanilla_fire_bit_does_not_change_speed() {
    let body = rom_body();
    let mut emu = build(&body, "x", &Flags::default());
    start_game(&mut emu);
    hold(&mut emu, 40, LEFT, None);
    step(&mut emu, 30, 0);
    let v = hold(&mut emu, 60, RIGHT, Some(0x10));
    assert_eq!(max_right(&v), 0x18);
}

#[test]
#[ignore = "needs Z2_ROM"]
fn dash_always_on_runs_fast_without_a_spell() {
    let body = rom_body();
    let mut f = Flags::default();
    f.spells.dash_always_on = true;
    let mut emu = build(&body, "always", &f);
    start_game(&mut emu);
    hold(&mut emu, 40, LEFT, None);
    step(&mut emu, 30, 0);
    let v = hold(&mut emu, 70, RIGHT, None);
    assert_eq!(max_right(&v), 0x30, "{v:02X?}");
    assert_eq!(emu.game.exec_errors, 0);
}

/// Own every spell, full magic, then cast menu slot `slot` with Select.
/// Returns the active-spell bits right after the cast frame.
fn cast_slot(emu: &mut Emu, slot: u8) -> u8 {
    cast_slot_cost(emu, slot).0
}

/// [`cast_slot`] plus the magic it used.
fn cast_slot_cost(emu: &mut Emu, slot: u8) -> (u8, u8) {
    for i in 0..8 {
        emu.game.ram[0x077B + i] = 1;
    }
    emu.game.ram[0x0783] = 8; // containers
    emu.game.ram[0x0773] = 0xFF; // magic (8 containers = $FF)
    emu.game.ram[0x076F] = 0;
    emu.game.ram[0x0774] = 0x10; // low life, so a Life cast shows
    emu.game.ram[0x0749] = slot;
    emu.game.ram[0x074A] = 0;
    step(emu, 1, SELECT);
    let bits = emu.game.ram[0x076F];
    let used = 0xFF - emu.game.ram[0x0773];
    step(emu, 2, 0);
    (bits, used)
}

/// After a Life cast at life `$10`, life climbs by the refill amount and
/// then stops (the handler ran once, not every frame).
fn assert_life_heals_once(emu: &mut Emu, what: &str) {
    step(emu, 120, 0);
    let hp = emu.game.ram[0x0774];
    let max = (emu.game.ram[0x0784] << 5).wrapping_sub(1);
    assert!(hp > 0x10, "{what}: life went up ({hp:02X})");
    assert!(
        hp < max,
        "{what}: life refilled once, not to full ({hp:02X})"
    );
    step(emu, 60, 0);
    assert_eq!(emu.game.ram[0x0774], hp, "{what}: no further healing");
}

#[test]
#[ignore = "needs Z2_ROM"]
fn shuffled_spell_menu_casts_the_spell_in_each_slot() {
    let body = rom_body();
    let mut f = Flags::default();
    f.spells.shuffle_spell_locations = Tri::On;
    for seed in ["menu-a", "menu-b", "menu-c"] {
        let out = z2_rando::randomize(&body, seed, &f).unwrap();
        // Recover the menu from the effect-bit table (one bit per spell).
        let off = 0x0DBB;
        let bits = &out.body[off..off + 8];
        let mut emu = build(&body, seed, &f);
        start_game(&mut emu);
        let level = usize::from(emu.game.ram[0x0778]).clamp(1, 8);
        for slot in 0..8u8 {
            let s = usize::from(slot);
            let want = bits[s];
            let cost = out.body[0x0D7B + s * 8 + level - 1];
            let (got, used) = cast_slot_cost(&mut emu, slot);
            assert_eq!(used, cost, "{seed} slot {slot}: magic used");
            match want {
                // Life, Spell and Thunder clear their own bit in the cast
                // frame; Life leaves a refill behind.
                0x04 => assert_life_heals_once(&mut emu, seed),
                0x40 | 0x80 => {}
                _ => assert_eq!(got & want, want, "{seed} slot {slot}: {got:02X}"),
            }
        }
        assert_eq!(emu.game.exec_errors, 0);
    }
}

#[test]
#[ignore = "needs Z2_ROM"]
fn linked_fire_casts_both_and_life_runs_once() {
    let body = rom_body();
    let mut f = Flags::default();
    f.spells.fire_option = FireOption::PairWithRandom;
    let mut seen_life = false;
    for seed in (0..40).map(|i| format!("link-{i}")) {
        let out = z2_rando::randomize(&body, &seed, &f).unwrap();
        let spoiler = out.spoiler.clone();
        let partner = spoiler
            .lines()
            .find_map(|l| l.trim().strip_prefix("Fire is linked with "))
            .expect("spoiler names the partner")
            .to_string();
        let names = [
            "Shield", "Jump", "Life", "Fairy", "Fire", "Reflect", "Spell", "Thunder",
        ];
        let p = names.iter().position(|n| *n == partner).unwrap() as u8;
        let pbit = 1u8 << p;
        if p == 2 {
            if seen_life {
                continue;
            }
            seen_life = true;
        } else if seed != "link-0" {
            continue;
        }
        let mut emu = build(&body, &seed, &f);
        start_game(&mut emu);
        // Cast Fire: Fire's bit and the partner's.
        let got = cast_slot(&mut emu, 4);
        assert_eq!(got & 0x10, 0x10, "{seed}: fire bit {got:02X}");
        if p == 2 {
            // Life heals once (its bit clears), Fire stays.
            assert_life_heals_once(&mut emu, &seed);
            assert_eq!(emu.game.ram[0x076F] & 0x14, 0x10, "life bit cleared");
        } else {
            assert_eq!(got & pbit, pbit, "{seed}: partner bit {got:02X}");
        }
        // Cast the partner: Fire comes with it.
        step(&mut emu, 10, 0);
        let got = cast_slot(&mut emu, p);
        assert_eq!(got & 0x10, 0x10, "{seed}: partner casts fire {got:02X}");
        assert_eq!(emu.game.exec_errors, 0);
    }
    assert!(seen_life, "no seed linked Fire with Life");
}

const B: u8 = 0x02;

/// Launch velocity (`$057D`) of the first jump frame.
fn jump_launch(emu: &mut Emu) -> u8 {
    step(emu, 30, 0);
    step(emu, 1, A);
    let v = emu.game.ram[0x057D];
    step(emu, 60, 0);
    v
}

#[test]
#[ignore = "needs Z2_ROM"]
fn jump_always_on_launches_with_the_spell_velocity() {
    let body = rom_body();
    let mut vanilla = build(&body, "j", &Flags::default());
    start_game(&mut vanilla);
    let plain = jump_launch(&mut vanilla);
    vanilla.game.ram[0x076F] |= 0x02; // Jump spell active
    step(&mut vanilla, 1, 0);
    let spell = jump_launch(&mut vanilla);
    assert_ne!(plain, spell, "the Jump spell changes the launch");

    let mut f = Flags::default();
    f.spells.jump_always_on = true;
    let mut emu = build(&body, "j", &f);
    start_game(&mut emu);
    assert_eq!(jump_launch(&mut emu), spell, "always-on jump = spell jump");
}

/// Press B at low life; whether a sword beam (Link projectile slot `$8D`)
/// came out.
fn beam_fired(emu: &mut Emu) -> bool {
    emu.game.ram[0x0774] = 0x10;
    step(emu, 20, 0);
    let mut fired = false;
    for _ in 0..12 {
        step(emu, 1, B);
        fired |= emu.game.ram[0x8D] != 0 || emu.game.ram[0x8E] != 0;
    }
    step(emu, 30, 0);
    fired
}

#[test]
#[ignore = "needs Z2_ROM"]
fn permanent_beam_fires_at_low_health() {
    let body = rom_body();
    let mut vanilla = build(&body, "b", &Flags::default());
    start_game(&mut vanilla);
    assert!(
        !beam_fired(&mut vanilla),
        "vanilla: no beam below full life"
    );

    let mut f = Flags::default();
    f.spells.permanent_beam_sword = true;
    let mut emu = build(&body, "b", &f);
    start_game(&mut emu);
    assert!(beam_fired(&mut emu), "permanent beam at low life");
}

#[test]
#[ignore = "needs Z2_ROM"]
fn life_refill_matches_the_spoiler() {
    let body = rom_body();
    let mut f = Flags::default();
    f.spells.shuffle_life_refill = true;
    for seed in ["r1", "r2", "r3"] {
        let out = z2_rando::randomize(&body, seed, &f).unwrap();
        let bars: u8 = out
            .spoiler
            .lines()
            .find_map(|l| l.strip_prefix("Life spell refill: "))
            .and_then(|r| r.split(' ').next())
            .and_then(|n| n.parse().ok())
            .expect("spoiler line");
        assert!((1..=5).contains(&bars));
        let mut emu = build(&body, seed, &f);
        start_game(&mut emu);
        emu.game.ram[0x0784] = 8; // room to heal
        cast_slot(&mut emu, 2);
        // The refill fills about one life unit per frame.
        step(&mut emu, 400, 0);
        // One bar is one heart container: 32 life units (the refill
        // counter is in half-bar steps of 16, drained two units a step).
        let gained = emu.game.ram[0x0774] - 0x10;
        assert_eq!(gained, bars * 32, "{seed}: healed {gained:#X}");
    }
}

/// Opens the pause menu with every spell owned and (when `RANDO_PNG_DIR`
/// is set) writes a screenshot per seed for a visual check of the names.
#[test]
#[ignore = "needs Z2_ROM"]
fn pause_menu_shows_shuffled_names() {
    let body = rom_body();
    let mut f = Flags::default();
    f.spells.shuffle_spell_locations = Tri::On;
    f.spells.fire_option = FireOption::ReplaceWithDash;
    for seed in ["menu-a", "menu-b"] {
        let mut emu = build(&body, seed, &f);
        start_game(&mut emu);
        for i in 0..8 {
            emu.game.ram[0x077B + i] = 1;
        }
        step(&mut emu, 5, START);
        step(&mut emu, 60, 0);
        assert_eq!(emu.game.exec_errors, 0);
        if let Ok(dir) = std::env::var("RANDO_PNG_DIR") {
            let png = z2_ppu::encode_indexed_png(emu.game.frame_indexed());
            std::fs::write(format!("{dir}/pause-{seed}.png"), png).unwrap();
        }
        step(&mut emu, 5, START);
        step(&mut emu, 60, 0);
        assert_eq!(emu.game.ram[0x0736], 0x0B);
    }
}

const MOVIE: &str = "/Volumes/HolyDrive/dev/z2-corpus/movies/anypct.bk2";

fn movie_track() -> Option<Vec<u8>> {
    let p = std::env::var("Z2_MOVIE").unwrap_or_else(|_| MOVIE.to_string());
    app::load_movie_track(std::path::Path::new(&p)).ok()
}

/// Frames at which Link's injured timer (`$050C`) starts, with the speeds
/// right after the hit.
fn hits(emu: &mut Emu, track: &[u8], limit: usize, max_hits: usize) -> Vec<(usize, u8, u8)> {
    let mut out = Vec::new();
    let mut prev = 0u8;
    for (f, &input) in track.iter().enumerate().take(limit) {
        step(emu, 1, input);
        let t = emu.game.ram[0x050C];
        if t > prev && t >= 0x1E && emu.game.ram[0x0736] == 0x0B {
            out.push((f, emu.game.ram[0x70], emu.game.ram[0x057D]));
            if out.len() >= max_hits {
                break;
            }
        }
        prev = t;
    }
    out
}

#[test]
#[ignore = "needs Z2_ROM and the any% movie (Z2_MOVIE)"]
fn chaotic_knockback_changes_the_first_real_hit() {
    let Some(track) = movie_track() else {
        eprintln!("skipping: no movie");
        return;
    };
    let body = rom_body();
    let mut vanilla = build(&body, "kb", &Flags::default());
    let v = hits(&mut vanilla, &track, track.len(), 1);
    let Some(&(frame, vx, vy)) = v.first() else {
        panic!("the movie never gets Link hurt");
    };
    eprintln!("vanilla first hit at frame {frame}: vx {vx:02X} vy {vy:02X}");

    let mut f = Flags::default();
    f.enemies.randomize_knockback = true;
    let mut seen_change = false;
    for seed in ["kb-1", "kb-2", "kb-3", "kb-4"] {
        let mut emu = build(&body, seed, &f);
        assert!(
            !emu.rom.untrapped.is_empty(),
            "the hurt routine runs as ROM code"
        );
        let h = hits(&mut emu, &track, frame + 2, 1);
        let &(f2, x, y) = h.first().expect("same first hit");
        assert_eq!(f2, frame, "{seed}: identical play up to the first hit");
        eprintln!("{seed}: vx {x:02X} vy {y:02X}");
        let mag = if x >= 0x80 { x.wrapping_neg() } else { x };
        assert!((1..0x40).contains(&mag) || x == 0, "{seed}: vx {x:02X}");
        assert!((0xF9..=0xFF).contains(&y), "{seed}: vy {y:02X}");
        seen_change |= x != vx || y != vy;
        // Keep playing (the movie desyncs from here); the game must not
        // wedge and Link's speed must never run away.
        for &input in track.iter().skip(frame + 2).take(1500) {
            step(&mut emu, 1, input);
            let s = emu.game.ram[0x70];
            assert!(s <= 0x40 || s >= 0xC0, "{seed}: runaway speed {s:02X}");
        }
        assert_eq!(emu.game.exec_errors, 0, "{seed}");
    }
    assert!(seen_change, "knockback differs from vanilla for some seed");
}

#[test]
#[ignore = "needs Z2_ROM"]
fn knockback_trampoline_returns_to_every_bank() {
    let body = rom_body();
    let mut f = Flags::default();
    f.enemies.randomize_knockback = true;
    let mut emu = build(&body, "kb-t", &f);
    start_game(&mut emu);
    let area = emu.game.ram[0x0769];
    for bank in 0..7u8 {
        for (id, y) in [(0x03u8, 1u8), (0x10, 2), (0x07, 0)] {
            emu.game.cpu.a = bank;
            emu.game.try_call_asm(0xFFCC, 1000).unwrap();
            assert_eq!(emu.game.mmc1.prg & 0x0F, bank);
            emu.game.ram[0xA1] = id;
            emu.game.ram[0x70] = 0;
            emu.game.cpu.x = 0;
            emu.game.cpu.y = y;
            emu.game.try_call_asm(0xBFEC, 1000).unwrap();
            assert_eq!(emu.game.mmc1.prg & 0x0F, bank, "back in bank {bank}");
            let vx = emu.game.ram[0x70];
            match y {
                0 => {
                    assert!((0xF9..=0xFF).contains(&emu.game.ram[0x057D]));
                    assert_eq!(emu.game.cpu.y, emu.game.ram[0x070F]);
                }
                1 => assert!((1..0x40).contains(&vx), "right push {vx:02X}"),
                _ => assert!(vx >= 0xC1, "left push {vx:02X}"),
            }
        }
    }
    emu.game.cpu.a = area;
    emu.game.try_call_asm(0xFFCC, 1000).unwrap();
    // (The synthetic pushes may carry Link out of the room; only check
    // that the game keeps running.)
    step(&mut emu, 120, 0);
    assert_eq!(emu.game.exec_errors, 0);
}
