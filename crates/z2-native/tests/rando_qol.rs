//! ROM-gated: the randomizer's quality-of-life and cosmetic patches do what
//! they say in the running game (hybrid interpreter + ports, exactly as the
//! app builds it).
//!
//! Each test builds a vanilla emulator and a randomized one with a single
//! option on, brings both to the same moment (scripted menu flow, or a
//! corpus movie prefix whose state is copied across) and compares what the
//! game does next: text-delay counter, backdrop colour, beep timer, game
//! mode, magic meter, palette RAM, OAM and APU output.
//!
//! Run with `Z2_ROM=/path/to/zelda2.nes cargo test --release -p z2-native
//! --test rando_qol -- --ignored`. The movie-based tests also need the
//! corpus (`$Z2_CORPUS`, default `/Volumes/HolyDrive/dev/z2-corpus`) and
//! skip without it.

use std::path::PathBuf;

use z2_native::app::{self, Emu, Features};
use z2_native::rando::RandoSpec;
use z2_rando::flags::{BeamSprite, BeepFrequency, BeepThreshold, Flags, NesColor};

const A: u8 = 0x01;
const B: u8 = 0x02;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;
const UP: u8 = 0x10;
const DOWN: u8 = 0x20;

fn rom_body() -> Vec<u8> {
    z2_assets::rom::open().expect("Z2_ROM must name the verified ROM")
}

fn movie(name: &str) -> Option<Vec<u8>> {
    let dir = std::env::var("Z2_CORPUS")
        .unwrap_or_else(|_| "/Volumes/HolyDrive/dev/z2-corpus".to_string());
    let path = PathBuf::from(dir).join("movies").join(name);
    if !path.is_file() {
        eprintln!("SKIP: corpus movie {} not found", path.display());
        return None;
    }
    Some(app::load_movie_track(&path).expect("parse movie"))
}

fn emu_with(body: &[u8], flags: Flags, sprite_ips: Option<Vec<u8>>) -> Emu {
    let spec = RandoSpec {
        seed: "qol-test".into(),
        flags,
        spoiler_path: None,
        sprite_ips,
    }
    .leak();
    let feats = Features {
        rando: Some(spec),
        ..Features::default()
    };
    app::emu_from_rom_body_with(body, 44_100, feats).expect("build randomized emu")
}

fn vanilla(body: &[u8]) -> Emu {
    app::emu_from_rom_body(body, 44_100).expect("build vanilla emu")
}

fn run(emu: &mut Emu, frames: usize, pad: u8) {
    app::step_frames(emu, &vec![pad; frames], None);
}

/// Fresh cartridge -> named save slot -> side-view gameplay in the North
/// Castle (the flow from `menu_flow_tests` / `player_magic_rom`).
fn to_gameplay(emu: &mut Emu) {
    run(emu, 30, 0);
    run(emu, 5, START);
    run(emu, 20, 0);
    run(emu, 5, START);
    run(emu, 20, 0);
    for _ in 0..8 {
        run(emu, 2, A);
        run(emu, 8, 0);
    }
    for _ in 0..3 {
        run(emu, 2, SELECT);
        run(emu, 8, 0);
    }
    run(emu, 5, START);
    run(emu, 30, 0);
    run(emu, 5, START);
    run(emu, 60, 0);
    run(emu, 1100, 0);
    assert_eq!(emu.game.ram[0x0736], 0x0B, "side-view gameplay mode");
    assert_eq!(emu.game.exec_errors, 0);
}

/// A vanilla emulator in gameplay, and a randomized one with `flags` whose
/// CPU/PPU state is a copy of it.
fn pair_in_gameplay(body: &[u8], flags: Flags) -> (Emu, Emu) {
    let mut v = vanilla(body);
    to_gameplay(&mut v);
    let mut r = emu_with(body, flags, None);
    r.game.load_state(&v.game.save_state());
    (v, r)
}

