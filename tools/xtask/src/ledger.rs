//! Ledger: rebuild the vendored z2disassembly, verify the output
//! against the pinned USA ROM, and generate the routine ledger (`ports.toml`).
//!
//! Inputs (all derived from `third_party/z2disassembly`, never from a ROM):
//! - `src/prgN.asm` — label definitions, `.export`/`.import`, JSR/JMP and
//!   data references, `.segment` boundaries, VECTORS segment.
//! - `prgN.lst` (`ca65 -l`) — authoritative per-label file offsets, which
//!   plus the bank base ($8000 for banks 0-6, $C000 for bank 7) give the
//!   CPU address of every routine. Cross-checked against `nes.labels`.
//! - `nes.labels` (`ld65 -Ln`) — linked addresses for global (imported /
//!   exported) symbols; resolves cross-bank JSR/JMP edges.
//! - `nes.map` (`ld65 --mapfile`) — segment placement; confirms bank bases.
//!
//! ports.toml entry identity is `(bank, name)`: auto-generated `Lxxxx`
//! labels repeat across banks (same name, different routine), so the name
//! alone is NOT unique.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const PINNED_DISASM: &str = "https://github.com/FiendsOfTheElements/z2disassembly";
pub const GENERATOR: &str = "xtask ledger 1";

// ---------------------------------------------------------------------------
// tiny hashing (std only): CRC32 (IEEE) + SHA1, for the ROM hash gate.
// ---------------------------------------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bit_len = (msg.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut s = String::with_capacity(40);
    for v in h {
        s.push_str(&format!("{v:08x}"));
    }
    s
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Args {
    disasm: PathBuf,
    work: PathBuf,
    out: PathBuf,
    rebuild: bool,
    verify: bool,
    check: bool,
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut a = Args {
        disasm: PathBuf::from("third_party/z2disassembly"),
        work: PathBuf::from("target/z2disasm"),
        out: PathBuf::from("ports.toml"),
        ..Default::default()
    };
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--disasm" => {
                i += 1;
                a.disasm = PathBuf::from(raw.get(i).ok_or("--disasm needs a value")?);
            }
            "--work" => {
                i += 1;
                a.work = PathBuf::from(raw.get(i).ok_or("--work needs a value")?);
            }
            "--out" => {
                i += 1;
                a.out = PathBuf::from(raw.get(i).ok_or("--out needs a value")?);
            }
            "--rebuild" => a.rebuild = true,
            "--verify" => a.verify = true,
            "--check" => a.check = true,
            "--help" | "-h" => return Err("help".into()),
            other => return Err(format!("unknown ledger flag '{other}'")),
        }
        i += 1;
    }
    Ok(a)
}

pub fn usage() -> &'static str {
    "xtask ledger [--rebuild] [--verify] [--check] [--disasm DIR] [--work DIR] [--out FILE]\n\
     \x20   --rebuild    run ca65/ld65 (needs the cc65 toolchain) into --work, emitting\n\
     \x20                prgN.lst listings, nes.map and nes.labels\n\
     \x20   --verify     hash-gate the rebuild against $Z2_ROM (pinned USA ROM body:\n\
     \x20                CRC32 BA322865, SHA1 11333adb723a5975e0ecca3aee8f4747aa8d2d26)\n\
     \x20                and classify any diff (bank files are source of truth)\n\
     \x20   --check      regenerate in memory and fail if ports.toml is stale\n\
     \x20   (no flags)   regenerate ports.toml from --work artifacts (run --rebuild first)\n"
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Code,
    Data,
    JumpTable,
    Ambiguous,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Code => "code",
            Kind::Data => "data",
            Kind::JumpTable => "jump-table",
            Kind::Ambiguous => "ambiguous",
        }
    }
}

#[derive(Clone)]
struct Routine {
    name: String,
    bank: u8,
    addr: u32,
    size: Option<u32>,
    kind: Kind,
    status: String,
    callers: BTreeSet<String>,
    callees: BTreeSet<String>,
    source: String,
    review: bool,
    notes: String,
}

#[derive(Default)]
struct SymInfo {
    banks_defined: BTreeSet<u8>,
    def_line: BTreeMap<u8, usize>, // bank -> source line of `Name:`
    exported_by: BTreeSet<u8>,
    imported_by: BTreeSet<u8>,
    jsr_from: BTreeSet<(u8, String)>, // (caller bank, caller routine)
    jmp_from: BTreeSet<(u8, String)>,
    jmp_indirect_from: BTreeSet<(u8, String)>,
    data_from: BTreeSet<(u8, String)>,
    word_from: BTreeSet<(u8, String)>,
    vector_in: BTreeSet<u8>, // banks whose VECTORS segment references this
    body: BTreeMap<u8, (bool, bool)>, // bank -> (has_instr, has_data)
    listing_addr: BTreeMap<u8, u32>, // bank -> cpu addr from listing offset
    linked_addr: Option<u32>, // from nes.labels
    /// `Name = $addr` aliases: bank -> (value, source line). Used to ledger
    /// JSR/JMP targets that have no `Name:` definition (typically bank-7
    /// routines referenced from switchable banks by address).
    aliases: BTreeMap<u8, (u32, usize)>,
}

impl SymInfo {
    fn body(&self, bank: u8) -> (bool, bool) {
        self.body.get(&bank).copied().unwrap_or((false, false))
    }
    fn mark_instr(&mut self, bank: u8) {
        self.body.entry(bank).or_insert((false, false)).0 = true;
    }
    fn mark_data(&mut self, bank: u8) {
        self.body.entry(bank).or_insert((false, false)).1 = true;
    }
}

// ---------------------------------------------------------------------------
// asm helpers
// ---------------------------------------------------------------------------

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Strip a `;` comment, ignoring `;` inside double-quoted strings.
fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_str = !in_str,
            ';' if !in_str => return line[..i].trim_end(),
            _ => {}
        }
    }
    line.trim_end()
}

