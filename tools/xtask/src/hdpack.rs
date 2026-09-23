//! `cargo xtask hdpack`: HD graphics pack tooling.
//!
//! - `template`: render full-page template sheets + `pack.json` from the CHR
//!   of the user's (hash-gated) ROM into a directory outside the repository.
//! - `check`: load and validate a pack directory and print a summary.
//!
//! Template output is ROM-derived (LEGAL.md §1): the output directory must
//! not lie inside a git work tree or the z2rs workspace unless
//! `--force-in-repo` is given.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use z2_assets::extract_tables::section_id;
use z2_render::fs::{enclosing_git_worktree, load_pack_dir, resolve_path, write_files};
use z2_render::recorder::Recorder;
use z2_render::template::{template_from_chr, TemplateOptions};

const EXIT_OK: i32 = 0;
const EXIT_INVALID: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_ROM: i32 = 3;
const EXIT_IO: i32 = 4;

fn usage() -> &'static str {
    "cargo xtask hdpack template --out DIR [--rom PATH] [--scale N] [--pages all|0,5,10-12]\n\
     \x20                          [--colors 0F,16,30] [--name NAME] [--overwrite] [--force-in-repo]\n\
     cargo xtask hdpack record --movie M --out DIR [--rom PATH] [--scale N] [--frames N]\n\
     \x20                       [--name NAME] [--overwrite] [--force-in-repo]\n\
     cargo xtask hdpack check --pack DIR\n\
     \n\
     template  render one 16x16-cell sheet per CHR page plus pack.json from your ROM\n\
     \x20         (--rom or $Z2_ROM, hash-gated). The output is ROM-derived: DIR must be\n\
     \x20         outside the repository (LEGAL.md). --scale 1..8 (default 2); --pages\n\
     \x20         default all; --colors = three hex NES colour indices for sub-palette\n\
     \x20         entries 1..3 (default: grey ramp). Exit 0 ok, 1 render, 2 usage/refused,\n\
     \x20         3 ROM, 4 I/O.\n\
     record    replay a movie and write template sheets for exactly the (page, tile,\n\
     \x20         palette) combinations the run actually drew, in their real colours.\n\
     \x20         --frames 0 (default) replays the whole movie. Same output rules as\n\
     \x20         template.\n\
     check     load + validate a pack directory and print a summary (exit 1 if invalid).\n\
     \n\
     Artist workflow: README.md (HD graphics packs). pack.json format: crates/z2-render/src/pack.rs"
}