fn flags(edit: impl FnOnce(&mut Flags)) -> Flags {
    let mut f = Flags::default();
    edit(&mut f);
    f
}

// ---------------------------------------------------------------------------
// QoL
// ---------------------------------------------------------------------------

/// Fast text: `warp-glitch` talks to a townsperson at frame 951 (the
/// text-delay counter `$0566` is loaded with the box-opening wait `$2A`).
/// From a state 20 frames before that, with the same input, the vanilla
/// printer spends most of the next 300 frames waiting on `$0566` (the box
/// wait, then 5 frames per letter and 11 per line); the patched printer
/// never waits and puts out a letter every frame.
#[test]
#[ignore = "needs Z2_ROM and the corpus"]
fn fast_text_prints_without_waiting() {
    let Some(track) = movie("warp-glitch.bk2") else {
        return;
    };
    let body = rom_body();
    let mut v = vanilla(&body);
    let opened = (0..2000)
        .find(|&f| {
            app::step_frames(&mut v, &[track[f]], None);
            v.game.ram[0x0566] == 0x2A
        })
        .expect("warp-glitch opens a dialog in its first 2000 frames");
    let start = opened - 20;
    let mut v = vanilla(&body);
    app::step_frames(&mut v, &track[..start], None);
    let mut r = emu_with(&body, flags(|f| f.qol.fast_text = true), None);
    assert!(r.rom.untrapped.is_empty(), "bank 3 operands only");
    r.game.load_state(&v.game.save_state());
    let waits = |e: &mut Emu| {
        let mut n = 0;
        let pads = track[start..=opened]
            .iter()
            .copied()
            .chain(std::iter::repeat(0));
        for pad in pads.take(300) {
            app::step_frames(e, &[pad], None);
            if e.game.ram[0x0566] != 0 {
                n += 1;
            }
        }
        assert_ne!(e.game.ram[0x0524], 0, "still in the dialog");
        n
    };
    let vw = waits(&mut v);
    let rw = waits(&mut r);
    eprintln!("dialog at frame {opened}: vanilla waits {vw} of 300 frames, fast text {rw}");
    assert!(vw > 200, "vanilla typewriter ({vw})");
    assert_eq!(rw, 0, "fast text never waits");
}

/// Remove flashing: the `warp-glitch` death (lives 3 -> 2 at frame 4528)
/// cycles the backdrop through the flash colours in vanilla; with the
/// option the backdrop changes at most twice (into and out of the steady
/// colour).
#[test]
#[ignore = "needs Z2_ROM and the corpus"]
fn remove_flashing_keeps_the_death_backdrop_steady() {
    let Some(track) = movie("warp-glitch.bk2") else {
        return;
    };
    let body = rom_body();
    let mut v = vanilla(&body);
    app::step_frames(&mut v, &track[..4200], None);
    let mut r = emu_with(&body, flags(|f| f.qol.remove_flashing = true), None);
    assert!(r.rom.untrapped.is_empty(), "{:X?}", r.rom.untrapped);
    r.game.load_state(&v.game.save_state());
    let backdrop_changes = |e: &mut Emu| {
        let mut changes = 0;
        let mut seen = std::collections::BTreeSet::new();
        let mut last = e.game.palette[0];
        let lives0 = e.game.ram[0x0700];
        for &pad in &track[4200..4560] {
            app::step_frames(e, &[pad], None);
            let c = e.game.palette[0];
            if c != last {
                changes += 1;
                seen.insert(c);
            }
            last = c;
        }
        assert_eq!(e.game.ram[0x0700] + 1, lives0, "Link died in this window");
        (changes, seen)
    };
    let (vc, vs) = backdrop_changes(&mut v);
    let (rc, rs) = backdrop_changes(&mut r);
    eprintln!("backdrop changes: vanilla {vc} {vs:02X?}, no-flash {rc} {rs:02X?}");
    assert!(vc >= 8, "vanilla flashes ({vc})");
    assert!(rc <= 3, "steady backdrop ({rc})");
}