/// Identifier tokens in `operand`, excluding `$`-prefixed hex and 1-letter
/// register names (A/X/Y). Returns owned Strings.
fn operand_idents(operand: &str) -> Vec<String> {
    let bytes = operand.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if is_ident_start(bytes[i]) && (i == 0 || bytes[i - 1] != b'$') {
            let mut j = i + 1;
            while j < bytes.len() && is_ident_char(bytes[j]) {
                j += 1;
            }
            let tok = &operand[i..j];
            if tok.len() > 1 {
                out.push(tok.to_string());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

const DATA_MNEMONICS: &[&str] = &[
    "LDA", "LDX", "LDY", "STA", "STX", "STY", "CMP", "CPX", "CPY", "AND", "ORA", "EOR", "ADC",
    "SBC", "BIT", "INC", "DEC", "ASL", "LSR", "ROL", "ROR",
];

fn is_data_mnemonic(m: &str) -> bool {
    DATA_MNEMONICS.contains(&m)
}

fn is_instr_mnemonic(m: &str) -> bool {
    matches!(
        m,
        "LDA"
            | "LDX"
            | "LDY"
            | "STA"
            | "STX"
            | "STY"
            | "TAX"
            | "TAY"
            | "TXA"
            | "TYA"
            | "TSX"
            | "TXS"
            | "DEX"
            | "DEY"
            | "INX"
            | "INY"
            | "INC"
            | "DEC"
            | "ADC"
            | "SBC"
            | "CMP"
            | "CPX"
            | "CPY"
            | "AND"
            | "ORA"
            | "EOR"
            | "BIT"
            | "ASL"
            | "LSR"
            | "ROL"
            | "ROR"
            | "JMP"
            | "JSR"
            | "RTS"
            | "RTI"
            | "BRK"
            | "BCC"
            | "BCS"
            | "BEQ"
            | "BMI"
            | "BNE"
            | "BPL"
            | "BVC"
            | "BVS"
            | "CLC"
            | "CLD"
            | "CLI"
            | "CLV"
            | "SEC"
            | "SED"
            | "SEI"
            | "NOP"
            | "PHA"
            | "PHP"
            | "PLA"
            | "PLP"
    )
}

fn bank_base(bank: u8) -> u32 {
    if bank == 7 {
        0xC000
    } else {
        0x8000
    }
}

fn bank_end(bank: u8) -> u32 {
    if bank == 7 {
        0x10000
    } else {
        0xC000
    }
}

// ---------------------------------------------------------------------------
// Rebuild: ca65 + ld65
// ---------------------------------------------------------------------------

fn run_cmd(prog: &str, args: &[String], cwd: &Path) -> Result<(), String> {
    let status = std::process::Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|e| format!("cannot run {prog}: {e} (is the cc65 toolchain installed?)"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{prog} {} failed with {status}", args.join(" ")))
    }
}

fn rebuild(a: &Args) -> Result<(), String> {
    let src_dir = a.disasm.join("src");
    let inc_dir = a.disasm.join("inc");
    for f in ["nes.cfg", "src/prg0.asm", "inc/nes.asm"] {
        if !a.disasm.join(f).exists() {
            return Err(format!(
                "disassembly layout mismatch: {} missing under {} (expected FiendsOfTheElements/z2disassembly pins; see third_party/README.md)",
                f,
                a.disasm.display()
            ));
        }
    }
    let obj_dir = a.work.join("obj");
    let lst_dir = a.work.join("lst");
    let bin_dir = a.work.join("bin");
    let map_dir = a.work.join("map");
    for d in [&obj_dir, &lst_dir, &bin_dir, &map_dir] {
        std::fs::create_dir_all(d).map_err(|e| format!("mkdir {}: {e}", d.display()))?;
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut objs = Vec::new();
    for n in 0..8u8 {
        let obj = obj_dir.join(format!("prg{n}.o"));
        let lst = lst_dir.join(format!("prg{n}.lst"));
        run_cmd(
            "ca65",
            &[
                "-I".into(),
                inc_dir.to_string_lossy().into_owned(),
                "-l".into(),
                lst.to_string_lossy().into_owned(),
                "-o".into(),
                obj.to_string_lossy().into_owned(),
                src_dir
                    .join(format!("prg{n}.asm"))
                    .to_string_lossy()
                    .into_owned(),
            ],
            &cwd,
        )
        .map_err(|e| format!("ca65 prg{n}: {e}"))?;
        objs.push(obj.to_string_lossy().into_owned());
    }
    // NOTE: ld65 writes prgN.bin next to CWD, so link with CWD=bin_dir and
    // absolute -C/obj paths to keep the worktree clean.
    let abs = |p: PathBuf| cwd.join(&p).to_string_lossy().into_owned();
    let cfg = a.disasm.join("nes.cfg").to_string_lossy().into_owned();
    let mut link_args = vec![
        "-C".to_string(),
        std::fs::canonicalize(&cfg)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned(),
        "--mapfile".to_string(),
        abs(map_dir.join("nes.map")),
        "-Ln".to_string(),
        abs(map_dir.join("nes.labels")),
    ];
    for o in &objs {
        link_args.push(
            std::fs::canonicalize(o)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned(),
        );
    }
    run_cmd("ld65", &link_args, &bin_dir).map_err(|e| format!("ld65: {e}"))?;
    // Move ld65's CWD-side prgN.bin outputs into bin_dir (ld65 ignores -o
    // when MEMORY areas carry `file=`; nes.cfg does).
    for n in 0..8u8 {
        let stray = bin_dir.join(format!("prg{n}.bin"));
        if !stray.exists() {
            let alt = cwd.join(format!("prg{n}.bin"));
            if alt.exists() {
                std::fs::rename(&alt, &stray).map_err(|e| e.to_string())?;
            } else {
                return Err(format!("ld65 did not emit prg{n}.bin"));
            }
        }
    }
    println!(
        "rebuild ok: listings in {}, bins in {}",
        lst_dir.display(),
        bin_dir.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

struct AsmFile {
    bank: u8,
    path: String, // as recorded in ports.toml `source`, e.g. src/prg7.asm
    lines: Vec<String>,
}

fn load_sources(disasm: &Path) -> Result<Vec<AsmFile>, String> {
    let mut files = Vec::new();
    for n in 0..8u8 {
        let p = disasm.join("src").join(format!("prg{n}.asm"));
        let text = std::fs::read_to_string(&p).map_err(|e| format!("read {}: {e}", p.display()))?;
        files.push(AsmFile {
            bank: n,
            path: format!("src/prg{n}.asm"),
            lines: text.lines().map(|l| l.to_string()).collect(),
        });
    }
    Ok(files)
}

/// First pass: defs, exports, imports, refs, body evidence, vectors.
#[allow(clippy::too_many_arguments)]
fn scan_sources(
    files: &[AsmFile],
    syms: &mut BTreeMap<String, SymInfo>,
    warnings: &mut Vec<String>,
) {
    for f in files {
        let mut current: Option<String> = None;
        let mut in_vectors = false;
        for (idx, raw) in f.lines.iter().enumerate() {
            let line = strip_comment(raw);
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Some(rest) = t.strip_prefix(".segment") {
                in_vectors = rest.contains("VECTORS");
                current = None;
                continue;
            }
            if let Some(rest) = t.strip_prefix(".export") {
                for name in rest.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    syms.entry(name.to_string())
                        .or_default()
                        .exported_by
                        .insert(f.bank);
                }
                continue;
            }
            if let Some(rest) = t.strip_prefix(".import") {
                for name in rest.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    syms.entry(name.to_string())
                        .or_default()
                        .imported_by
                        .insert(f.bank);
                }
                continue;
            }
            // `Name:` label definition — must start at column 0 (da65 style).
            // This excludes indented `BEQ :+` branches to anonymous labels.
            if line.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
                let t = line.trim();
                if let Some(colon) = t.find(':') {
                    let head = t[..colon].trim();
                    if !head.is_empty()
                        && !head.contains(char::is_whitespace)
                        && head.bytes().all(is_ident_char)
                        && is_ident_start(head.as_bytes()[0])
                    {
                        let e = syms.entry(head.to_string()).or_default();
                        e.banks_defined.insert(f.bank);
                        e.def_line.entry(f.bank).or_insert(idx + 1);
                        current = Some(head.to_string());
                        continue;
                    }
                }
            }
            // `Name = $addr` alias at column 0 (not a ledger entry by itself,
            // but it can resolve JSR/JMP targets that lack a `Name:` def).
            if line.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
                if let Some(eq) = t.find('=') {
                    // exclude `==` (none in this codebase) and directive lines
                    let head = t[..eq].trim();
                    let tail = t[eq + 1..].trim();
                    if !head.is_empty()
                        && !head.contains(char::is_whitespace)
                        && head.bytes().all(is_ident_char)
                        && is_ident_start(head.as_bytes()[0])
                        && tail.starts_with('$')
                        && tail[1..].split_whitespace().next().is_some_and(|h| {
                            !h.is_empty() && h.bytes().all(|c| c.is_ascii_hexdigit())
                        })
                    {
                        let hex = tail[1..].split_whitespace().next().unwrap_or("");
                        if let Ok(v) = u32::from_str_radix(hex, 16) {
                            syms.entry(head.to_string())
                                .or_default()
                                .aliases
                                .entry(f.bank)
                                .or_insert((v, idx + 1));
                            continue;
                        }
                    }
                }
            }
            // Directives contributing body evidence / refs.
            let mut words = t.split_whitespace();
            let first = words.next().unwrap_or("");
            let up = first.to_ascii_uppercase();
            if first.starts_with('.') {
                let dir = up.as_str();
                if matches!(
                    dir,
                    ".BYT" | ".BYTE" | ".WORD" | ".ADDR" | ".DBYT" | ".RES" | ".INCBIN"
                ) {
                    if let Some(cur) = &current {
                        syms.entry(cur.clone()).or_default().mark_data(f.bank);
                    }
                    // `.word VEC` inside VECTORS marks interrupt vectors.
                    let operand = t[first.len()..].trim();
                    if in_vectors && matches!(dir, ".WORD" | ".ADDR") {
                        for id in operand_idents(operand) {
                            syms.entry(id).or_default().vector_in.insert(f.bank);
                        }
                    } else if matches!(dir, ".WORD" | ".ADDR") {
                        for id in operand_idents(operand) {
                            if let Some(cur) = &current {
                                syms.entry(id)
                                    .or_default()
                                    .word_from
                                    .insert((f.bank, cur.clone()));
                            }
                        }
                    } else if matches!(dir, ".BYT" | ".BYTE") {
                        for id in operand_idents(operand) {
                            if id.chars().any(|c| c.is_ascii_lowercase()) || id.len() > 2 {
                                if let Some(cur) = &current {
                                    syms.entry(id)
                                        .or_default()
                                        .data_from
                                        .insert((f.bank, cur.clone()));
                                }
                            }
                        }
                    }
                    continue;
                }
                continue;
            }
            if is_instr_mnemonic(&up) {
                if let Some(cur) = &current {
                    // instruction evidence belongs to this bank's routine
                    syms.entry(cur.clone()).or_default().mark_instr(f.bank);
                }
                let operand = t[first.len()..].trim();
                let target = operand_idents(operand).into_iter().next();
                match up.as_str() {
                    "JSR" => {
                        if let Some(sym) = target {
                            if !operand.trim_start().starts_with(['#', '$'])
                                && !operand
                                    .trim_start()
                                    .chars()
                                    .next()
                                    .is_some_and(|c| c.is_ascii_digit())
                            {
                                if let Some(cur) = &current {
                                    syms.entry(sym)
                                        .or_default()
                                        .jsr_from
                                        .insert((f.bank, cur.clone()));
                                }
                            }
                        }
                    }
                    "JMP" => {
                        if operand.trim_start().starts_with('(') {
                            if let Some(sym) = target {
                                if let Some(cur) = &current {
                                    syms.entry(sym)
                                        .or_default()
                                        .jmp_indirect_from
                                        .insert((f.bank, cur.clone()));
                                }
                            }
                        } else if let Some(sym) = target {
                            if !operand.trim_start().starts_with(['#', '$'])
                                && !operand
                                    .trim_start()
                                    .chars()
                                    .next()
                                    .is_some_and(|c| c.is_ascii_digit())
                            {
                                if let Some(cur) = &current {
                                    syms.entry(sym)
                                        .or_default()
                                        .jmp_from
                                        .insert((f.bank, cur.clone()));
                                }
                            }
                        }
                    }
                    _ if is_data_mnemonic(&up) => {
                        for id in operand_idents(operand) {
                            if let Some(cur) = &current {
                                syms.entry(id)
                                    .or_default()
                                    .data_from
                                    .insert((f.bank, cur.clone()));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let _ = warnings;
    }
}

fn parse_labels_file(path: &Path) -> Result<BTreeMap<String, u32>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let t = line.trim();
        // ld65 -Ln form: `al 00FF9D .ConfigureMMC1`
        let mut w = t.split_whitespace();
        if let (Some("al"), Some(addr), Some(name)) = (w.next(), w.next(), w.next()) {
            if let Ok(v) = u32::from_str_radix(addr, 16) {
                map.insert(name.trim_start_matches('.').to_string(), v);
            }
        }
    }
    Ok(map)
}

/// Parse `prgN.lst` label definitions: `^([0-9A-F]{6})(r| ) +\d+ +echo`.
/// Returns (name, cpu_addr) with cpu_addr = bank_base + file offset.
fn parse_listing(path: &Path, bank: u8) -> Result<Vec<(String, u32)>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut out = Vec::new();
    for line in text.lines() {
        let b = line.as_bytes();
        if b.len() < 9 || !b[0..6].iter().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        if b[6] != b'r' && b[6] != b' ' {
            continue;
        }
        let off = u32::from_str_radix(&line[0..6], 16).unwrap_or(0);
        // skip `r`, then whitespace + decimal source line number
        let mut i = 7;
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        let echo = line[i..].trim_start();
        // label def? `Name:` (leading ws tolerated; `@x:`/`:` excluded)
        if let Some(colon) = echo.find(':') {
            let head = echo[..colon].trim();
            if !head.is_empty()
                && !head.contains(char::is_whitespace)
                && head.bytes().all(is_ident_char)
                && is_ident_start(head.as_bytes()[0])
            {
                out.push((head.to_string(), bank_base(bank) + off));
            }
        }
    }
    Ok(out)
}

/// Parse map segment placement lines: `PRG7  00C000  00FFF9 ...`.
fn parse_map_bases(path: &Path) -> BTreeMap<u8, (u32, u32)> {
    let mut bases = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return bases;
    };
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("PRG") {
            let mut w = rest.split_whitespace();
            let parts: Vec<&str> = w.by_ref().take(3).collect();
            if parts.len() == 3 && parts[0].len() == 1 {
                if let Ok(bank) = parts[0].parse::<u8>() {
                    if let (Ok(s), Ok(e)) = (
                        u32::from_str_radix(parts[1], 16),
                        u32::from_str_radix(parts[2], 16),
                    ) {
                        bases.insert(bank, (s, e));
                    }
                }
            }
        }
    }
    bases
}

// ---------------------------------------------------------------------------
// ports.toml emit / read (minimal TOML subset, hand-rolled, std only)
// ---------------------------------------------------------------------------

fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn toml_str_list(items: &BTreeSet<String>) -> String {
    let v: Vec<String> = items
        .iter()
        .map(|s| format!("\"{}\"", toml_escape(s)))
        .collect();
    format!("[{}]", v.join(", "))
}

fn emit_ports(
    routines: &[Routine],
    pin: &str,
    policy_ported: [u32; 8],
    policy_verified: [u32; 8],
) -> String {
    let mut s = String::new();
    s.push_str("# ports.toml — routine ledger for the Zelda II reconstruction.\n");
    s.push_str("# Generated by `cargo xtask ledger`; do NOT hand-edit bank/addr/size.\n");
    s.push_str(
        "# To record progress, set status = \"ported\" | \"verified\" (ledger preserves it).\n\n",
    );
    s.push_str("[meta]\n");
    s.push_str(&format!("disassembly = \"{PINNED_DISASM}\"\n"));
    s.push_str(&format!("pin = \"{pin}\"\n"));
    s.push_str(&format!("generator = \"{GENERATOR}\"\n"));
    s.push_str("version = 1\n\n");
    s.push_str("[policy]\n");
    s.push_str("# Regression floors enforced by `cargo xtask coverage --check`.\n");
    s.push_str(&format!(
        "min_ported_per_bank = [{}]\n",
        policy_ported
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    s.push_str(&format!(
        "min_verified_per_bank = [{}]\n",
        policy_verified
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    s.push_str("\n# Schema per [[routine]]: name/bank/addr identify the routine\n");
    s.push_str("# (names repeat across banks — e.g. L80C9 — so (bank, name) is the key).\n");
    s.push_str("# kind = code|data|jump-table|ambiguous; status = unported|ported|verified.\n");
    s.push_str("# callers/callees = JSR/JMP-absolute edges (branches excluded: intra-routine).\n");
    s.push_str("# review = true flags hand-review (ambiguous class or listing mismatch).\n");
    for r in routines {
        s.push_str("\n[[routine]]\n");
        s.push_str(&format!("name = \"{}\"\n", toml_escape(&r.name)));
        s.push_str(&format!("bank = {}\n", r.bank));
        s.push_str(&format!("addr = 0x{:04X}\n", r.addr));
        match r.size {
            Some(v) => s.push_str(&format!("size = {v}\n")),
            None => s.push_str("size = -1\n"),
        }
        s.push_str(&format!("kind = \"{}\"\n", r.kind.as_str()));
        s.push_str(&format!("status = \"{}\"\n", toml_escape(&r.status)));
        s.push_str(&format!("callers = {}\n", toml_str_list(&r.callers)));
        s.push_str(&format!("callees = {}\n", toml_str_list(&r.callees)));
        s.push_str(&format!("source = \"{}\"\n", toml_escape(&r.source)));
        s.push_str(&format!("review = {}\n", r.review));
        if !r.notes.is_empty() {
            s.push_str(&format!("notes = \"{}\"\n", toml_escape(&r.notes)));
        }
    }
    s
}

#[derive(Default)]
struct ExistingPorts {
    status: BTreeMap<(u8, String), String>,
    notes: BTreeMap<(u8, String), String>,
    policy_ported: [u32; 8],
    policy_verified: [u32; 8],
    have_policy: bool,
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(c2) => out.push(c2),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_existing_ports(path: &Path) -> Result<ExistingPorts, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut ep = ExistingPorts::default();
    let mut cur_bank: Option<u8> = None;
    let mut cur_name: Option<String> = None;
    let mut in_policy = false;
    for line in text.lines() {
        let t = line.trim();
        if t == "[[routine]]" {
            if let (Some(b), Some(n)) = (cur_bank.take(), cur_name.take()) {
                let _ = (b, n);
            }
            in_policy = false;
            continue;
        }
        if t == "[policy]" {
            in_policy = true;
            continue;
        }
        if t.starts_with('[') {
            in_policy = false;
            cur_bank = None;
            cur_name = None;
            continue;
        }
        let Some(eq) = t.find('=') else { continue };
        let (k, v) = (t[..eq].trim(), t[eq + 1..].trim());
        if in_policy {
            if k == "min_ported_per_bank" {
                ep.have_policy = true;
                for (i, n) in v
                    .trim_matches(|c| c == '[' || c == ']')
                    .split(',')
                    .enumerate()
                    .take(8)
                {
                    ep.policy_ported[i] = n.trim().parse().unwrap_or(0);
                }
            } else if k == "min_verified_per_bank" {
                for (i, n) in v
                    .trim_matches(|c| c == '[' || c == ']')
                    .split(',')
                    .enumerate()
                    .take(8)
                {
                    ep.policy_verified[i] = n.trim().parse().unwrap_or(0);
                }
            }
            continue;
        }
        match k {
            "bank" => cur_bank = v.parse().ok(),
            "name" if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') => {
                cur_name = Some(unescape(&v[1..v.len() - 1]));
            }
            "name" => {}
            "status" => {
                if let (Some(b), Some(n)) = (cur_bank, cur_name.clone()) {
                    let val = v.trim_matches('"');
                    ep.status.insert((b, n), unescape(val));
                }
            }
            "notes" => {
                if let (Some(b), Some(n)) = (cur_bank, cur_name.clone()) {
                    let mut val = v.to_string();
                    if val.starts_with('"') {
                        val.remove(0);
                    }
                    if val.ends_with('"') {
                        val.pop();
                    }
                    ep.notes.insert((b, n), unescape(&val));
                }
            }
            _ => {}
        }
    }
    Ok(ep)
}

// ---------------------------------------------------------------------------
// Verify (hash gate + known-diff classification)
// ---------------------------------------------------------------------------

const PIN_CRC32: u32 = 0xBA32_2865;
const PIN_SHA1: &str = "11333adb723a5975e0ecca3aee8f4747aa8d2d26";
const BANK_LEN: usize = 16384;

/// Known single-byte head diffs: (bank, file_offset) where the reassembly
/// differs from the pinned ROM for a documented reason (Class B/C).
/// Class B: JSR $BFxx (local stub copy) vs $FFxx (bank7 copy) — same bytes.
/// Class C: anonymous-label collision (BPL `:-` binds nearer `:`).
const KNOWN_HEAD_DIFFS: [(u8, usize); 6] = [
    (0, 0x14d),
    (0, 0x152),
    (0, 0x286d),
    (5, 0x2716),
    (5, 0x272a),
    (7, 0x181),
];

fn verify_build(a: &Args) -> Result<(), String> {
    // An exported-but-empty Z2_ROM counts as "not set", not as a path.
    let rom_path = std::env::var("Z2_ROM")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| {
            "verify: $Z2_ROM is not set; point it at your own USA dump (read-only, never copied). Skipping hash gate.".to_string()
        });
    let rom_path = match rom_path {
        Ok(p) => p,
        Err(msg) => {
            println!("{msg}");
            return Ok(());
        }
    };
    let rom = std::fs::read(&rom_path).map_err(|e| format!("read $Z2_ROM: {e}"))?;
    if rom.len() < 16 || rom[..4] != *b"NES\x1a" {
        return Err("verify: $Z2_ROM is not an iNES ROM (bad magic)".to_string());
    }
    // Pinned hashes cover the FULL body (PRG + CHR) after the 16-byte header.
    let body = &rom[16..];
    let crc = crc32(body);
    let sha = sha1_hex(body);
    println!(
        "reference ROM: {} bytes, body CRC32 {crc:08X}, SHA1 {sha}",
        rom.len()
    );
    if crc != PIN_CRC32 || sha != PIN_SHA1 {
        return Err(
            "reference ROM is NOT the pinned USA dump (want body CRC32 BA322865 / SHA1 11333adb…d26)"
                .to_string(),
        );
    }
    if body.len() < 8 * BANK_LEN {
        return Err(format!(
            "reference ROM body too small ({} bytes)",
            body.len()
        ));
    }
    let mut unknown: Vec<String> = Vec::new();
    let mut known = 0u32;
    for n in 0..8u8 {
        let bin = std::fs::read(a.work.join("bin").join(format!("prg{n}.bin")))
            .map_err(|e| format!("read prg{n}.bin (run --rebuild first): {e}"))?;
        if bin.len() != BANK_LEN {
            return Err(format!("prg{n}.bin len {} != 16384", bin.len()));
        }
        let exp = &body[n as usize * BANK_LEN..(n as usize + 1) * BANK_LEN];
        for (i, (&e, &g)) in exp.iter().zip(bin.iter()).enumerate() {
            if e == g {
                continue;
            }
            let known_tail = n < 7 && i >= 0x3f70; // Class A: commented-out shared stub
            let known_head = KNOWN_HEAD_DIFFS.contains(&(n, i));
            if known_tail || known_head {
                known += 1;
            } else {
                unknown.push(format!(
                    "bank{n} +0x{i:04x}: rom {e:02x} vs rebuilt {g:02x}"
                ));
            }
        }
    }
    println!("known upstream diffs (documented, bank files are source of truth): {known} bytes");
    println!("  Class A: banks 0-6 tail $BF70-$BFFF stub commented out in .asm (7x121 B)");
    println!("  Class B: 5x JSR high byte $BF (local stub) vs $FF (bank7 copy, same bytes)");
    println!("  Class C: prg7 BPL anonymous-label collision ($C180: FD vs F5)");
    if unknown.is_empty() {
        println!("verify ok: no UNKNOWN diffs");
        Ok(())
    } else {
        println!("UNKNOWN diffs ({}):", unknown.len());
        for u in unknown.iter().take(20) {
            println!("  {u}");
        }
        Err(format!(
            "verify failed: {} unknown diff bytes",
            unknown.len()
        ))
    }
}

// ---------------------------------------------------------------------------
// Main entry
// ---------------------------------------------------------------------------

fn git_pin(disasm: &Path) -> String {
    std::process::Command::new("git")
        .args(["-C", &disasm.to_string_lossy(), "rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown (not a git checkout?)".to_string())
}

pub fn run(raw: &[String]) -> i32 {
    let args = match parse_args(raw) {
        Ok(a) => a,
        Err(e) => {
            if e == "help" {
                print!("{}", usage());
                return 0;
            }
            eprintln!("ledger: {e}\n\n{}", usage());
            return 2;
        }
    };

    if args.rebuild {
        if let Err(e) = rebuild(&args) {
            eprintln!("ledger --rebuild failed: {e}");
            return 3;
        }
    }
    if args.verify {
        if let Err(e) = verify_build(&args) {
            eprintln!("{e}");
            return 3;
        }
        // --verify is read-only (hash gate only); it never rewrites ports.toml.
        // Combine with --rebuild/--check to also regenerate or compare.
        if !args.check && !args.rebuild {
            return 0;
        }
    }

    match build_ledger(&args) {
        Ok((text, stats)) => {
            if args.check {
                match std::fs::read_to_string(&args.out) {
                    Ok(cur) => {
                        if normalize(&cur) == normalize(&text) {
                            println!(
                                "ledger --check ok: {} is up to date ({})",
                                args.out.display(),
                                stats
                            );
                            0
                        } else {
                            eprintln!(
                                "ledger --check FAILED: {} is stale vs {} — rerun `cargo xtask ledger{}`",
                                args.out.display(),
                                args.disasm.display(),
                                if args.rebuild { " --rebuild" } else { "" }
                            );
                            3
                        }
                    }
                    Err(_) => {
                        eprintln!(
                            "ledger --check FAILED: {} missing — run `cargo xtask ledger`",
                            args.out.display()
                        );
                        3
                    }
                }
            } else {
                match std::fs::write(&args.out, &text) {
                    Ok(()) => {
                        println!("wrote {} ({})", args.out.display(), stats);
                        0
                    }
                    Err(e) => {
                        eprintln!("ledger: write {}: {e}", args.out.display());
                        3
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("ledger failed: {e}");
            3
        }
    }
}

/// Compare ports.toml ignoring status/notes lines (progress markings).
fn normalize(s: &str) -> String {
    // For --check we want staleness vs the disassembly, but status/notes are
    // human-maintained progress. Drop those lines before comparing.
    s.lines()
        .filter(|l| {
            let t = l.trim();
            !(t.starts_with("status =") || t.starts_with("notes ="))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_ledger(args: &Args) -> Result<(String, String), String> {
    let files = load_sources(&args.disasm)?;
    let mut syms: BTreeMap<String, SymInfo> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();
    scan_sources(&files, &mut syms, &mut warnings);

    // name -> bank -> addr from listings (primary for intra-bank labels)
    for f in &files {
        let lst = args.work.join("lst").join(format!("prg{}.lst", f.bank));
        match parse_listing(&lst, f.bank) {
            Ok(defs) => {
                for (name, addr) in defs {
                    if let Some(e) = syms.get_mut(&name) {
                        // listing defs in THIS file belong to THIS bank; same
                        // name may exist in several banks (Lxxxx) — key by file.
                        if e.banks_defined.contains(&f.bank) {
                            e.listing_addr.insert(f.bank, addr);
                        }
                    }
                }
            }
            Err(e) => {
                warnings.push(format!(
                    "no listing for bank {} ({e}; run --rebuild)",
                    f.bank
                ));
            }
        }
    }

    // linked addresses for globals
    let labels_path = args.work.join("map").join("nes.labels");
    let linked = parse_labels_file(&labels_path).unwrap_or_else(|e| {
        warnings.push(format!("no nes.labels ({e}; run --rebuild)"));
        BTreeMap::new()
    });
    for (name, addr) in &linked {
        if let Some(e) = syms.get_mut(name) {
            e.linked_addr = Some(*addr);
        }
    }

    // map segment bases (informational cross-check)
    let bases = parse_map_bases(&args.work.join("map").join("nes.map"));
    for (bank, (s, _)) in &bases {
        let want = bank_base(*bank);
        if *s != want {
            warnings.push(format!(
                "map: PRG{bank} starts at ${s:04X}, want ${want:04X}"
            ));
        }
    }

    // global def-bank lookup for import attribution
    let mut def_bank: BTreeMap<String, BTreeSet<u8>> = BTreeMap::new();
    for (name, e) in &syms {
        if !e.banks_defined.is_empty() {
            def_bank.insert(name.clone(), e.banks_defined.clone());
        }
    }

    // resolve callee bank for an edge from bank `from`
    let resolve = |sym: &str, from: u8, imported: bool| -> Option<u8> {
        let banks = def_bank.get(sym)?;
        if !imported && banks.contains(&from) {
            return Some(from);
        }
        if banks.len() == 1 {
            return banks.iter().next().copied();
        }
        // ambiguous same-name defs: prefer caller's bank, else lowest
        if banks.contains(&from) {
            Some(from)
        } else {
            banks.iter().next().copied()
        }
    };

    // per-file import sets
    let mut imports: BTreeMap<u8, BTreeSet<String>> = BTreeMap::new();
    for (name, e) in &syms {
        for b in &e.imported_by {
            imports.entry(*b).or_default().insert(name.clone());
        }
    }

    // Alias targets: JSR/JMP symbols with no `Name:` def but with
    // `Name = $addr` aliases. Fixed-window ($C000+) aliases belong to bank 7
    // (always mapped); swap-window ($8000-$BFFF) aliases resolve in the
    // caller's bank; RAM/IO aliases (<$8000) get no entry (out of scope).
    // Returns (entry_banks, addr, conflict_note).
    let alias_entry = |e: &SymInfo| -> Option<(Vec<u8>, u32, bool)> {
        if e.aliases.is_empty() {
            return None;
        }
        let mut vals: BTreeSet<u32> = e.aliases.values().map(|(v, _)| *v).collect();
        if vals.len() != 1 {
            return None;
        }
        let v = vals.pop_first().unwrap();
        if !(0x8000..0x10000).contains(&v) {
            return None;
        }
        if v >= 0xC000 {
            Some((vec![7], v, false))
        } else {
            let banks: BTreeSet<u8> = e
                .jsr_from
                .iter()
                .chain(e.jmp_from.iter())
                .map(|(b, _)| *b)
                .collect();
            if banks.is_empty() {
                return None;
            }
            Some((banks.into_iter().collect(), v, true))
        }
    };

    // JSR/JMP/data/indirect refs, resolved to entry banks (imports =>
    // defining bank, otherwise the referrer's own bank; alias targets per
    // alias_entry). Keys are (entry_bank, target); values are
    // (referrer_bank, referrer_routine).
    let mut callers_of: BTreeMap<(u8, String), BTreeSet<(u8, String)>> = BTreeMap::new();
    let mut callees_of: BTreeMap<(u8, String), BTreeSet<(u8, String)>> = BTreeMap::new();
    let mut data_users_of: BTreeMap<(u8, String), BTreeSet<(u8, String)>> = BTreeMap::new();
    let mut word_users_of: BTreeMap<(u8, String), BTreeSet<(u8, String)>> = BTreeMap::new();
    let mut ind_users_of: BTreeMap<(u8, String), BTreeSet<(u8, String)>> = BTreeMap::new();
    // (bank, name) pairs that will become alias entries
    let mut alias_keys: BTreeSet<(u8, String)> = BTreeSet::new();
    for (name, e) in &syms {
        if e.banks_defined.is_empty() {
            if let Some((banks, _, _)) = alias_entry(e) {
                for b in banks {
                    alias_keys.insert((b, name.clone()));
                }
            }
        }
    }
    let mut edge_warnings = 0u32;
    // (ref-set, is_code_edge, per-kind map)
    let resolve_one = |name: &str, e: &SymInfo, from_bank: u8, imported: bool| -> Option<u8> {
        if let Some(to) = resolve(name, from_bank, imported) {
            return Some(to);
        }
        alias_entry(e).and_then(|(banks, _, _)| {
            if banks.contains(&7) {
                Some(7)
            } else if banks.contains(&from_bank) {
                Some(from_bank)
            } else {
                banks.into_iter().next()
            }
        })
    };
    for (name, e) in &syms {
        for set in [&e.jsr_from, &e.jmp_from] {
            for (from_bank, caller) in set {
                let imported = imports.get(from_bank).is_some_and(|s| s.contains(name));
                match resolve_one(name, e, *from_bank, imported) {
                    Some(to_bank) => {
                        if def_bank.contains_key(name)
                            || alias_keys.contains(&(to_bank, name.clone()))
                        {
                            callers_of
                                .entry((to_bank, name.clone()))
                                .or_default()
                                .insert((*from_bank, caller.clone()));
                            callees_of
                                .entry((*from_bank, caller.clone()))
                                .or_default()
                                .insert((to_bank, name.clone()));
                        } else {
                            edge_warnings += 1;
                        }
                    }
                    None => edge_warnings += 1,
                }
            }
        }
        for (set, map) in [
            (&e.data_from, &mut data_users_of),
            (&e.word_from, &mut word_users_of),
            (&e.jmp_indirect_from, &mut ind_users_of),
        ] {
            for (from_bank, user) in set {
                let imported = imports.get(from_bank).is_some_and(|s| s.contains(name));
                if let Some(to_bank) = resolve_one(name, e, *from_bank, imported) {
                    if def_bank.contains_key(name) || alias_keys.contains(&(to_bank, name.clone()))
                    {
                        map.entry((to_bank, name.clone()))
                            .or_default()
                            .insert((*from_bank, user.clone()));
                    }
                }
            }
        }
    }
    // .word refs also count as data refs for inclusion/classification
    for (k, v) in &word_users_of {
        data_users_of
            .entry(k.clone())
            .or_default()
            .extend(v.iter().cloned());
    }

    // candidate routines: per (bank, name) — same `Lxxxx` name in two banks
    // is two routines, and evidence must not leak across banks.
    // Included iff the bank's own evidence shows: exported, JSR/JMP target,
    // vector, data/.word reference target, or it contains JSR/JMP calls.
    let mut routines: Vec<Routine> = Vec::new();
    for (name, e) in &syms {
        if e.banks_defined.is_empty() {
            continue; // import-only / alias-only / constant (aliases below)
        }
        for &bank in &e.banks_defined {
            let key = (bank, name.clone());
            let is_export = e.exported_by.contains(&bank);
            let is_calltarget = callers_of.contains_key(&key);
            let is_vector = e.vector_in.contains(&bank);
            let is_dataref = data_users_of.contains_key(&key);
            let contains_calls = callees_of.contains_key(&key);
            if !(is_export || is_calltarget || is_vector || is_dataref || contains_calls) {
                continue; // unreferenced internal label — not ledger material
            }
            // address: linked (globals) else listing; cross-check when both
            let mut addr = e.linked_addr;
            let mut mismatch = false;
            if let Some(la) = e.listing_addr.get(&bank) {
                match addr {
                    Some(a) if a == *la => {}
                    Some(_) => mismatch = true,
                    None => addr = Some(*la),
                }
            }
            // sanity: addr must fall in the bank window
            if let Some(a) = addr {
                if a < bank_base(bank) || a >= bank_end(bank) {
                    // e.g. an imported symbol's linked addr viewed from a
                    // non-defining bank is meaningless — but we iterate only
                    // defining banks, so flag it.
                    mismatch = true;
                }
            }
            let Some(addr) = addr else {
                warnings.push(format!("no address for {name} (bank {bank})"));
                continue;
            };
            // classification, all evidence scoped to (bank, name)
            let targeted_indirect = ind_users_of.contains_key(&key);
            let word_refd = word_users_of.contains_key(&key);
            let (has_instr, has_data) = e.body(bank);
            let kind = if is_vector {
                if has_data && !has_instr {
                    Kind::Data
                } else {
                    Kind::Code
                }
            } else if targeted_indirect {
                Kind::JumpTable
            } else if is_calltarget {
                Kind::Code
            } else if has_instr {
                // contains code but is only data-referenced (e.g. a routine
                // whose bytes are also read as data)
                if is_dataref {
                    Kind::Data
                } else {
                    Kind::Code
                }
            } else if has_data || is_dataref {
                // all-.word/.addr body referenced as pointers -> jump table
                if has_data && !has_instr && word_refd {
                    Kind::JumpTable
                } else {
                    Kind::Data
                }
            } else if is_export || contains_calls {
                Kind::Code
            } else {
                Kind::Ambiguous
            };
            // callers/callees display: same-bank -> bare name;
            // cross-bank -> `name@bankN`
            let disp = |bn: (u8, &String)| {
                let (b2, n2) = bn;
                if b2 == bank {
                    n2.clone()
                } else {
                    format!("{n2}@bank{b2}")
                }
            };
            let callers: BTreeSet<String> = callers_of
                .get(&(bank, name.clone()))
                .map(|s| s.iter().map(|(b2, n2)| disp((*b2, n2))).collect())
                .unwrap_or_default();
            let callees: BTreeSet<String> = callees_of
                .get(&(bank, name.clone()))
                .map(|s| s.iter().map(|(b2, n2)| disp((*b2, n2))).collect())
                .unwrap_or_default();
            let review = mismatch || kind == Kind::Ambiguous;
            let mut notes = String::new();
            if mismatch {
                notes.push_str("listing/linked address mismatch; ");
            }
            if kind == Kind::Ambiguous {
                notes.push_str("ambiguous: no call/data refs; hand-review; ");
            }
            routines.push(Routine {
                name: name.clone(),
                bank,
                addr,
                size: None, // filled below
                kind,
                status: "unported".into(),
                callers,
                callees,
                source: format!(
                    "{}:{}",
                    files
                        .iter()
                        .find(|f| f.bank == bank)
                        .map(|f| f.path.as_str())
                        .unwrap_or("?"),
                    e.def_line.get(&bank).copied().unwrap_or(0)
                ),
                review,
                notes: notes.trim_end_matches("; ").to_string(),
            });
        }
    }

    // Alias entries: JSR/JMP targets with no `Name:` def (alternate /
    // unlabeled entry points, usually into bank 7). First-class routines
    // with review=true so a human confirms the boundary.
    for (name, e) in &syms {
        if !e.banks_defined.is_empty() {
            continue;
        }
        if e.jsr_from.is_empty() && e.jmp_from.is_empty() {
            continue;
        }
        let Some((banks, addr, swap_window)) = alias_entry(e) else {
            continue;
        };
        let alias_src = e
            .aliases
            .iter()
            .min_by_key(|(b, _)| *b)
            .map(|(b, (_, line))| {
                format!(
                    "{}:{line}",
                    files
                        .iter()
                        .find(|f| f.bank == *b)
                        .map(|f| f.path.as_str())
                        .unwrap_or("?")
                )
            })
            .unwrap_or_else(|| "?".to_string());
        for bank in banks {
            let disp = |bn: (u8, &String)| {
                let (b2, n2) = bn;
                if b2 == bank {
                    n2.clone()
                } else {
                    format!("{n2}@bank{b2}")
                }
            };
            let callers: BTreeSet<String> = callers_of
                .get(&(bank, name.clone()))
                .map(|s| s.iter().map(|(b2, n2)| disp((*b2, n2))).collect())
                .unwrap_or_default();
            let notes = if swap_window {
                "address-alias target in the swap window: entry bank depends on the runtime mapping; no `Label:` def"
            } else {
                "address-alias target, no `Label:` def in bank sources (alternate entry point?)"
            };
            routines.push(Routine {
                name: name.clone(),
                bank,
                addr,
                size: None,
                kind: Kind::Code,
                status: "unported".into(),
                callers,
                callees: BTreeSet::new(),
                source: alias_src.clone(),
                review: true,
                notes: notes.to_string(),
            });
        }
    }

    // sizes: next routine/data addr in the same bank, else bank end.
    // Build per-bank sorted addr list from ALL defined labels (not just
    // routines) so sizes abut correctly.
    let mut bank_addrs: BTreeMap<u8, Vec<u32>> = BTreeMap::new();
    for e in syms.values() {
        for &bank in &e.banks_defined {
            let a = e.linked_addr.or_else(|| e.listing_addr.get(&bank).copied());
            if let Some(a) = a {
                if a >= bank_base(bank) && a < bank_end(bank) {
                    bank_addrs.entry(bank).or_default().push(a);
                }
            }
        }
        // alias entry points also terminate the previous routine's range
        if e.banks_defined.is_empty() {
            if let Some((banks, addr, _)) = alias_entry(e) {
                for b in banks {
                    bank_addrs.entry(b).or_default().push(addr);
                }
            }
        }
    }
    for v in bank_addrs.values_mut() {
        v.sort_unstable();
        v.dedup();
    }
    for r in &mut routines {
        if let Some(v) = bank_addrs.get(&r.bank) {
            let next = v.iter().find(|&&a| a > r.addr);
            r.size = Some(next.copied().unwrap_or(bank_end(r.bank)) - r.addr);
        }
    }
    routines.sort_by_key(|r| (r.bank, r.addr));

    // preserve human-maintained status/notes
    let existing = std::fs::metadata(&args.out)
        .is_ok()
        .then(|| parse_existing_ports(&args.out))
        .transpose()?
        .unwrap_or_default();
    let (mut pol_p, mut pol_v) = ([0u32; 8], [0u32; 8]);
    if existing.have_policy {
        pol_p = existing.policy_ported;
        pol_v = existing.policy_verified;
    }
    for r in &mut routines {
        if let Some(s) = existing.status.get(&(r.bank, r.name.clone())) {
            if ["unported", "ported", "verified"].contains(&s.as_str()) {
                r.status = s.clone();
            }
        }
        if let Some(n) = existing.notes.get(&(r.bank, r.name.clone())) {
            if !n.is_empty() {
                r.notes = n.clone();
            }
        }
    }

    let pin = git_pin(&args.disasm);
    let text = emit_ports(&routines, &pin, pol_p, pol_v);

    let mut per_bank = [0u32; 8];
    let mut n_review = 0u32;
    for r in &routines {
        per_bank[r.bank as usize] += 1;
        n_review += r.review as u32;
    }
    let stats = format!(
        "{} routines ({}), review={} {}",
        routines.len(),
        per_bank
            .iter()
            .enumerate()
            .map(|(b, c)| format!("b{b}={c}"))
            .collect::<Vec<_>>()
            .join(" "),
        n_review,
        if warnings.is_empty() && edge_warnings == 0 {
            String::new()
        } else {
            format!("warnings={}", warnings.len() + edge_warnings as usize)
        }
    );
    for w in warnings.iter().take(10) {
        eprintln!("ledger warning: {w}");
    }
    if edge_warnings > 0 {
        eprintln!("ledger warning: {edge_warnings} edges to undefined symbols dropped");
    }
    Ok((text, stats))
}