/// Entry point for `cargo xtask hdpack ...`.
pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("template") => template(&args[1..]),
        Some("record") => record(&args[1..]),
        Some("check") => check(&args[1..]),
        Some("help" | "-h" | "--help") => {
            println!("{}", usage());
            EXIT_OK
        }
        None => {
            eprintln!("{}", usage());
            EXIT_USAGE
        }
        Some(other) => {
            eprintln!("xtask hdpack: unknown command '{other}'\n\n{}", usage());
            EXIT_USAGE
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TemplateArgs {
    rom: Option<String>,
    out: PathBuf,
    scale: u32,
    pages: Vec<u8>,
    colors: Option<[u8; 3]>,
    name: String,
    overwrite: bool,
    force_in_repo: bool,
}

fn next_val(it: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, String> {
    it.next()
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn parse_pages(s: &str) -> Result<Vec<u8>, String> {
    if s == "all" {
        return Ok((0..32).collect());
    }
    let page = |p: &str| {
        p.trim()
            .parse::<u8>()
            .ok()
            .filter(|p| *p < 32)
            .ok_or_else(|| format!("--pages: '{p}' is not a CHR page 0..=31"))
    };
    let mut set = BTreeSet::new();
    for part in s.split(',').filter(|p| !p.trim().is_empty()) {
        if let Some((a, b)) = part.split_once('-') {
            let (a, b) = (page(a)?, page(b)?);
            if a > b {
                return Err(format!("--pages: empty range '{part}'"));
            }
            set.extend(a..=b);
        } else {
            set.insert(page(part)?);
        }
    }
    if set.is_empty() {
        return Err("--pages: no pages given".to_string());
    }
    Ok(set.into_iter().collect())
}

fn parse_colors(s: &str) -> Result<[u8; 3], String> {
    let vals: Vec<&str> = s.split(',').map(str::trim).collect();
    if vals.len() != 3 {
        return Err(format!(
            "--colors: expected three hex NES colours like 0F,16,30, got '{s}'"
        ));
    }
    let mut out = [0u8; 3];
    for (o, v) in out.iter_mut().zip(&vals) {
        let digits = v
            .strip_prefix('$')
            .or_else(|| v.strip_prefix("0x"))
            .unwrap_or(v);
        *o = u8::from_str_radix(digits, 16)
            .ok()
            .filter(|c| *c <= 0x3F)
            .ok_or_else(|| format!("--colors: '{v}' is not an NES colour 00..3F"))?;
    }
    Ok(out)
}

fn parse_template_args(args: &[String]) -> Result<TemplateArgs, String> {
    let mut rom = None;
    let mut out = None;
    let mut scale = 2;
    let mut pages: Vec<u8> = (0..32).collect();
    let mut colors = None;
    let mut name = "z2rs template".to_string();
    let (mut overwrite, mut force_in_repo) = (false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--rom" => rom = Some(next_val(&mut it, a)?),
            "--out" => out = Some(PathBuf::from(next_val(&mut it, a)?)),
            "--scale" => {
                let v = next_val(&mut it, a)?;
                scale = v
                    .parse()
                    .ok()
                    .filter(|s| (1..=z2_render::MAX_SCALE).contains(s))
                    .ok_or_else(|| format!("--scale must be 1..=8, got '{v}'"))?;
            }
            "--pages" => pages = parse_pages(&next_val(&mut it, a)?)?,
            "--colors" => colors = Some(parse_colors(&next_val(&mut it, a)?)?),
            "--name" => name = next_val(&mut it, a)?,
            "--overwrite" => overwrite = true,
            "--force-in-repo" => force_in_repo = true,
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    let out = out.ok_or(
        "--out DIR is required (template sheets are ROM-derived; pick a directory outside the repository)",
    )?;
    Ok(TemplateArgs {
        rom,
        out,
        scale,
        pages,
        colors,
        name,
        overwrite,
        force_in_repo,
    })
}

/// Why `out` must not receive ROM-derived files, if it lies in a repository.
fn repo_conflict(out: &Path) -> Option<String> {
    if let Some(root) = enclosing_git_worktree(out) {
        return Some(format!("the git work tree at {}", root.display()));
    }
    let ws = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (Ok(ws), Ok(abs)) = (ws.canonicalize(), resolve_path(out)) else {
        return None;
    };
    abs.starts_with(&ws)
        .then(|| format!("the z2rs workspace at {}", ws.display()))
}

fn template(args: &[String]) -> i32 {
    let a = match parse_template_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("xtask hdpack template: {e}\n\n{}", usage());
            return EXIT_USAGE;
        }
    };
    if let Some(code) = guard_output(&a.out, a.force_in_repo, a.overwrite, "template") {
        return code;
    }
    let rom_path = match crate::verify::rom_path(a.rom.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("xtask hdpack template: {e}");
            return EXIT_USAGE;
        }
    };
    let body = match z2_assets::rom::open_at(&rom_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("xtask hdpack template: {e}");
            return EXIT_ROM;
        }
    };
    let extracted = match z2_assets::extract::extract(&body) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("xtask hdpack template: {e}");
            return EXIT_ROM;
        }
    };
    let Some(chr) = extracted.get(section_id::CHR_ROM) else {
        eprintln!("xtask hdpack template: CHR_ROM section missing from the section table");
        return EXIT_ROM;
    };
    let opts = TemplateOptions {
        name: &a.name,
        scale: a.scale,
        pages: &a.pages,
        colors: a.colors,
    };
    let files = match template_from_chr(chr, &opts) {
        Ok(out) => out.into_files(),
        Err(e) => {
            eprintln!("xtask hdpack template: {e}");
            return EXIT_INVALID;
        }
    };
    match write_files(&a.out, &files) {
        Ok(paths) => {
            println!(
                "xtask hdpack template: wrote {} files to {}",
                paths.len(),
                a.out.display()
            );
            for (p, (_, bytes)) in paths.iter().zip(&files) {
                println!("  {} ({} bytes)", p.display(), bytes.len());
            }
            println!(
                "These sheets are ROM-derived: keep them out of the repository and share only \
                 cells you repainted.\nPaint over the cells in place, then validate with: \
                 cargo xtask hdpack check --pack {}",
                a.out.display()
            );
            EXIT_OK
        }
        Err(e) => {
            eprintln!("xtask hdpack template: writing {}: {e}", a.out.display());
            EXIT_IO
        }
    }
}

