//! Dump composed PNGs for visual inspection (developer tool).
//!
//! ```text
//! cargo run -p z2-render --example dump_compose -- --out DIR [--rom PATH] [--frames N] [--scale N]
//! ```
//!
//! With a ROM it boots the game, steps `--frames` frames and writes the
//! composed image with no pack and with a synthetic pack that recolours a few
//! of the tiles actually drawn. Without a ROM it uses a synthetic scene.
//!
//! The output is ROM-derived when a ROM is used: write it outside the
//! repository (LEGAL.md §1). This example refuses an output directory inside a
//! git work tree.

use std::path::{Path, PathBuf};

use z2_render::fs::{enclosing_git_worktree, write_files};
use z2_render::presenter::{PresentConfig, Presenter};
use z2_render::{encode_png_rgba, HdPack, Recorder};

fn main() {
    let mut out: Option<PathBuf> = None;
    let mut rom: Option<String> = None;
    let mut frames = 240usize;
    let mut scale = 2u32;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = it.next().map(PathBuf::from),
            "--rom" => rom = it.next().cloned(),
            "--frames" => frames = it.next().and_then(|v| v.parse().ok()).unwrap_or(frames),
            "--scale" => scale = it.next().and_then(|v| v.parse().ok()).unwrap_or(scale),
            other => {
                eprintln!("dump_compose: unknown argument '{other}'");
                std::process::exit(2);
            }
        }
    }
    let Some(out) = out else {
        eprintln!("dump_compose: --out DIR is required");
        std::process::exit(2);
    };
    if let Some(root) = enclosing_git_worktree(&out) {
        eprintln!(
            "dump_compose: refusing to write into the git work tree at {} (LEGAL.md §1)",
            root.display()
        );
        std::process::exit(2);
    }
    // $Z2_ROM counts only when it names an existing file: an empty or stale
    // value must reach the "nothing to dump" exit below, not panic in read().
    let rom = rom.or_else(|| {
        std::env::var("Z2_ROM")
            .ok()
            .filter(|p| Path::new(p).is_file())
    });
    match rom {
        Some(path) => dump_rom(&out, Path::new(&path), frames, scale),
        None => {
            eprintln!("dump_compose: no --rom / $Z2_ROM; nothing to dump");
            std::process::exit(2);
        }
    }
}

fn dump_rom(out: &Path, rom: &Path, frames: usize, scale: u32) {
    let raw = std::fs::read(rom).expect("read ROM");
    let mut g = z2_core::game::Game::from_ines(&raw).expect("build Game");
    g.reset();
    z2_core::boot_traps::register_boot_traps(&mut g);
    g.ppu.model_mut().set_record(true);

    let mut rec = Recorder::new();
    for _ in 0..frames {
        g.step(0);
        rec.observe(g.ppu.model().frame_record().expect("record"));
    }
    let frame = *g.frame_indexed();
    let record = g.ppu.model().frame_record().unwrap().clone();

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut p = Presenter::new(PresentConfig {
        scale,
        ..PresentConfig::default()
    })
    .expect("presenter");
    let (w, h) = (p.width() as u32, p.height() as u32);
    let rgba = p.present(&frame, &record, &g.chr).expect("compose");
    files.push((
        format!("compose-nopack-{scale}x.png"),
        encode_png_rgba(w, h, rgba, &[]).unwrap(),
    ));

    // Synthetic pack: recolour the 12 most-drawn tiles bright magenta/cyan so
    // replacement placement is obvious, leaving everything else original.
    let mut keys: Vec<_> = rec.iter().map(|(k, s)| (s.count, *k)).collect();
    keys.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    keys.truncate(12);
    if let Some(pack) = synthetic_pack(&keys, scale) {
        p.set_pack(Some(pack)).expect("pack");
        let (w, h) = (p.width() as u32, p.height() as u32);
        let rgba = p.present(&frame, &record, &g.chr).expect("compose");
        files.push((
            format!("compose-pack-{scale}x.png"),
            encode_png_rgba(w, h, rgba, &[]).unwrap(),
        ));
    }

    // Widescreen with backdrop margins (no decoder wired here yet).
    p.set_pack(None).expect("clear pack");
    p.set_config(PresentConfig {
        scale,
        margin_tiles: 8,
        fill_left_clip: false,
        fill_right_clip: false,
    })
    .expect("wide");
    let (w, h) = (p.width() as u32, p.height() as u32);
    let rgba = p.present(&frame, &record, &g.chr).expect("compose wide");
    files.push((
        format!("compose-wide-{scale}x.png"),
        encode_png_rgba(w, h, rgba, &[]).unwrap(),
    ));

    let written = write_files(out, &files).expect("write");
    for path in written {
        println!("{}", path.display());
    }
    println!(
        "{} frames observed, {} distinct keys",
        rec.frames(),
        rec.distinct_keys()
    );
}

/// Page sheets recolouring just `keys`, as a loadable pack.
fn synthetic_pack(keys: &[(u64, z2_render::SeenKey)], scale: u32) -> Option<HdPack> {
    if keys.is_empty() {
        return None;
    }
    let cell = 8 * scale;
    let edge = 16 * cell;
    let mut pages: Vec<u8> = keys.iter().map(|(_, k)| k.page).collect();
    pages.sort_unstable();
    pages.dedup();
    let mut files = Vec::new();
    let mut entries = Vec::new();
    for page in pages {
        let mut rgba = vec![0u8; (edge * edge * 4) as usize];
        for (_, k) in keys.iter().filter(|(_, k)| k.page == page) {
            let (cx, cy) = (u32::from(k.tile % 16) * cell, u32::from(k.tile / 16) * cell);
            for y in 0..cell {
                for x in 0..cell {
                    // Checkerboard of two loud colours, with a transparent
                    // border so holes are visible against the NES art.
                    let border = x < scale || y < scale || x >= cell - scale || y >= cell - scale;
                    let px: [u8; 4] = if border {
                        [0, 0, 0, 0]
                    } else if ((x / scale) + (y / scale)).is_multiple_of(2) {
                        [255, 0, 255, 255]
                    } else {
                        [0, 255, 255, 255]
                    };
                    let i = (((cy + y) * edge + cx + x) * 4) as usize;
                    rgba[i..i + 4].copy_from_slice(&px);
                }
            }
        }
        let name = format!("page{page:02}.png");
        entries.push(format!("{{\"file\":\"{name}\",\"page\":{page}}}"));
        files.push((name, encode_png_rgba(edge, edge, &rgba, &[]).unwrap()));
    }
    let json = format!(
        "{{\"version\":1,\"name\":\"visual check\",\"scale\":{scale},\"sheets\":[{}]}}",
        entries.join(",")
    );
    let mut list = vec![("pack.json".to_string(), json.into_bytes())];
    list.extend(files);
    HdPack::from_files(&list).ok()
}