/// Count beep starts (the beep timer `$07F9` reloading) over `frames`.
fn beeps(e: &mut Emu, frames: usize) -> usize {
    let mut n = 0;
    let mut last = e.game.ram[0x07F9];
    for _ in 0..frames {
        run(e, 1, 0);
        let t = e.game.ram[0x07F9];
        if t > last {
            n += 1;
        }
        last = t;
    }
    n
}

/// Low-health beep: from gameplay with life poked to `$1F` (just under the
/// vanilla threshold `$20`) or `$3F`, count beeps over 600 frames for each
/// threshold and speed.
#[test]
#[ignore = "needs Z2_ROM"]
fn beep_threshold_and_speed() {
    let body = rom_body();
    let mut base = vanilla(&body);
    to_gameplay(&mut base);
    let state = base.game.save_state();
    let count = |f: Flags, hp: u8| {
        let mut e = if f == Flags::default() {
            vanilla(&body)
        } else {
            emu_with(&body, f, None)
        };
        assert!(e.rom.untrapped.is_empty(), "{:X?}", e.rom.untrapped);
        e.game.load_state(&state);
        // Life and the HUD's copy of it (the beep reads the copy, which
        // the status bar only refreshes when life changes).
        e.game.ram[0x0774] = hp;
        e.game.ram[0x0565] = hp;
        run(&mut e, 2, 0);
        let bar = e.game.ram[0x0565];
        (beeps(&mut e, 600), bar)
    };
    let (normal, bar) = count(Flags::default(), 0x1F);
    eprintln!("life bar ${bar:02X}: vanilla {normal} beeps");
    assert!(bar < 0x20, "the poke reached the life bar (${bar:02X})");
    assert!(normal >= 8, "vanilla beeps ({normal})");
    let (half, _) = count(
        flags(|f| f.qol.beep_frequency = BeepFrequency::HalfSpeed),
        0x1F,
    );
    let (quarter, _) = count(
        flags(|f| f.qol.beep_frequency = BeepFrequency::QuarterSpeed),
        0x1F,
    );
    let (off, _) = count(flags(|f| f.qol.beep_frequency = BeepFrequency::Off), 0x1F);
    let (low_threshold, _) = count(
        flags(|f| f.qol.beep_threshold = BeepThreshold::QuarterBar),
        0x1F,
    );
    let (high_threshold, _) = count(
        flags(|f| f.qol.beep_threshold = BeepThreshold::TwoBars),
        0x3F,
    );
    let (vanilla_at_3f, _) = count(Flags::default(), 0x3F);
    eprintln!(
        "half {half}, quarter {quarter}, off {off}, quarter-bar {low_threshold}, \
         two bars at $3F {high_threshold} (vanilla {vanilla_at_3f})"
    );
    assert!(
        half * 2 <= normal + 2 && half >= normal / 2 - 2,
        "half speed"
    );
    assert!(quarter * 4 <= normal + 4 && quarter >= 1, "quarter speed");
    assert_eq!(off, 0, "beep off");
    assert_eq!(low_threshold, 0, "$1F is above a quarter bar");
    assert_eq!(vanilla_at_3f, 0);
    assert!(high_threshold >= 8, "two bars beeps at $3F");
}

