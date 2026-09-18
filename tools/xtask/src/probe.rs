//! `xtask probe`: sub-frame timing probe of one frame, oracle vs game.
//!
//! Steps both sides to frame `N`, then walks frame `N` itself: the oracle
//! instruction by instruction (recording NMI entry, the sprite-0 flag rise,
//! `v`/`t` changes and `$2002`-spin exits with their scanline/dot), the
//! game through its PPU event trace (register accesses with the beam
//! position the model assigned). Prints both timelines for comparison.

use std::path::Path;

use z2_verify::oracle::{Oracle, Port, TetanesOracle};

use crate::verify::{load_movie_track, rom_path, GamePort};

fn usage() -> &'static str {
    "cargo xtask probe --frame N [--movie M] [--rom PATH] [--no-traps] [--max-events K] [--pc a,b]"
}

pub fn run(args: &[String]) -> i32 {
    let mut frame: usize = 100;
    let mut movie: Option<String> = None;
    let mut rom: Option<String> = None;
    let mut traps = true;
    let mut max_events: usize = 400;
    let mut pcs: Vec<u16> = Vec::new();
    let mut lines: Vec<u8> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--frame" => frame = it.next().and_then(|s| s.parse().ok()).unwrap_or(100),
            "--movie" => movie = it.next().cloned(),
            "--rom" => rom = it.next().cloned(),
            "--no-traps" => traps = false,
            "--max-events" => max_events = it.next().and_then(|s| s.parse().ok()).unwrap_or(400),
            "--lines" => {
                let list = it.next().cloned().unwrap_or_default();
                for part in list.split(',').filter(|p| !p.is_empty()) {
                    if let Ok(n) = part.parse::<u8>() {
                        lines.push(n);
                    }
                }
            }
            "--pc" => {
                let list = it.next().cloned().unwrap_or_default();
                for part in list.split(',').filter(|p| !p.is_empty()) {
                    if let Ok(n) = u16::from_str_radix(part.trim_start_matches('$'), 16) {
                        pcs.push(n);
                    }
                }
            }
            "-h" | "--help" => {
                println!("{}", usage());
                return 0;
            }
            other => {
                eprintln!("xtask probe: unknown arg '{other}'\n{}", usage());
                return 2;
            }
        }
    }
    let rom = match rom_path(rom.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("xtask probe: {e}");
            return 2;
        }
    };
    let track: Vec<u8> = match &movie {
        Some(m) => match load_movie_track(Path::new(m)) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("xtask probe: {e}");
                return 2;
            }
        },
        None => Vec::new(),
    };
    let mut oracle = match TetanesOracle::load_rom(&rom) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("xtask probe: {e}");
            return 2;
        }
    };
    let raw = std::fs::read(&rom).expect("rom readable");
    let mut game = GamePort(z2_core::game::Game::from_ines(&raw).expect("game"));
    game.0.reset();
    if traps {
        crate::verify::register_all_traps(&mut game.0);
    }
    for f in 0..frame {
        let input = track.get(f).copied().unwrap_or(0);
        oracle.step(input);
        game.step(input);
    }
    let input = track.get(frame).copied().unwrap_or(0);

    // ---- oracle: instruction walk over frame `frame`.
    println!("=== oracle frame {frame} (scanline,dot pc event) ===");
    let start_frame = oracle.ppu_frame_number();
    let mut prev = oracle.ppu_regs();
    let mut prev_pc = oracle.cpu_pc();
    let mut n = 0usize;
    let mut printed = 0usize;
    let mut in_spin = false;
    // Latch the frame's pad input the same way Port::step does (tetanes
    // joypad state persists across clock_instr calls).
    oracle.set_input(input);
    loop {
        if let Err(e) = oracle.clock_instr() {
            eprintln!("oracle: {e}");
            break;
        }
        n += 1;
        let r = oracle.ppu_regs();
        let pc = oracle.cpu_pc();
        let mut events: Vec<String> = Vec::new();
        if pc == 0xC07B || pc == 0xC060 {
            events.push("NMI entry".into());
        }
        if pcs.contains(&prev_pc) {
            events.push(format!("after instr @${prev_pc:04X}"));
        }
        if r.spr_zero_hit && !prev.spr_zero_hit {
            events.push("sprite0 hit flag set".into());
        }
        if !r.spr_zero_hit && prev.spr_zero_hit {
            events.push("sprite0 hit flag cleared".into());
        }
        if r.v != prev.v
            && (r.v & 0x7FFF) != prev.v.wrapping_add(1)
            && (r.v & 0x7FFF) != prev.v.wrapping_add(32)
        {
            events.push(format!("v ${:04X}->${:04X}", prev.v, r.v));
        }
        if r.t != prev.t {
            events.push(format!("t ${:04X}->${:04X}", prev.t, r.t));
        }
        if r.rendering_enabled != prev.rendering_enabled {
            events.push(format!("rendering {}", r.rendering_enabled));
        }
        if r.mirroring != prev.mirroring {
            events.push(format!("mirroring {}->{}", prev.mirroring, r.mirroring));
        }
        // Spin loop detection: BIT $2002 / BVC at $A73D/$A740, $D4B2/$D4B5, etc.
        let spin_now = matches!(pc, 0xA73D..=0xA741 | 0xD4B2..=0xD4B6 | 0xAB73..=0xAB77 | 0x9D59..=0x9D5D | 0x9DB0..=0x9DB4 | 0xA7B1..=0xA7B5 | 0xB08F..=0xB093);
        if spin_now && !in_spin {
            events.push(format!("enter sprite0 spin @${pc:04X}"));
        }
        if !spin_now && in_spin {
            events.push(format!("exit sprite0 spin -> ${pc:04X}"));
        }
        in_spin = spin_now;
        if !events.is_empty() && printed < max_events {
            println!(
                "  ({:3},{:3}) pc=${:04X} prev_pc=${:04X} cyc={} {}",
                r.scanline,
                r.cycle,
                pc,
                prev_pc,
                oracle.cpu_cycle(),
                events.join("; ")
            );
            printed += 1;
        }
        prev = r;
        prev_pc = pc;
        if oracle.ppu_frame_number() != start_frame {
            break;
        }
        if n > 200_000 {
            eprintln!("oracle: runaway");
            break;
        }
    }
    oracle.refresh_caches();
    println!("  ({} instructions)", n);

    // ---- game: PPU trace over frame `frame`.
    println!("=== game frame {frame} (line,dot event) ===");
    game.0.ppu.model_mut().set_trace(true);
    let cyc0 = game.0.cpu.cycles;
    game.step(input);
    let events = game.0.ppu.model_mut().take_trace();
    game.0.ppu.model_mut().set_trace(false);
    println!(
        "  frame origin={} step cycles {}..{} vblank hook cadence: see game.rs",
        game.0.ppu.frame_origin(),
        cyc0,
        game.0.cpu.cycles
    );
    let mut printed = 0usize;
    let mut last_status: Option<u8> = None;
    for e in &events {
        // Compress runs of identical $2002 polls.
        if let z2_ppu::PpuEventKind::Status(v) = e.kind {
            if last_status == Some(v) {
                continue;
            }
            last_status = Some(v);
        } else {
            last_status = None;
        }
        if printed < max_events {
            println!("  ({:3},{:3}) {:?}", e.line, e.dot, e.kind);
            printed += 1;
        }
    }
    println!("  ({} events)", events.len());
    if !lines.is_empty() {
        // Dry-run comparison: re-render chosen lines from the frame's start
        // `t` (fine Y stepped per line) with the whole-line renderer and
        // print them next to the pipeline's pixels and the oracle's.
        let start_v = events
            .iter()
            .find_map(|e| match e.kind {
                z2_ppu::PpuEventKind::FrameStart(v) => Some(v),
                _ => None,
            })
            .unwrap_or(0);
        println!("=== dry-run lines from FrameStart v=${start_v:04X} (x 232..255) ===");
        let mut model = game.0.ppu.model().clone();
        for &l in &lines {
            let mut v = start_v;
            for _ in 0..l {
                // inc_y equivalent via public API is not exposed; emulate.
                if v & 0x7000 != 0x7000 {
                    v += 0x1000;
                } else {
                    v &= !0x7000;
                    let mut y = (v >> 5) & 0x1F;
                    if y == 29 {
                        y = 0;
                        v ^= 0x0800;
                    } else if y == 31 {
                        y = 0;
                    } else {
                        y += 1;
                    }
                    v = (v & !0x03E0) | (y << 5);
                }
            }
            model.debug_set_v(v);
            let mut out = [0u8; 256];
            let _ = z2_ppu::render_line(&model, l, &mut out);
            let hex = |s: &[u8]| {
                s.iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let gf = game.frame_indexed();
            let of = oracle.frame_indexed();
            let base = usize::from(l) * 256;
            println!("  line {l:3} dry   : {}", hex(&out[232..256]));
            println!("  line {l:3} game  : {}", hex(&gf[base + 232..base + 256]));
            println!("  line {l:3} oracle: {}", hex(&of[base + 232..base + 256]));
        }
        let m = game.0.ppu.model();
        println!(
            "  mirroring={:?} ctrl=${:02X} mask=${:02X} t=${:04X} fine_x={}",
            m.mirroring(),
            m.ctrl(),
            m.mask(),
            m.t(),
            m.fine_x()
        );
        for base in [0x2000u16, 0x2800] {
            let row: Vec<String> = (26..32u16)
                .map(|c| format!("{:02X}", m.nt_read(base + c)))
                .collect();
            let attr = m.nt_read(base + 0x3C0 + 7);
            println!(
                "  NT ${base:04X} row0 cols26-31: {} attr(7)=${attr:02X}",
                row.join(" ")
            );
        }
    }
    0
}
