//! Article screenshot driver: widescreen + co-op stills from anywhere in the
//! game. A developer tool, not part of the product; see
//! README.md.
//!
//! One reference replay of a movie runs with co-op OFF (the verified path).
//! Each spec line is a *graft*: the reference state at frame `G` is loaded
//! into a second game with co-op ON, the second Link anchors beside player 1,
//! RAM pokes are applied, and the graft runs `N` more frames on scripted pads
//! before it is presented through the same [`app::Display`] path the window
//! uses (widescreen, scale, optional HD pack). Grafting keeps a fighting
//! player 2 from desynchronising the movie.
//!
//! Every output is derived from the user's ROM, so `--out` (like `--record`)
//! refuses a directory inside a git work tree (LEGAL.md).
//!
//! ```text
//! article_shots [--rom R] [--movie M] --out DIR [--spec FILE] [--scale S]
//!               [--main VARIANT] [--survey STEP] [--from F] [--to F]
//!               [--hd-pack DIR] [--record DIR] [--chrbug]
//! ```
//!
//! * `--survey STEP` dumps a plain 256x240 PNG plus a `survey.tsv` row every
//!   `STEP` reference frames (scouting).
//! * `--main` picks the variant written as `<name>.png` (default `wide`).
//! * `--hd-pack` / `--record` load / record an HD pack on the present path.
//! * `--chrbug` truncates CHR to 64 KiB before the first frame (recreates the
//!   short-iNES-header "black overworld" bug).
//!
//! Spec lines: `name G N p1 p2 [opts]` (`#` comments, `-` = default; `N = 0`
//! presents the reference itself at frame `G`).
//!
//! p1/p2 scripts are comma-separated segments, run in order, then idle:
//!
//! * `t` / `t120`: the movie's pad-1 byte (whole window / 120 frames);
//! * `m30` / `m30*90`: the movie's pad 1 delayed 30 frames; `n30` is the same
//!   with B (attack) masked out;
//! * `RA20`, `_10`: hold buttons (`U D L R A B`, `S` Start, `s` Select) or
//!   nothing.
//!
//! opts, comma-separated:
//!
//! * `solo` no second Link; `ghost` player 2 without contact passes;
//!   `dx=N` player 2 spawn offset in pixels right of player 1 (i8);
//! * `burst=CxK` C captures K frames apart ending at `N`; `both` defers each
//!   capture (up to 16 frames) until both Links are drawn (they blink while
//!   player 1 is hurt);
//! * `poke=AAAA:VV/@N:AAAA:VV` RAM/WRAM writes at window frame 0 or `N`;
//!   `dump=AAAA:LEN` hex dump in the log;
//! * `find=edge` / `find=dialog:DELAY` extra captures when a non-Link sprite
//!   straddles the window's right edge / a dialogue is open;
//! * `v=a+b` extra variants: `native` (4:3), `w1610`, `w16`, `noclip`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use z2_native::app::{self, Display, DisplaySettings, Emu, Features};

const A: u8 = 0x01;
const B: u8 = 0x02;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;
const UP: u8 = 0x10;
const DOWN: u8 = 0x20;
const LEFT: u8 = 0x40;
const RIGHT: u8 = 0x80;

#[derive(Debug, Clone)]
enum Seg {
    /// Movie pad 1 passthrough.
    Tas(Option<usize>),
    /// Movie pad 1 delayed by `delay`, with `mask` removed.
    Mirror {
        delay: usize,
        mask: u8,
        len: Option<usize>,
    },
    /// Constant byte.
    Hold(u8, usize),
}