/// Up+Select on controller 1 opens the save prompt from the pause pane;
/// vanilla ignores it there. Controller 2's Up+A keeps working.
#[test]
#[ignore = "needs Z2_ROM"]
fn up_select_on_controller_1_saves() {
    let body = rom_body();
    let (mut v, mut r) = pair_in_gameplay(&body, flags(|f| f.qol.up_a_on_controller_1 = true));
    // Pause, then hold the buttons; the save prompt takes the game out of
    // side-view mode `$0B`.
    let try_combo = |e: &mut Emu, p1: u8, p2: u8| {
        let s = e.game.save_state();
        run(e, 1, START);
        let mut hit = false;
        for _ in 0..60 {
            app::step_frames2(e, &[(p1, p2)], None);
            hit |= e.game.ram[0x0736] != 0x0B;
        }
        e.game.load_state(&s);
        hit
    };
    assert!(
        !try_combo(&mut v, UP | SELECT, 0),
        "vanilla: pad 1 does nothing"
    );
    assert!(try_combo(&mut v, 0, UP | A), "vanilla: pad 2 Up+A");
    assert!(
        try_combo(&mut r, UP | SELECT, 0),
        "patched: pad 1 Up+Select"
    );
    assert!(
        try_combo(&mut r, 0, UP | A),
        "patched: pad 2 Up+A still works"
    );
    assert!(
        !try_combo(&mut r, UP | A, 0),
        "patched: pad 1 Up+A is not the combo"
    );
}

/// Learn every spell, full meter, select Shield in the pause pane.
fn arm_shield(e: &mut Emu) {
    for a in 0x077B..0x0783 {
        e.game.ram[a] = 1;
    }
    e.game.ram[0x0778] = 1;
    e.game.ram[0x0783] = 8;
    e.game.ram[0x0773] = 0xFF;
    run(e, 1, START);
    run(e, 25, 0);
    for _ in 0..16 {
        if e.game.ram[0x0749] == 0 {
            break;
        }
        run(e, 1, DOWN);
        run(e, 1, 0);
    }
    assert_eq!(e.game.ram[0x0749], 0, "Shield selected");
    run(e, 1, START);
    run(e, 25, 0);
    assert_eq!(e.game.ram[0x0524], 0, "pause pane closed");
}

/// Fast spell casting: after one cast, vanilla blocks a second Select until
/// the pause pane is reopened (`$074A` latch); the patched game casts again.
#[test]
#[ignore = "needs Z2_ROM"]
fn fast_spell_casting_recasts_with_select() {
    let body = rom_body();
    let (mut v, mut r) = pair_in_gameplay(&body, flags(|f| f.qol.fast_spell_casting = true));
    let two_casts = |e: &mut Emu| {
        arm_shield(e);
        let m0 = e.game.ram[0x0773];
        run(e, 1, SELECT);
        run(e, 20, 0);
        let m1 = e.game.ram[0x0773];
        e.game.ram[0x076F] = 0; // spell effect over
        run(e, 1, SELECT);
        run(e, 20, 0);
        (m0, m1, e.game.ram[0x0773])
    };
    let (v0, v1, v2) = two_casts(&mut v);
    let (r0, r1, r2) = two_casts(&mut r);
    eprintln!("meter vanilla {v0:02X}->{v1:02X}->{v2:02X}, fast {r0:02X}->{r1:02X}->{r2:02X}");
    assert!(v1 < v0 && v2 == v1, "vanilla: one cast, then latched");
    assert!(r1 < r0 && r2 < r1, "fast casting: second cast paid");
    assert_eq!(v0 - v1, r1 - r2, "same cost each time");
}