/// Shared legal guard for every writer of ROM-derived output: refuse a path
/// inside a repository, and refuse to clobber an artist's painted pack.
fn guard_output(out: &Path, force_in_repo: bool, overwrite: bool, cmd: &str) -> Option<i32> {
    if let Some(place) = repo_conflict(out) {
        if !force_in_repo {
            eprintln!(
                "xtask hdpack {cmd}: refusing to write into {} (inside {place}).\n\
                 Template sheets are rendered from your ROM's CHR graphics and must never be\n\
                 committed (LEGAL.md §1). Choose a directory outside the repository, e.g.\n\
                 --out ~/z2-art/my-pack, or pass --force-in-repo to override.",
                out.display()
            );
            return Some(EXIT_USAGE);
        }
        eprintln!(
            "xtask hdpack {cmd}: warning: --force-in-repo: writing ROM-derived files inside \
             {place}; do NOT commit them"
        );
    }
    if out.join("pack.json").exists() && !overwrite {
        eprintln!(
            "xtask hdpack {cmd}: {} already contains a pack.json; pass --overwrite to replace \
             it (painted sheets with the same names would be lost)",
            out.display()
        );
        return Some(EXIT_USAGE);
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
struct RecordArgs {
    rom: Option<String>,
    movie: PathBuf,
    out: PathBuf,
    scale: u32,
    frames: usize,
    name: String,
    overwrite: bool,
    force_in_repo: bool,
}

fn parse_record_args(args: &[String]) -> Result<RecordArgs, String> {
    let mut rom = None;
    let mut movie = None;
    let mut out = None;
    let mut scale = 2u32;
    let mut frames = 0usize;
    let mut name = "z2rs recording".to_string();
    let (mut overwrite, mut force_in_repo) = (false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--rom" => rom = Some(next_val(&mut it, a)?),
            "--movie" => movie = Some(PathBuf::from(next_val(&mut it, a)?)),
            "--out" => out = Some(PathBuf::from(next_val(&mut it, a)?)),
            "--scale" => {
                let v = next_val(&mut it, a)?;
                scale = v
                    .parse()
                    .ok()
                    .filter(|s| (1..=z2_render::MAX_SCALE).contains(s))
                    .ok_or_else(|| format!("--scale must be 1..=8, got '{v}'"))?;
            }
            "--frames" => {
                let v = next_val(&mut it, a)?;
                frames = v
                    .parse()
                    .map_err(|_| format!("--frames must be a frame count, got '{v}'"))?;
            }
            "--name" => name = next_val(&mut it, a)?,
            "--overwrite" => overwrite = true,
            "--force-in-repo" => force_in_repo = true,
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(RecordArgs {
        rom,
        movie: movie.ok_or("--movie PATH is required")?,
        out: out.ok_or(
            "--out DIR is required (recorded sheets are ROM-derived; pick a directory outside the repository)",
        )?,
        scale,
        frames,
        name,
        overwrite,
        force_in_repo,
    })
}

fn record(args: &[String]) -> i32 {
    let a = match parse_record_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("xtask hdpack record: {e}\n\n{}", usage());
            return EXIT_USAGE;
        }
    };
    if let Some(code) = guard_output(&a.out, a.force_in_repo, a.overwrite, "record") {
        return code;
    }
    let rom_path = match crate::verify::rom_path(a.rom.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("xtask hdpack record: {e}");
            return EXIT_USAGE;
        }
    };
    // Hash gate first, then re-read the full iNES image for `from_ines`.
    if let Err(e) = z2_assets::rom::open_at(&rom_path) {
        eprintln!("xtask hdpack record: {e}");
        return EXIT_ROM;
    }
    let raw = match std::fs::read(&rom_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("xtask hdpack record: read {}: {e}", rom_path.display());
            return EXIT_IO;
        }
    };
    let track = match crate::verify::load_movie_track(&a.movie) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("xtask hdpack record: {e}");
            return EXIT_USAGE;
        }
    };
    let mut game = match z2_core::game::Game::from_ines(&raw) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("xtask hdpack record: build Game: {e:?}");
            return EXIT_ROM;
        }
    };
    game.reset();
    crate::verify::register_all_traps(&mut game);
    game.ppu.model_mut().set_record(true);

    let n = if a.frames == 0 {
        track.len()
    } else {
        a.frames.min(track.len())
    };
    let mut rec = Recorder::new();
    for &pad in track.iter().take(n) {
        game.step(pad);
        if let Some(fr) = game.ppu.model().frame_record() {
            rec.observe(fr);
        }
    }
    println!(
        "xtask hdpack record: {} frames replayed, {} distinct tiles, {} tile/palette combinations",
        rec.frames(),
        rec.distinct_tiles(),
        rec.distinct_keys()
    );
    let files = match rec.write_pack(&game.chr, a.scale, &a.name) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("xtask hdpack record: {e}");
            return EXIT_INVALID;
        }
    };
    match write_files(&a.out, &files) {
        Ok(paths) => {
            println!("wrote {} files to {}", paths.len(), a.out.display());
            for (p, (_, bytes)) in paths.iter().zip(&files) {
                println!("  {} ({} bytes)", p.display(), bytes.len());
            }
            println!(
                "These sheets are ROM-derived: keep them out of the repository and share only \
                 cells you repainted.\nValidate with: cargo xtask hdpack check --pack {}",
                a.out.display()
            );
            EXIT_OK
        }
        Err(e) => {
            eprintln!("xtask hdpack record: writing {}: {e}", a.out.display());
            EXIT_IO
        }
    }
}