fn parse_script(s: &str) -> Result<Vec<Seg>, String> {
    let mut out = Vec::new();
    if s == "-" || s.is_empty() {
        return Ok(out);
    }
    for part in s.split(',') {
        let part = part.trim();
        let first = part.chars().next().ok_or("empty segment")?;
        match first {
            't' => {
                let n = &part[1..];
                out.push(Seg::Tas(if n.is_empty() {
                    None
                } else {
                    Some(n.parse().map_err(|_| format!("bad count in '{part}'"))?)
                }));
            }
            'm' | 'n' => {
                let body = &part[1..];
                let (d, len) = match body.split_once('*') {
                    Some((d, l)) => (
                        d,
                        Some(l.parse().map_err(|_| format!("bad len in '{part}'"))?),
                    ),
                    None => (body, None),
                };
                out.push(Seg::Mirror {
                    delay: d.parse().map_err(|_| format!("bad delay in '{part}'"))?,
                    mask: if first == 'n' { B } else { 0 },
                    len,
                });
            }
            _ => {
                let letters: String = part.chars().take_while(|c| !c.is_ascii_digit()).collect();
                let count: usize = part[letters.len()..]
                    .parse()
                    .map_err(|_| format!("bad count in '{part}'"))?;
                let mut b = 0u8;
                for c in letters.chars() {
                    b |= match c {
                        'U' => UP,
                        'D' => DOWN,
                        'L' => LEFT,
                        'R' => RIGHT,
                        'A' => A,
                        'B' => B,
                        'S' => START,
                        's' => SELECT,
                        '_' => 0,
                        other => return Err(format!("bad button '{other}' in '{part}'")),
                    };
                }
                out.push(Seg::Hold(b, count));
            }
        }
    }
    Ok(out)
}

/// Pad byte for window-relative frame `i` (absolute movie frame `g + i`).
fn script_byte(script: &[Seg], track: &[u8], g: usize, i: usize) -> u8 {
    let at = |f: usize| track.get(f).copied().unwrap_or(0);
    let mut start = 0usize;
    for seg in script {
        let (len, val): (Option<usize>, u8) = match seg {
            Seg::Tas(len) => (*len, at(g + i)),
            Seg::Mirror { delay, mask, len } => (
                *len,
                if i >= *delay {
                    at(g + i - delay) & !mask
                } else {
                    0
                },
            ),
            Seg::Hold(b, n) => (Some(*n), *b),
        };
        match len {
            None => return val,
            Some(n) if i < start + n => return val,
            Some(n) => start += n,
        }
    }
    0
}

#[derive(Debug, Clone)]
struct Shot {
    name: String,
    graft: usize,
    run: usize,
    p1: Vec<Seg>,
    p2: Vec<Seg>,
    dx: i8,
    solo: bool,
    ghost: bool,
    burst: (usize, usize),
    variants: Vec<String>,
    /// `(window frame, address, value)` RAM/WRAM writes.
    pokes: Vec<(usize, u16, u8)>,
    /// `(address, length)` hex dumps printed at capture time.
    dumps: Vec<(u16, usize)>,
    /// Extra captures when a condition first holds: `edge` (a non-Link sprite
    /// straddling the window's right edge) or `dialog:DELAY`.
    find: Option<(String, usize)>,
    /// Defer each capture (up to 16 frames) until both Links are drawn.
    need_both: bool,
}

/// RAM (`$0000-$07FF`) or battery WRAM (`$6000-$7FFF`), the two images a
/// spec may poke or dump.
fn mem_addr(text: &str) -> Result<u16, String> {
    let addr = u16::from_str_radix(text, 16).map_err(|_| format!("bad address '{text}'"))?;
    match addr {
        0x0000..=0x07FF | 0x6000..=0x7FFF => Ok(addr),
        _ => Err(format!("${addr:04X} is neither RAM nor WRAM")),
    }
}