/// Darken Thunderbird: run the bank-0 flash routine (`$9235`) with the
/// Thunder flash timer set. Vanilla and the patched game away from
/// Thunderbird pick a flashing backdrop; with Thunderbird (world 5, enemy
/// `$20`) in a slot the patched game keeps the normal backdrop.
#[test]
#[ignore = "needs Z2_ROM"]
fn thunder_flash_is_dark_with_thunderbird() {
    let body = rom_body();
    let (mut v, mut r) = pair_in_gameplay(&body, flags(|f| f.qol.darken_thunderbird = true));
    let backdrop = |e: &mut Emu, world: u8, enemy: u8| {
        let s = e.game.save_state();
        // Map bank 0 at $8000 the way the game does (`LDA #0 : JSR $FFCC`).
        let (_, x, y, sp, pc, p) = e.game.cpu_state();
        e.game.set_cpu(0, x, y, sp, pc, p);
        e.game.call_asm(0xFFCC);
        let g = &mut e.game;
        g.ram[0x0301] = 0;
        g.ram[0x074B] = 0xA1;
        g.ram[0x0707] = world;
        g.ram[0x00B6] = 1;
        g.ram[0x00A1] = enemy;
        g.wram[0x09C8..0x09CE].copy_from_slice(&[0x21, 0x22, 0x23, 0x24, 0x0F, 0x05]);
        g.call_asm(0x9235);
        let bg = g.ram[0x0305];
        e.game.load_state(&s);
        bg
    };
    assert_eq!(backdrop(&mut v, 5, 0x20), 0x21, "vanilla flashes");
    assert_eq!(backdrop(&mut r, 0, 0x20), 0x21, "patched, not Great Palace");
    assert_eq!(backdrop(&mut r, 5, 0x05), 0x21, "patched, no Thunderbird");
    assert_eq!(
        backdrop(&mut r, 5, 0x20),
        0x0F,
        "patched, Thunderbird: no flash"
    );
}

/// Bug fix: the 300-point table entry reads 300 through the experience
/// port's bus reads.
#[test]
#[ignore = "needs Z2_ROM"]
fn bug_fix_bytes_reach_the_game() {
    let body = rom_body();
    let r = emu_with(&body, flags(|f| f.qol.bug_fixes = true), None);
    assert!(r.rom.untrapped.is_empty(), "{:X?}", r.rom.untrapped);
    let prg = &r.game.prg;
    let fixed = prg.len() - 0x4000;
    let lo = prg[fixed + 0x1DCC];
    let hi = prg[fixed + 0x1DE8];
    assert_eq!(u16::from(hi) << 8 | u16::from(lo), 300);
    assert_eq!(&prg[4 * 0x4000 + 0x3EB1..][..2], &[0xAA, 0xEA]);
}

// ---------------------------------------------------------------------------
// Cosmetics
// ---------------------------------------------------------------------------

/// Explicit colours land in the PPU's sprite palette 0 in gameplay, and the
/// Shield spell uses the chosen shield colour.
#[test]
#[ignore = "needs Z2_ROM"]
fn link_colours_reach_palette_ram() {
    let body = rom_body();
    let f = flags(|f| {
        f.cosmetic.tunic_outline = NesColor::Color(0x0F);
        f.cosmetic.skin_tone = NesColor::Color(0x27);
        f.cosmetic.tunic = NesColor::Color(0x11);
        f.cosmetic.shield_tunic = NesColor::Color(0x30);
    });
    let mut v = vanilla(&body);
    to_gameplay(&mut v);
    let mut r = emu_with(&body, f, None);
    to_gameplay(&mut r);
    assert!(r.rom.untrapped.is_empty(), "{:X?}", r.rom.untrapped);
    assert_eq!(&v.game.palette[0x11..0x14], &[0x18, 0x36, 0x2A]);
    assert_eq!(&r.game.palette[0x11..0x14], &[0x0F, 0x27, 0x11]);
    // Shield: tunic turns to the shield colour.
    arm_shield(&mut r);
    run(&mut r, 1, SELECT);
    run(&mut r, 60, 0); // past the cast flash
    assert_eq!(r.game.palette[0x13], 0x30, "shield tunic");
}

/// Random colours are reproducible per seed and differ between seeds.
#[test]
#[ignore = "needs Z2_ROM"]
fn random_colours_follow_the_seed() {
    let body = rom_body();
    let f = flags(|f| {
        f.cosmetic.tunic = NesColor::Random;
        f.cosmetic.skin_tone = NesColor::Random;
    });
    let tunic = |seed: &str| {
        let out = z2_rando::randomize(&body, seed, &f).unwrap();
        out.body[0x4000 + 0x00A0..][..2].to_vec()
    };
    assert_eq!(tunic("a"), tunic("a"));
    let all: std::collections::BTreeSet<Vec<u8>> = ["a", "b", "c", "d", "e", "f"]
        .iter()
        .map(|s| tunic(s))
        .collect();
    assert!(all.len() >= 4, "{all:02X?}");
}

