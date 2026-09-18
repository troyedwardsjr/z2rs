//! `xtask verify --continue`: tolerant lockstep diagnostics.
//!
//! Unlike the gate ([`z2_verify::lockstep::Lockstep::run`]), this driver
//! keeps stepping past divergences and reports, per frame, *which regions*
//! disagree, so render-only problems deep into a movie can be located even
//! while earlier RAM drift exists. `--dump-at F,G,...` writes both sides'
//! frames as PNGs plus a text dump of PPU/mapper state, palette, OAM and
//! nametable/pattern-table diffs for each listed frame.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use z2_verify::lockstep::{Lockstep, Region};
use z2_verify::oracle::{Port, TetanesOracle};

use crate::verify::GamePort;

/// Options for the tolerant run.
pub(crate) struct ContinueOpts<'a> {
    pub frames: usize,
    pub dump_at: &'a [usize],
    pub out: &'a Path,
    pub trace_mode: bool,
    /// Print at most this many region-set change lines (0 = unlimited).
    pub max_lines: usize,
    /// CPU RAM addresses to trace on change (either side).
    pub watch: &'a [u16],
}

/// Region key for the "set changed" tracking.
fn region_key(r: Region) -> u8 {
    match r {
        Region::ZeroPageStack => 0,
        Region::MainRam => 1,
        Region::Wram => 2,
        Region::Oam => 3,
        Region::Palette => 4,
        Region::Frame => 5,
    }
}

fn region_name(k: u8) -> &'static str {
    match k {
        0 => "zp+stack",
        1 => "ram",
        2 => "wram",
        3 => "oam",
        4 => "palette",
        _ => "frame",
    }
}

pub(crate) fn run_continue(
    oracle: &mut TetanesOracle,
    game: &mut GamePort,
    track: &[u8],
    opts: &ContinueOpts<'_>,
) -> i32 {
    let mut prev_set: BTreeSet<u8> = BTreeSet::new();
    let mut first_frame: [Option<usize>; 6] = [None; 6];
    let mut count: [usize; 6] = [0; 6];
    let mut lines = 0usize;
    let mut prev_mode = (0xFFu8, 0xFFu8);
    let mut any_div = false;
    let mut prev_watch = String::new();
    for f in 0..opts.frames {
        let input = track.get(f).copied().unwrap_or(0);
        oracle.step(input);
        game.step(input);
        let divs = Lockstep::compare_all(oracle, game, f as u64);
        let set: BTreeSet<u8> = divs.iter().map(|d| region_key(d.region)).collect();
        for d in &divs {
            let k = region_key(d.region) as usize;
            count[k] += 1;
            if first_frame[k].is_none() {
                first_frame[k] = Some(f);
            }
        }
        if !divs.is_empty() {
            any_div = true;
        }
        if set != prev_set && (opts.max_lines == 0 || lines < opts.max_lines) {
            let names: Vec<&str> = set.iter().map(|&k| region_name(k)).collect();
            let mut line = format!("frame {f}: regions [{}]", names.join(","));
            for d in &divs {
                let _ = write!(
                    line,
                    " | {} {:#06X} e={:#04X} a={:#04X}",
                    d.region, d.addr, d.expected, d.actual
                );
            }
            println!("{line}");
            lines += 1;
            prev_set = set;
        }
        if !opts.watch.is_empty() {
            let mut line = String::new();
            for &a in opts.watch {
                let o = oracle.ram()[a as usize];
                let gg = game.ram()[a as usize];
                let _ = write!(line, " ${a:04X}={o:02X}/{gg:02X}");
            }
            if line != prev_watch {
                println!("frame {f}: watch{line}");
                prev_watch = line;
            }
        }
        if opts.trace_mode {
            let mode = (oracle.ram()[0x736], game.ram()[0x736]);
            if mode != prev_mode {
                println!(
                    "frame {f}: mode $736 oracle={:#04X} game={:#04X}",
                    mode.0, mode.1
                );
                prev_mode = mode;
            }
        }
        if opts.dump_at.contains(&f) {
            if let Err(e) = dump_frame(oracle, game, f, &divs, opts.out) {
                eprintln!("dump frame {f}: {e}");
            } else {
                eprintln!(
                    "dumped frame {f} to {}",
                    opts.out.join(f.to_string()).display()
                );
            }
        }
    }
    println!("--- summary over {} frames ---", opts.frames);
    for k in 0..6u8 {
        let kk = k as usize;
        match first_frame[kk] {
            Some(ff) => println!(
                "{:>9}: {} mismatching frames, first at frame {ff}",
                region_name(k),
                count[kk]
            ),
            None => println!("{:>9}: clean", region_name(k)),
        }
    }
    i32::from(any_div)
}