fn parse_spec(text: &str) -> Result<Vec<Shot>, String> {
    let mut shots = Vec::new();
    for (ln, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 5 {
            return Err(format!(
                "spec line {}: want `name G N p1 p2 [opts]`",
                ln + 1
            ));
        }
        let err = |e: String| format!("spec line {}: {e}", ln + 1);
        let mut shot = Shot {
            name: f[0].to_string(),
            graft: f[1].parse().map_err(|_| err("bad G".into()))?,
            run: f[2].parse().map_err(|_| err("bad N".into()))?,
            p1: if f[3] == "-" {
                vec![Seg::Tas(None)]
            } else {
                parse_script(f[3]).map_err(err)?
            },
            p2: parse_script(f[4]).map_err(err)?,
            dx: 0x20,
            solo: false,
            ghost: false,
            burst: (1, 1),
            variants: Vec::new(),
            pokes: Vec::new(),
            dumps: Vec::new(),
            find: None,
            need_both: false,
        };
        if let Some(opts) = f.get(5) {
            for o in opts.split(',') {
                if o == "both" {
                    shot.need_both = true;
                } else if o == "solo" {
                    shot.solo = true;
                } else if o == "ghost" {
                    shot.ghost = true;
                } else if let Some(v) = o.strip_prefix("dx=") {
                    shot.dx = v.parse().map_err(|_| err(format!("bad dx '{v}'")))?;
                } else if let Some(v) = o.strip_prefix("burst=") {
                    let (c, k) = v.split_once('x').ok_or_else(|| err("burst=CxK".into()))?;
                    shot.burst = (
                        c.parse().map_err(|_| err("bad burst count".into()))?,
                        k.parse().map_err(|_| err("bad burst step".into()))?,
                    );
                    if shot.burst.0 == 0 || shot.burst.1 == 0 {
                        return Err(err("burst=CxK needs C >= 1 and K >= 1".into()));
                    }
                } else if let Some(v) = o.strip_prefix("poke=") {
                    for item in v.split('/') {
                        let mut parts: Vec<&str> = item.split(':').collect();
                        let at = if parts[0].starts_with('@') {
                            let n = parts.remove(0);
                            n[1..]
                                .parse()
                                .map_err(|_| err(format!("bad poke frame '{n}'")))?
                        } else {
                            0usize
                        };
                        if parts.len() != 2 {
                            return Err(err(format!("bad poke '{item}'")));
                        }
                        let addr = mem_addr(parts[0]).map_err(&err)?;
                        let val = u8::from_str_radix(parts[1], 16)
                            .map_err(|_| err(format!("bad poke val '{item}'")))?;
                        shot.pokes.push((at, addr, val));
                    }
                } else if let Some(v) = o.strip_prefix("dump=") {
                    for item in v.split('/') {
                        let (a, l) = item
                            .split_once(':')
                            .ok_or_else(|| err("dump=ADDR:LEN".into()))?;
                        let addr = mem_addr(a).map_err(&err)?;
                        let len =
                            usize::from_str_radix(l, 16).map_err(|_| err("bad dump len".into()))?;
                        mem_addr(&format!("{:X}", usize::from(addr) + len.saturating_sub(1)))
                            .map_err(&err)?;
                        shot.dumps.push((addr, len));
                    }
                } else if let Some(v) = o.strip_prefix("find=") {
                    let (k, d) = v.split_once(':').unwrap_or((v, "0"));
                    shot.find = Some((
                        k.to_string(),
                        d.parse().map_err(|_| err("bad find delay".into()))?,
                    ));
                } else if let Some(v) = o.strip_prefix("v=") {
                    shot.variants = v.split('+').map(str::to_string).collect();
                } else {
                    return Err(err(format!("unknown opt '{o}'")));
                }
            }
        }
        shots.push(shot);
    }
    Ok(shots)
}

struct Displays {
    pack: Option<PathBuf>,
    record: Option<PathBuf>,
    scale: u32,
    map: HashMap<String, Display>,
}

impl Displays {
    fn get(&mut self, variant: &str) -> Result<&mut Display, String> {
        if !self.map.contains_key(variant) {
            let (tiles, fill) = match variant {
                "wide" => (11, true),
                "w1610" => (8, true),
                "w16" => (16, true),
                "native" => (0, true),
                "noclip" => (11, false),
                other => return Err(format!("unknown variant '{other}'")),
            };
            let d = Display::new(DisplaySettings {
                wide_tiles: tiles,
                scale: self.scale,
                fill_left_clip: fill,
                fill_right_clip: fill,
                pack_dir: self.pack.clone(),
                record_dir: if variant == "wide" {
                    self.record.clone()
                } else {
                    None
                },
            })?;
            self.map.insert(variant.to_string(), d);
        }
        Ok(self.map.get_mut(variant).expect("just inserted"))
    }