/// Swing the sword at full life for 40 frames and collect the sword beam's
/// OAM `(tile, attributes)`. The beam is the projectile drawn with palette
/// 1 or 3 and tile `$32` (vanilla) or `$84` (patched); Link's own sword
/// uses `$32` with palette 0.
fn beam_sprites(e: &mut Emu) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    for f in 0..40 {
        run(e, 1, if f % 2 == 0 { B } else { 0 });
        out.extend(
            e.game
                .oam
                .chunks(4)
                .filter(|s| s[0] < 0xEF && matches!(s[1], 0x32 | 0x84) && s[2] & 3 != 0)
                .map(|s| (s[1], s[2])),
        );
    }
    out
}

/// The beam: vanilla draws tile `$32` with the palette cycling between 1
/// and 3; with Axe the beam is tile `$84` with steady palette 1, and `$84`
/// in the loaded CHR is the axe graphic from the player's ROM.
#[test]
#[ignore = "needs Z2_ROM"]
fn beam_sprite_is_drawn() {
    let body = rom_body();
    let src = z2_rando::cosmetic::beam_source(BeamSprite::Axe).unwrap();
    let (mut v, mut r) =
        pair_in_gameplay(&body, flags(|f| f.cosmetic.beam_sprite = BeamSprite::Axe));
    assert!(r.rom.untrapped.is_empty(), "{:X?}", r.rom.untrapped);
    let (page, tile) = src.tile.unwrap();
    let from = usize::from(page) * 0x2000 + usize::from(tile) * 16;
    let want = body[0x20000 + from..][..32].to_vec();
    assert_ne!(&v.game.chr[0x840..][..32], &want[..]);
    for p in [0usize, 2, 4, 9, 12] {
        assert_eq!(
            &r.game.chr[p * 0x2000 + 0x840..][..32],
            &want[..],
            "page {p}"
        );
    }
    let vs = beam_sprites(&mut v);
    let rs = beam_sprites(&mut r);
    eprintln!("beam sprites: vanilla {vs:02X?}, axe {rs:02X?}");
    assert!(!vs.is_empty() && !rs.is_empty(), "beams drawn");
    assert!(vs.iter().all(|&(t, _)| t == 0x32));
    let vpal: std::collections::BTreeSet<u8> = vs.iter().map(|&(_, a)| a & 3).collect();
    assert_eq!(vpal.len(), 2, "vanilla beam palette cycles");
    assert!(
        rs.iter().all(|&(t, a)| t == 0x84 && a & 3 == 1),
        "steady axe"
    );
}

/// Step `frames` frames with `pad` and count the frames in which the game
/// wrote an audible pulse volume (`$4000`/`$4004` low nibble), an open
/// triangle linear counter (`$4008` low 7 bits) or an audible noise volume
/// (`$400C`), from the APU register log.
fn audible_frames(e: &mut Emu, frames: usize, pad: u8) -> [usize; 3] {
    let mut n = [0; 3];
    for _ in 0..frames {
        e.game.step(pad);
        let mut hit = [false; 3];
        for (addr, v) in e.game.apu.drain_log() {
            match addr {
                0x4000 | 0x4004 => hit[0] |= v & 0x0F != 0,
                0x4008 => hit[1] |= v & 0x7F != 0,
                0x400C => hit[2] |= v & 0x0F != 0,
                _ => {}
            }
        }
        for k in 0..3 {
            n[k] += usize::from(hit[k]);
        }
    }
    n
}