fn check(args: &[String]) -> i32 {
    let mut dir = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--pack" => match next_val(&mut it, a) {
                Ok(v) => dir = Some(PathBuf::from(v)),
                Err(e) => {
                    eprintln!("xtask hdpack check: {e}");
                    return EXIT_USAGE;
                }
            },
            other => {
                eprintln!(
                    "xtask hdpack check: unknown argument '{other}'\n\n{}",
                    usage()
                );
                return EXIT_USAGE;
            }
        }
    }
    let Some(dir) = dir else {
        eprintln!("xtask hdpack check: --pack DIR is required\n\n{}", usage());
        return EXIT_USAGE;
    };
    let pack = match load_pack_dir(&dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("xtask hdpack check: {}: {e}", dir.display());
            return EXIT_INVALID;
        }
    };
    let author = pack
        .author()
        .map(|a| format!(" by {a}"))
        .unwrap_or_default();
    println!("pack \"{}\"{author}: OK", pack.name());
    println!(
        "  scale {} ({c}x{c} px cells), alpha threshold {}",
        pack.scale(),
        pack.alpha_threshold(),
        c = pack.cell_size()
    );
    println!(
        "  master palette: {}",
        if pack.palette().is_some() {
            "custom"
        } else {
            "default NES"
        }
    );
    println!("  {} sheet image(s):", pack.sheets().len());
    for (i, s) in pack.sheets().iter().enumerate() {
        println!(
            "    {} ({}x{})",
            pack.sheet_file(i).unwrap_or("?"),
            s.width,
            s.height
        );
    }
    println!(
        "  {} tile(s) replaced for any palette, {} palette-specific variant(s)",
        pack.tile_count(),
        pack.variant_count()
    );
    for page in 0..32u8 {
        let (d, v) = pack.page_counts(page);
        if d + v > 0 {
            println!("    page {page:2}: {d:3} default, {v:3} variant");
        }
    }
    if pack.sprite_alpha_art() {
        println!(
            "  sprite_alpha \"art\": sprite cells keep their own shape (largest vertical bleed {} px)",
            pack.max_bleed_v()
        );
    }
    if !pack.layers().is_empty() {
        println!("  {} layer(s), drawn in this order:", pack.layers().len());
    }
    for layer in pack.layers() {
        let any = "any".to_string();
        let when = |v: Option<u8>| v.map_or(any.clone(), |v| v.to_string());
        let sheet = usize::from(layer.sheet);
        println!(
            "    {} ({}x{}): {:?} at ({}, {}), scroll {}%{}{}; world {}, region {}, scene {}",
            pack.sheet_file(sheet).unwrap_or("?"),
            pack.sheets()[sheet].width,
            pack.sheets()[sheet].height,
            layer.depth,
            layer.x,
            layer.y,
            layer.scroll,
            if layer.repeat_x { ", repeats" } else { "" },
            if layer.over_tiles.is_empty() {
                String::new()
            } else {
                format!(", over {} tile(s)", layer.over_tiles.len())
            },
            when(layer.world),
            when(layer.region),
            when(layer.scene)
        );
    }
    EXIT_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn refuses_output_inside_repo() {
        let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("hdpack-test-refused-output");
        let code = run(&s(&[
            "template",
            "--rom",
            "/nonexistent/zelda2.nes",
            "--out",
            out.to_str().unwrap(),
        ]));
        // EXIT_ROM would mean the guard was skipped and the ROM was opened.
        assert_eq!(code, EXIT_USAGE);
        assert!(!out.exists());
        assert!(repo_conflict(&out.join("deeper/../nested")).is_some());
    }

    #[test]
    fn page_and_colour_parsing() {
        assert_eq!(parse_pages("all").unwrap().len(), 32);
        assert_eq!(parse_pages("5,0,2-3,5").unwrap(), [0, 2, 3, 5]);
        assert!(parse_pages("32").is_err());
        assert!(parse_pages("4-2").is_err());
        assert!(parse_pages(",").is_err());
        assert_eq!(parse_colors("0F,$16,0x30").unwrap(), [0x0F, 0x16, 0x30]);
        assert!(parse_colors("40,00,00").is_err());
        assert!(parse_colors("0F,16").is_err());
    }

    #[test]
    fn template_args_defaults_and_errors() {
        let a = parse_template_args(&s(&["--out", "/tmp/x"])).unwrap();
        assert_eq!((a.scale, a.pages.len(), a.colors), (2, 32, None));
        assert!(parse_template_args(&s(&[])).is_err());
        assert!(parse_template_args(&s(&["--out", "/tmp/x", "--scale", "9"])).is_err());
        assert!(parse_template_args(&s(&["--out"])).is_err());
        assert_eq!(run(&s(&["template"])), EXIT_USAGE);
        assert_eq!(run(&s(&["bogus"])), EXIT_USAGE);
    }

    #[test]
    fn check_reports_invalid_pack() {
        assert_eq!(
            run(&s(&["check", "--pack", "/nonexistent/z2rs-pack"])),
            EXIT_INVALID
        );
        assert_eq!(run(&s(&["check"])), EXIT_USAGE);
    }
}