    fn dump(&mut self, variant: &str, emu: &Emu, path: &Path) -> Result<(), String> {
        let d = self.get(variant)?;
        let (w, h) = d.size();
        let rgba = d.present(&emu.game)?.to_vec();
        let png = z2_render::encode_png_rgba(w, h, &rgba, &[]).map_err(|e| format!("png: {e}"))?;
        std::fs::write(path, png).map_err(|e| format!("write {}: {e}", path.display()))
    }
}

fn facts_row(emu: &Emu) -> String {
    let r = &emu.game.ram;
    format!(
        "{}\t{:02X}\t{}\t{}\t{:02X}\t{:02X}\t{}\t{:02X}\t{:02X}\t{:02X}\t{}\t{:02X}{:02X}\t{}{}{}",
        emu.game.frame_count(),
        r[0x0736],
        r[0x0707],
        r[0x0706],
        r[0x0748],
        r[0x0561],
        r[0x003B],
        r[0x004D],
        r[0x0029],
        r[0x0774],
        r[0x0700],
        r[0x0775],
        r[0x0776],
        r[0x0777],
        r[0x0778],
        r[0x0779],
    )
}

fn main() {
    if let Err(e) = run() {
        eprintln!("article_shots: {e}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let argv: Vec<String> = std::env::args().collect();
    let mut rom = std::env::var("Z2_ROM").ok();
    let mut movie = None;
    let mut out = None;
    let mut spec = None;
    let mut scale = 1u32;
    let mut survey = 0usize;
    let mut main_variant = String::from("wide");
    let mut chrbug = false;
    let mut pack: Option<PathBuf> = None;
    let mut record: Option<PathBuf> = None;
    let mut from = 0usize;
    let mut to = usize::MAX;
    let mut it = argv.iter().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--rom" => rom = Some(val()?),
            "--movie" => movie = Some(val()?),
            "--out" => out = Some(val()?),
            "--spec" => spec = Some(val()?),
            "--scale" => scale = val()?.parse().map_err(|_| "bad --scale")?,
            "--survey" => survey = val()?.parse().map_err(|_| "bad --survey")?,
            "--main" => main_variant = val()?,
            "--chrbug" => chrbug = true,
            "--hd-pack" => pack = Some(PathBuf::from(val()?)),
            "--record" => record = Some(PathBuf::from(val()?)),
            "--from" => from = val()?.parse().map_err(|_| "bad --from")?,
            "--to" => to = val()?.parse().map_err(|_| "bad --to")?,
            other => return Err(format!("unknown flag {other}")),
        }
    }
    let rom = PathBuf::from(rom.ok_or("--rom or $Z2_ROM")?);
    let out = PathBuf::from(out.ok_or("--out DIR")?);
    if let Some(root) = z2_render::fs::enclosing_git_worktree(&out) {
        return Err(format!(
            "--out {}: refusing to write ROM-derived frames into the git work tree at {} \
             (LEGAL.md) — pick a directory outside the repository",
            out.display(),
            root.display()
        ));
    }
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let track: Vec<u8> = match &movie {
        Some(m) => app::load_movie_track(Path::new(m))?,
        None => Vec::new(),
    };
    let mut shots = match &spec {
        Some(p) => parse_spec(&std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?)?,
        None => Vec::new(),
    };
    shots.sort_by_key(|s| s.graft);
    let last_needed = shots.iter().map(|s| s.graft).max().unwrap_or(0);
    let end = if survey > 0 {
        to.min(track.len().max(last_needed))
    } else {
        last_needed
    };

    // Reference: co-op OFF (the verified path); the record is display-only.
    let (mut a, _) = app::emu_from_rom_file_with(
        &rom,
        44_100,
        Features {
            coop: false,
            record: true,
        },
    )?;
    // Graft target: co-op registered before reset, as the frontends do.
    let (mut b, _) = app::emu_from_rom_file_with(
        &rom,
        44_100,
        Features {
            coop: true,
            record: true,
        },
    )?;
    if chrbug {
        // Recreate the short-header bug: only the first 64 KiB of CHR exists,
        // so `PpuBind::sync_from_mapper` cannot load any 4 KiB page >= 16.
        a.game.chr.truncate(0x1_0000);
        b.game.chr.truncate(0x1_0000);
    }
    let mut displays = Displays {
        pack,
        record,
        scale,
        map: HashMap::new(),
    };
    let mut survey_rows =
        String::from("frame\tmode\tworld\tregion\tarea\tscene\tpage\tx\ty\thp\tlives\texp\tlvl\n");
    let mut log = String::new();
    let mut next = 0usize;
    let mut f = 0usize;
    loop {
        // Grafts whose start frame is the reference's current frame.
        while next < shots.len() && shots[next].graft == f {
            let shot = shots[next].clone();
            next += 1;
            let state = a.game.save_state();
            b.game.load_state(&state);
            if !shot.solo {
                b.game.set_coop(true);
                b.game.set_coop_options(z2_core::coop::CoopOptions {
                    p2_x_offset: shot.dx as u8,
                    p2_contact: !shot.ghost,
                    ..Default::default()
                });
                b.game.coop_reset_area();
            } else {
                b.game.set_coop(false);
            }
            let (count, step) = shot.burst;
            let first_cap = shot.run.saturating_sub(count.saturating_sub(1) * step);
            let mut cap = 0usize;
            let mut found_at: Vec<usize> = Vec::new();
            let mut pending: Option<usize> = None;
            let mut owed: usize = 0;
            let mut owed_age: usize = 0;
            for i in 0..=shot.run {
                let mut found_now = false;
                if let Some((kind, delay)) = &shot.find {
                    if i > 0 && found_at.len() < 4 && pending.is_none() {
                        let hit = match kind.as_str() {
                            "dialog" => b.game.ram[0x074C] == 2,
                            "edge" => {
                                // OAM shadow $0200: a visible sprite cut by x 256,
                                // outside Link's slots (2..=9) and P2's (18,19,56..=61).
                                b.game.ram[0x0736] == 0x0B
                                    && (0..64usize).any(|n| {
                                        let o = 0x0200 + n * 4;
                                        let (y, x) = (b.game.ram[o], b.game.ram[o + 3]);
                                        let link = (2..=9).contains(&n)
                                            || n == 18
                                            || n == 19
                                            || (56..=61).contains(&n);
                                        !link && n != 0 && (0x30..0xD0).contains(&y) && x >= 0xF3
                                    })
                            }
                            _ => false,
                        };
                        let spaced = found_at.last().is_none_or(|&l| i >= l + 45);
                        if hit && spaced {
                            pending = Some(i + delay);
                        }
                    }
                    if pending == Some(i) {
                        pending = None;
                        found_at.push(i);
                        found_now = true;
                    }
                }
                if found_now {
                    let base = out.join(format!("{}-f{}.png", shot.name, found_at.len()));
                    displays.dump(&main_variant, &b, &base)?;
                    for v in &shot.variants {
                        let p = out.join(format!("{}-f{}.{v}.png", shot.name, found_at.len()));
                        displays.dump(v, &b, &p)?;
                    }
                    log.push_str(&format!(
                        "{}-f{}\tframe={}\tFOUND\n",
                        shot.name,
                        found_at.len(),
                        shot.graft + i
                    ));
                }
                let scheduled =
                    i >= first_cap && (i - first_cap) % step == 0 && i > 0 || (shot.run == 0);
                if scheduled {
                    owed += 1;
                    owed_age = 0;
                }
                let both_ok = !shot.need_both || owed_age >= 16 || i == shot.run || {
                    let vis =
                        |lo: usize, hi: usize| (lo..=hi).any(|n| b.game.ram[0x0200 + n * 4] < 0xEF);
                    vis(2, 7) && vis(56, 61)
                };
                if owed > 0 && !both_ok {
                    owed_age += 1;
                }
                if owed > 0 && both_ok {
                    owed -= 1;
                    let suffix = if count > 1 {
                        format!("-{cap}")
                    } else {
                        String::new()
                    };
                    let src: &Emu = if shot.run == 0 { &a } else { &b };
                    let base = out.join(format!("{}{suffix}.png", shot.name));
                    displays.dump(&main_variant, src, &base)?;
                    for v in &shot.variants {
                        let p = out.join(format!("{}{suffix}.{v}.png", shot.name));
                        displays.dump(v, src, &p)?;
                    }
                    let st = src.game.coop_status();
                    log.push_str(&format!(
                        "{}{suffix}\tframe={}\tmode={:02X}\tp1=({},{:02X},{:02X})\ten={:02X?}\tdlg={:02X}\tp2={}\n",
                        shot.name,
                        shot.graft + i,
                        src.game.ram[0x0736],
                        src.game.ram[0x003B],
                        src.game.ram[0x004D],
                        src.game.ram[0x0029],
                        &src.game.ram[0x00A1..0x00A7],
                        src.game.ram[0x074C],
                        match st {
                            Some(s) => format!(
                                "active={} alive={} hp={} page={} x={:02X} y={:02X} sx={}",
                                s.active, s.p2_alive, s.p2_hp, s.p2_page, s.p2_x, s.p2_y,
                                s.p2_screen_x
                            ),
                            None => "off".into(),
                        }
                    ));
                    for &(addr, len) in &shot.dumps {
                        let mut line = format!("  dump ${addr:04X}:");
                        for k in 0..len {
                            let a = usize::from(addr) + k;
                            let v = if a < 0x800 {
                                src.game.ram[a]
                            } else {
                                src.game.wram[a - 0x6000]
                            };
                            if k % 32 == 0 {
                                line.push_str(&format!("\n   {:04X}:", a));
                            }
                            line.push_str(&format!(" {v:02X}"));
                        }
                        log.push_str(&line);
                        log.push('\n');
                    }
                    cap += 1;
                }
                if i == shot.run {
                    break;
                }
                for &(at, addr, val) in &shot.pokes {
                    if at == i {
                        match addr {
                            0x0000..=0x07FF => b.game.ram[usize::from(addr)] = val,
                            _ => b.game.wram[usize::from(addr) - 0x6000] = val,
                        }
                    }
                }
                let p1 = script_byte(&shot.p1, &track, shot.graft, i);
                let p2 = script_byte(&shot.p2, &track, shot.graft, i);
                app::step_one2(&mut b, (p1, p2), None);
            }
        }
        if survey > 0 && f >= from && f.is_multiple_of(survey) {
            let png = z2_ppu::encode_indexed_png(a.game.frame_indexed());
            std::fs::write(out.join(format!("s{f:06}.png")), png).map_err(|e| e.to_string())?;
            survey_rows.push_str(&facts_row(&a));
            survey_rows.push('\n');
        }
        if f >= end {
            break;
        }
        app::step_one(&mut a, track.get(f).copied().unwrap_or(0), None);
        f += 1;
    }
    if survey > 0 {
        std::fs::write(out.join("survey.tsv"), survey_rows).map_err(|e| e.to_string())?;
    }
    if let Some(d) = displays.map.get("wide") {
        if let Some(dir) = d.write_recorded_pack(&a.game.chr)? {
            eprintln!("article_shots: recorded pack -> {}", dir.display());
        }
    }
    if !log.is_empty() {
        std::fs::write(out.join("shots.log"), &log).map_err(|e| e.to_string())?;
        print!("{log}");
    }
    eprintln!(
        "article_shots: reference ran {f} frames, faults={}",
        a.game.exec_errors
    );
    Ok(())
}