/// Disable music: on the title screen the music's pulses, triangle and
/// drums never get a volume (only the title's short sound effects do), and
/// in gameplay the area music is silent while a sword swing still makes its
/// sound. The game's own state is unchanged (the music engine still runs).
#[test]
#[ignore = "needs Z2_ROM"]
fn disable_music_silences_music_but_not_effects() {
    let body = rom_body();
    let f = flags(|f| f.cosmetic.disable_music = true);
    let mut v = vanilla(&body);
    let mut r = emu_with(&body, f, None);
    assert!(r.rom.untrapped.is_empty());
    let va = audible_frames(&mut v, 600, 0);
    let ra = audible_frames(&mut r, 600, 0);
    eprintln!("title, audible frames [pulse, triangle, noise]: vanilla {va:?}, music off {ra:?}");
    assert!(va[0] > 400 && va[1] > 0, "vanilla title music");
    assert!(
        ra[0] * 8 < va[0],
        "only the title sound effects ({})",
        ra[0]
    );
    assert_eq!(ra[1], 0, "no triangle");
    assert_eq!(ra[2], 0, "no drums");
    assert_eq!(v.game.ram[..], r.game.ram[..], "music state still advances");
    // Gameplay: the North Castle music is silent; sword swings are heard.
    let (mut gv, mut gr) = pair_in_gameplay(&body, flags(|f| f.cosmetic.disable_music = true));
    let gva = audible_frames(&mut gv, 240, 0);
    let gra = audible_frames(&mut gr, 240, 0);
    let mut swings = [0; 3];
    for _ in 0..20 {
        let a = audible_frames(&mut gr, 1, B);
        let b = audible_frames(&mut gr, 5, 0);
        for k in 0..3 {
            swings[k] += a[k] + b[k];
        }
    }
    eprintln!("gameplay: vanilla {gva:?}, music off {gra:?}, music off + 20 swings {swings:?}");
    assert!(gva.iter().sum::<usize>() > 200, "vanilla area music");
    assert_eq!(gra, [0, 0, 0], "silent area");
    assert!(swings.iter().sum::<usize>() >= 20, "sword sound effects");
}

/// A bring-your-own sprite patch: its graphics reach the loaded CHR, code
/// bytes in it are dropped (the patched game is unchanged there and nothing
/// gets untrapped).
#[test]
#[ignore = "needs Z2_ROM"]
fn sprite_patch_graphics_only() {
    let body = rom_body();
    // Our own test patch: invert Link's first sprite tile in page 0, set
    // the tunic at the first sideview palette set, and try to overwrite
    // the reset code (must be ignored).
    let mut p = b"PATCH".to_vec();
    let chr_off = 0x10 + 0x20000;
    let tile: Vec<u8> = body[0x20000..0x20010].iter().map(|b| !b).collect();
    p.extend_from_slice(&[
        (chr_off >> 16) as u8,
        (chr_off >> 8) as u8,
        chr_off as u8,
        0,
        16,
    ]);
    p.extend_from_slice(&tile);
    let pal = 0x10 + 0x4000 + 0x00A1;
    p.extend_from_slice(&[(pal >> 16) as u8, (pal >> 8) as u8, pal as u8, 0, 1, 0x05]);
    let code = 0x10 + 7 * 0x4000 + 0x3F70; // fixed bank reset area
    p.extend_from_slice(&[
        (code >> 16) as u8,
        (code >> 8) as u8,
        code as u8,
        0,
        2,
        0xEA,
        0xEA,
    ]);
    p.extend_from_slice(b"EOF");
    let mut r = emu_with(&body, Flags::default(), Some(p));
    assert!(r.rom.untrapped.is_empty(), "code bytes were not applied");
    assert_eq!(&r.game.chr[..16], &tile[..]);
    assert_eq!(r.game.prg[0x4000 + 0xA1], 0x05);
    assert_eq!(
        &r.game.prg[r.game.prg.len() - 0x90..][..2],
        &body[7 * 0x4000 + 0x3F70..][..2]
    );
    to_gameplay(&mut r);
    assert_eq!(r.game.exec_errors, 0);
}