fn hex_line(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for b in bytes {
        let _ = write!(s, "{b:02X} ");
    }
    s
}

fn dump_frame(
    oracle: &TetanesOracle,
    game: &GamePort,
    f: usize,
    divs: &[z2_verify::lockstep::Divergence],
    out: &Path,
) -> Result<(), String> {
    let dir = out.join(f.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let wr = |name: &str, bytes: &[u8]| -> Result<(), String> {
        let p = dir.join(name);
        std::fs::write(&p, bytes).map_err(|e| format!("write {}: {e}", p.display()))
    };
    wr(
        "oracle.png",
        &z2_ppu::encode_indexed_png(oracle.frame_indexed()),
    )?;
    wr(
        "game.png",
        &z2_ppu::encode_indexed_png(game.frame_indexed()),
    )?;
    wr("oracle.idx", oracle.frame_indexed())?;
    wr("game.idx", game.frame_indexed())?;

    let g = &game.0;
    let gp = g.ppu.model();
    let or = oracle.ppu_regs();
    let mut t = String::new();
    let _ = writeln!(t, "frame {f}");
    for d in divs {
        let _ = writeln!(t, "DIVERGENCE {d}");
    }
    // Frame pixel diff summary.
    let of = oracle.frame_indexed();
    let gf = game.frame_indexed();
    let mut rows_diff = Vec::new();
    for y in 0..240 {
        let a = &of[y * 256..(y + 1) * 256];
        let b = &gf[y * 256..(y + 1) * 256];
        let n = Lockstep::count_diff(a, b);
        if n > 0 {
            rows_diff.push((y, n));
        }
    }
    let total: usize = rows_diff.iter().map(|(_, n)| n).sum();
    let _ = writeln!(
        t,
        "frame pixels differing: {total} over {} rows (first row {:?}, last row {:?})",
        rows_diff.len(),
        rows_diff.first().map(|r| r.0),
        rows_diff.last().map(|r| r.0)
    );
    // Row band summary: contiguous runs of differing rows.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &(y, _) in &rows_diff {
        match runs.last_mut() {
            Some((_, end)) if *end + 1 == y => *end = y,
            _ => runs.push((y, y)),
        }
    }
    let _ = writeln!(t, "differing row runs: {runs:?}");
    let mut listed = 0usize;
    let mut pix = String::new();
    for (i, (&e, &a)) in of.iter().zip(gf.iter()).enumerate() {
        if e != a && listed < 48 {
            let _ = write!(pix, "({},{}) e={e:02X} a={a:02X}; ", i % 256, i / 256);
            listed += 1;
        }
    }
    let _ = writeln!(t, "first differing pixels: {pix}");

    let _ = writeln!(t, "\n== oracle PPU regs ==\n{or:#?}");
    let (sx, sy) = gp.scroll();
    let _ = writeln!(
        t,
        "\n== game PPU regs ==\nctrl=${:02X} mask=${:02X} status=${:02X} scroll=({sx},{sy}) v=${:04X} t=${:04X} fine_x={} hit_at={:?} chr_pages=({},{}) mirroring={:?} last_hit={}",
        gp.ctrl(),
        gp.mask(),
        gp.status(),
        gp.v(),
        gp.t(),
        gp.fine_x(),
        gp.sprite0_hit_at(),
        gp.chr_page_no(0),
        gp.chr_page_no(1),
        gp.mirroring(),
        g.ppu.last_render_hit()
    );
    let m = &g.mmc1;
    let _ = writeln!(
        t,
        "game MMC1: ctrl=${:02X} chr0=${:02X} chr1=${:02X} prg=${:02X} (map_chr4: {} {})",
        m.ctrl,
        m.chr0,
        m.chr1,
        m.prg,
        m.map_chr4(0),
        m.map_chr4(1)
    );
    let _ = writeln!(
        t,
        "game CPU: pc=${:04X} cycles={} frame_count={} exec_errors={}",
        g.cpu.pc,
        g.cpu.cycles,
        g.frame_count(),
        g.exec_errors
    );
    let watch: [u16; 14] = [
        0x0736, 0x00FF, 0x00FE, 0x00FD, 0x00FC, 0x0746, 0x0747, 0x0768, 0x0725, 0x0100, 0x0012,
        0x0769, 0x071D, 0x071E,
    ];
    let _ = writeln!(t, "\n== RAM watch (oracle / game) ==");
    for a in watch {
        let _ = writeln!(
            t,
            "${a:04X}: {:02X} / {:02X}",
            oracle.ram()[a as usize],
            game.ram()[a as usize]
        );
    }
    let _ = writeln!(
        t,
        "\n== palette ==\noracle: {}\ngame:   {}",
        hex_line(oracle.palette()),
        hex_line(game.palette())
    );
    let _ = writeln!(t, "\n== OAM (64 entries, y tile attr x) ==");
    for i in 0..64 {
        let o = &oracle.oam()[i * 4..i * 4 + 4];
        let gg = &game.oam()[i * 4..i * 4 + 4];
        let _ = writeln!(t, "{i:2}: oracle {} | game {}", hex_line(o), hex_line(gg));
    }
    // Nametable diffs per logical 1 KiB slot.
    let _ = writeln!(
        t,
        "\n== nametable diffs (logical $2000/$2400/$2800/$2C00) =="
    );
    for slot in 0..4u16 {
        let base = 0x2000 + slot * 0x400;
        let mut n = 0usize;
        let mut first = None;
        let mut attr_n = 0usize;
        for i in 0..0x400u16 {
            let a = oracle.ppu_peek(base + i);
            let b = gp.nt_read(base + i);
            if a != b {
                n += 1;
                if i >= 0x3C0 {
                    attr_n += 1;
                }
                if first.is_none() {
                    first = Some((base + i, a, b));
                }
            }
        }
        let _ = writeln!(
            t,
            "NT slot {slot} (${base:04X}): {n} bytes differ ({attr_n} attribute bytes); first {first:?}"
        );
    }
    let _ = writeln!(t, "\n== pattern-table diffs ==");
    for slot in 0..2usize {
        let g_slot = gp.chr_slot(slot);
        let n = g_slot
            .iter()
            .enumerate()
            .filter(|(i, &b)| oracle.ppu_peek((slot * 0x1000 + i) as u16) != b)
            .count();
        let _ = writeln!(
            t,
            "CHR slot {slot}: {n} bytes differ (game page {})",
            gp.chr_page_no(slot)
        );
    }
    wr("info.txt", t.as_bytes())?;

    // Full nametable dumps (tile hex, 32 cols x 30 rows, then attributes).
    for (name, side) in [("nt_oracle.txt", 0), ("nt_game.txt", 1)] {
        let mut s = String::new();
        for slot in 0..4u16 {
            let base = 0x2000 + slot * 0x400;
            let _ = writeln!(s, "== ${base:04X} ==");
            for row in 0..30u16 {
                let mut line = String::new();
                for col in 0..32u16 {
                    let a = base + row * 32 + col;
                    let b = if side == 0 {
                        oracle.ppu_peek(a)
                    } else {
                        gp.nt_read(a)
                    };
                    let _ = write!(line, "{b:02X} ");
                }
                let _ = writeln!(s, "{line}");
            }
            let _ = writeln!(s, "-- attr --");
            for row in 0..8u16 {
                let mut line = String::new();
                for col in 0..8u16 {
                    let a = base + 0x3C0 + row * 8 + col;
                    let b = if side == 0 {
                        oracle.ppu_peek(a)
                    } else {
                        gp.nt_read(a)
                    };
                    let _ = write!(line, "{b:02X} ");
                }
                let _ = writeln!(s, "{line}");
            }
        }
        wr(name, s.as_bytes())?;
    }
    Ok(())
}
