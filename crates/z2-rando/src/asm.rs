//! A small two-pass 6502 assembler for the randomizer's own patches.
//!
//! Patches are written as readable source strings and assembled here, so
//! the crate never carries opaque byte blobs. Only the 151 official opcodes
//! are accepted.
//!
//! # Syntax
//!
//! ```text
//! ; comment
//! LIVES = $0700           ; constant (also `.define`-style via Assembler::define)
//! .org $C358              ; set the program counter (starts a new chunk)
//! start:  LDA #3          ; label + instruction
//!         STA LIVES       ; absolute (a known value < $100 picks zero page)
//!         LDA a:$0010     ; `a:` forces absolute addressing
//!         LDA table,Y
//!         LDA ($00),Y
//!         JMP (vector)
//!         BNE start       ; relative branches, range-checked
//!         ASL             ; accumulator (also `ASL A`)
//! table:  .byte 1, $02, %11, 'A', <start, >start
//!         .word start, table+2
//!         .res 4, $EA      ; 4 bytes of $EA (fill defaults to 0)
//! ```
//!
//! Expressions are numbers (`$hex`, `%binary`, decimal, `'c'`), symbols and
//! `*` (the current address), joined with `+` and `-`, with an optional
//! leading `<` (low byte) or `>` (high byte). Mnemonics, registers and
//! directives are case-insensitive; symbols are case-sensitive.
//!
//! Forward references work: a symbol that is unknown in the first pass is
//! sized as absolute, and the second pass keeps that size.

use std::collections::BTreeMap;
use std::fmt;

/// Addressing modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    /// No operand.
    Implied,
    /// Operates on A (`ASL`, `ASL A`).
    Accumulator,
    /// `#value`.
    Immediate,
    /// Zero page.
    ZeroPage,
    /// Zero page, X.
    ZeroPageX,
    /// Zero page, Y.
    ZeroPageY,
    /// Absolute.
    Absolute,
    /// Absolute, X.
    AbsoluteX,
    /// Absolute, Y.
    AbsoluteY,
    /// `(addr)` (JMP only).
    Indirect,
    /// `(zp,X)`.
    IndirectX,
    /// `(zp),Y`.
    IndirectY,
    /// Branch target.
    Relative,
}

impl Mode {
    /// Instruction length in bytes for this mode (1-3).
    #[must_use]
    #[allow(clippy::len_without_is_empty)] // an instruction is never empty
    pub fn len(self) -> u8 {
        match self {
            Mode::Implied | Mode::Accumulator => 1,
            Mode::Immediate
            | Mode::ZeroPage
            | Mode::ZeroPageX
            | Mode::ZeroPageY
            | Mode::IndirectX
            | Mode::IndirectY
            | Mode::Relative => 2,
            Mode::Absolute | Mode::AbsoluteX | Mode::AbsoluteY | Mode::Indirect => 3,
        }
    }
}

use Mode::*;

/// Every official `(mnemonic, mode, opcode)`.
pub const OPCODES: &[(&str, Mode, u8)] = &[
    ("ADC", Immediate, 0x69),
    ("ADC", ZeroPage, 0x65),
    ("ADC", ZeroPageX, 0x75),
    ("ADC", Absolute, 0x6D),
    ("ADC", AbsoluteX, 0x7D),
    ("ADC", AbsoluteY, 0x79),
    ("ADC", IndirectX, 0x61),
    ("ADC", IndirectY, 0x71),
    ("AND", Immediate, 0x29),
    ("AND", ZeroPage, 0x25),
    ("AND", ZeroPageX, 0x35),
    ("AND", Absolute, 0x2D),
    ("AND", AbsoluteX, 0x3D),
    ("AND", AbsoluteY, 0x39),
    ("AND", IndirectX, 0x21),
    ("AND", IndirectY, 0x31),
    ("ASL", Accumulator, 0x0A),
    ("ASL", ZeroPage, 0x06),
    ("ASL", ZeroPageX, 0x16),
    ("ASL", Absolute, 0x0E),
    ("ASL", AbsoluteX, 0x1E),
    ("BCC", Relative, 0x90),
    ("BCS", Relative, 0xB0),
    ("BEQ", Relative, 0xF0),
    ("BIT", ZeroPage, 0x24),
    ("BIT", Absolute, 0x2C),
    ("BMI", Relative, 0x30),
    ("BNE", Relative, 0xD0),
    ("BPL", Relative, 0x10),
    ("BRK", Implied, 0x00),
    ("BVC", Relative, 0x50),
    ("BVS", Relative, 0x70),
    ("CLC", Implied, 0x18),
    ("CLD", Implied, 0xD8),
    ("CLI", Implied, 0x58),
    ("CLV", Implied, 0xB8),
    ("CMP", Immediate, 0xC9),
    ("CMP", ZeroPage, 0xC5),
    ("CMP", ZeroPageX, 0xD5),
    ("CMP", Absolute, 0xCD),
    ("CMP", AbsoluteX, 0xDD),
    ("CMP", AbsoluteY, 0xD9),
    ("CMP", IndirectX, 0xC1),
    ("CMP", IndirectY, 0xD1),
    ("CPX", Immediate, 0xE0),
    ("CPX", ZeroPage, 0xE4),
    ("CPX", Absolute, 0xEC),
    ("CPY", Immediate, 0xC0),
    ("CPY", ZeroPage, 0xC4),
    ("CPY", Absolute, 0xCC),
    ("DEC", ZeroPage, 0xC6),
    ("DEC", ZeroPageX, 0xD6),
    ("DEC", Absolute, 0xCE),
    ("DEC", AbsoluteX, 0xDE),
    ("DEX", Implied, 0xCA),
    ("DEY", Implied, 0x88),
    ("EOR", Immediate, 0x49),
    ("EOR", ZeroPage, 0x45),
    ("EOR", ZeroPageX, 0x55),
    ("EOR", Absolute, 0x4D),
    ("EOR", AbsoluteX, 0x5D),
    ("EOR", AbsoluteY, 0x59),
    ("EOR", IndirectX, 0x41),
    ("EOR", IndirectY, 0x51),
    ("INC", ZeroPage, 0xE6),
    ("INC", ZeroPageX, 0xF6),
    ("INC", Absolute, 0xEE),
    ("INC", AbsoluteX, 0xFE),
    ("INX", Implied, 0xE8),
    ("INY", Implied, 0xC8),
    ("JMP", Absolute, 0x4C),
    ("JMP", Indirect, 0x6C),
    ("JSR", Absolute, 0x20),
    ("LDA", Immediate, 0xA9),
    ("LDA", ZeroPage, 0xA5),
    ("LDA", ZeroPageX, 0xB5),
    ("LDA", Absolute, 0xAD),
    ("LDA", AbsoluteX, 0xBD),
    ("LDA", AbsoluteY, 0xB9),
    ("LDA", IndirectX, 0xA1),
    ("LDA", IndirectY, 0xB1),
    ("LDX", Immediate, 0xA2),
    ("LDX", ZeroPage, 0xA6),
    ("LDX", ZeroPageY, 0xB6),
    ("LDX", Absolute, 0xAE),
    ("LDX", AbsoluteY, 0xBE),
    ("LDY", Immediate, 0xA0),
    ("LDY", ZeroPage, 0xA4),
    ("LDY", ZeroPageX, 0xB4),
    ("LDY", Absolute, 0xAC),
    ("LDY", AbsoluteX, 0xBC),
    ("LSR", Accumulator, 0x4A),
    ("LSR", ZeroPage, 0x46),
    ("LSR", ZeroPageX, 0x56),
    ("LSR", Absolute, 0x4E),
    ("LSR", AbsoluteX, 0x5E),
    ("NOP", Implied, 0xEA),
    ("ORA", Immediate, 0x09),
    ("ORA", ZeroPage, 0x05),
    ("ORA", ZeroPageX, 0x15),
    ("ORA", Absolute, 0x0D),
    ("ORA", AbsoluteX, 0x1D),
    ("ORA", AbsoluteY, 0x19),
    ("ORA", IndirectX, 0x01),
    ("ORA", IndirectY, 0x11),
    ("PHA", Implied, 0x48),
    ("PHP", Implied, 0x08),
    ("PLA", Implied, 0x68),
    ("PLP", Implied, 0x28),
    ("ROL", Accumulator, 0x2A),
    ("ROL", ZeroPage, 0x26),
    ("ROL", ZeroPageX, 0x36),
    ("ROL", Absolute, 0x2E),
    ("ROL", AbsoluteX, 0x3E),
    ("ROR", Accumulator, 0x6A),
    ("ROR", ZeroPage, 0x66),
    ("ROR", ZeroPageX, 0x76),
    ("ROR", Absolute, 0x6E),
    ("ROR", AbsoluteX, 0x7E),
    ("RTI", Implied, 0x40),
    ("RTS", Implied, 0x60),
    ("SBC", Immediate, 0xE9),
    ("SBC", ZeroPage, 0xE5),
    ("SBC", ZeroPageX, 0xF5),
    ("SBC", Absolute, 0xED),
    ("SBC", AbsoluteX, 0xFD),
    ("SBC", AbsoluteY, 0xF9),
    ("SBC", IndirectX, 0xE1),
    ("SBC", IndirectY, 0xF1),
    ("SEC", Implied, 0x38),
    ("SED", Implied, 0xF8),
    ("SEI", Implied, 0x78),
    ("STA", ZeroPage, 0x85),
    ("STA", ZeroPageX, 0x95),
    ("STA", Absolute, 0x8D),
    ("STA", AbsoluteX, 0x9D),
    ("STA", AbsoluteY, 0x99),
    ("STA", IndirectX, 0x81),
    ("STA", IndirectY, 0x91),
    ("STX", ZeroPage, 0x86),
    ("STX", ZeroPageY, 0x96),
    ("STX", Absolute, 0x8E),
    ("STY", ZeroPage, 0x84),
    ("STY", ZeroPageX, 0x94),
    ("STY", Absolute, 0x8C),
    ("TAX", Implied, 0xAA),
    ("TAY", Implied, 0xA8),
    ("TSX", Implied, 0xBA),
    ("TXA", Implied, 0x8A),
    ("TXS", Implied, 0x9A),
    ("TYA", Implied, 0x98),
];

fn opcode(mn: &str, mode: Mode) -> Option<u8> {
    OPCODES
        .iter()
        .find(|(m, md, _)| *m == mn && *md == mode)
        .map(|(_, _, op)| *op)
}

fn has_mode(mn: &str, mode: Mode) -> bool {
    opcode(mn, mode).is_some()
}

fn is_mnemonic(mn: &str) -> bool {
    OPCODES.iter().any(|(m, _, _)| *m == mn)
}

/// Decoded official opcode: `(mnemonic, mode)`, or `None` for an unofficial
/// opcode.
#[must_use]
pub fn decode(op: u8) -> Option<(&'static str, Mode)> {
    OPCODES
        .iter()
        .find(|(_, _, o)| *o == op)
        .map(|(m, md, _)| (*m, *md))
}

/// Instruction length of an official opcode (1-3), `None` for unofficial.
#[must_use]
pub fn opcode_len(op: u8) -> Option<u8> {
    decode(op).map(|(_, m)| m.len())
}

/// Whether `op` never falls through to the next instruction (`RTS`, `RTI`,
/// `JMP`).
#[must_use]
pub fn ends_flow(op: u8) -> bool {
    matches!(op, 0x60 | 0x40 | 0x4C | 0x6C)
}

/// Assembly failure with the 1-based source line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsmError {
    /// 1-based line number (0 when not tied to a line).
    pub line: usize,
    /// What went wrong.
    pub msg: String,
}

impl fmt::Display for AsmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "asm line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for AsmError {}

/// Assembly output: one chunk per `.org`, plus the symbol table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Assembled {
    /// `(start address, bytes)` in source order.
    pub chunks: Vec<(u16, Vec<u8>)>,
    /// Every label and constant.
    pub symbols: BTreeMap<String, u16>,
}

impl Assembled {
    /// All bytes concatenated (handy when there is a single chunk).
    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        self.chunks.iter().flat_map(|(_, b)| b.clone()).collect()
    }

    /// Address of a symbol.
    #[must_use]
    pub fn symbol(&self, name: &str) -> Option<u16> {
        self.symbols.get(name).copied()
    }
}

/// Assembler with optional predefined symbols.
#[derive(Debug, Clone, Default)]
pub struct Assembler {
    predefined: BTreeMap<String, u16>,
}

/// Assemble `src` with no predefined symbols. Code before the first `.org`
/// starts at `$0000` unless `src` sets one.
pub fn assemble(src: &str) -> Result<Assembled, AsmError> {
    Assembler::new().assemble(src)
}

/// Assemble `src` starting at `org` and return the flat bytes.
pub fn assemble_at(org: u16, src: &str) -> Result<Vec<u8>, AsmError> {
    let mut a = Assembler::new();
    a.define("__org", org);
    let out = a.assemble(&format!(".org __org\n{src}"))?;
    Ok(out.bytes())
}

#[derive(Debug, Clone)]
enum Item {
    Org(String),
    Label(String),
    Const(String, String),
    Instr {
        mn: &'static str,
        mode_hint: Operand,
    },
    Bytes(Vec<String>),
    Words(Vec<String>),
    Res(String, Option<String>),
}

#[derive(Debug, Clone)]
enum Operand {
    None,
    Acc,
    Imm(String),
    /// Address-like operand; `force_abs` from `a:`.
    Addr {
        expr: String,
        index: Option<char>,
        force_abs: bool,
    },
    Ind(String),
    IndX(String),
    IndY(String),
}

impl Assembler {
    /// New assembler with no symbols.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Predefine a symbol (e.g. a RAM address or a routine found at run
    /// time).
    pub fn define(&mut self, name: &str, value: u16) -> &mut Self {
        self.predefined.insert(name.to_string(), value);
        self
    }

    /// Assemble `src`.
    pub fn assemble(&self, src: &str) -> Result<Assembled, AsmError> {
        let items = parse(src)?;
        // Pass 1: sizes and symbol values (unknown symbols size as absolute).
        let mut syms = self.predefined.clone();
        let mut sizes: Vec<u8> = Vec::with_capacity(items.len());
        let mut pc: u32 = 0;
        for (line, item) in &items {
            let line = *line;
            let size: u32 = match item {
                Item::Org(e) => {
                    pc = u32::from(eval(e, &syms, pc, line, false)?.unwrap_or(0));
                    0
                }
                Item::Label(name) => {
                    define_sym(&mut syms, name, pc, line)?;
                    0
                }
                Item::Const(name, e) => {
                    let v = eval(e, &syms, pc, line, false)?.ok_or_else(|| AsmError {
                        line,
                        msg: format!("constant {name} uses a symbol defined later"),
                    })?;
                    if syms.insert(name.clone(), v).is_some() {
                        return Err(err(line, format!("duplicate symbol {name}")));
                    }
                    0
                }
                Item::Instr { mn, mode_hint } => {
                    u32::from(resolve_mode(mn, mode_hint, &syms, pc, line, false)?.len())
                }
                Item::Bytes(list) => list.iter().map(|e| byte_item_len(e)).sum(),
                Item::Words(list) => 2 * list.len() as u32,
                Item::Res(n, _) => u32::from(eval(n, &syms, pc, line, true)?.unwrap_or(0)),
            };
            sizes.push(size as u8);
            pc += size;
            if pc > 0x1_0000 {
                return Err(err(line, "program counter passed $FFFF".into()));
            }
        }
        // Pass 2: emit with every symbol known; keep pass-1 sizes.
        let mut out = Assembled {
            chunks: Vec::new(),
            symbols: syms.clone(),
        };
        let mut pc: u32 = 0;
        let mut cur: Option<(u16, Vec<u8>)> = None;
        for (i, (line, item)) in items.iter().enumerate() {
            let line = *line;
            match item {
                Item::Org(e) => {
                    if let Some(c) = cur.take() {
                        if !c.1.is_empty() {
                            out.chunks.push(c);
                        }
                    }
                    let v = eval(e, &syms, pc, line, true)?.unwrap_or(0);
                    pc = u32::from(v);
                    cur = Some((v, Vec::new()));
                }
                Item::Label(_) | Item::Const(..) => {}
                Item::Instr { mn, mode_hint } => {
                    let mode = resolve_mode(mn, mode_hint, &syms, pc, line, true)?;
                    // Pass 1 may have chosen absolute for a forward reference
                    // that turned out to be zero page: keep the pass-1 size.
                    let mode = if mode.len() != sizes[i] {
                        widen(mn, mode)
                            .ok_or_else(|| err(line, format!("cannot keep size for {mn}")))?
                    } else {
                        mode
                    };
                    let op = opcode(mn, mode)
                        .ok_or_else(|| err(line, format!("{mn} has no {mode:?} mode")))?;
                    let bytes = encode_operand(mn, mode, mode_hint, &syms, pc, line)?;
                    let buf = &mut cur.get_or_insert_with(|| (pc as u16, Vec::new())).1;
                    buf.push(op);
                    buf.extend_from_slice(&bytes);
                }
                Item::Bytes(list) => {
                    let buf = &mut cur.get_or_insert_with(|| (pc as u16, Vec::new())).1;
                    for e in list {
                        if let Some(s) = string_literal(e) {
                            buf.extend_from_slice(s.as_bytes());
                        } else {
                            let v = eval(e, &syms, pc, line, true)?.unwrap_or(0);
                            if v > 0xFF {
                                return Err(err(line, format!(".byte value ${v:X} > $FF")));
                            }
                            buf.push(v as u8);
                        }
                    }
                }
                Item::Words(list) => {
                    let buf = &mut cur.get_or_insert_with(|| (pc as u16, Vec::new())).1;
                    for e in list {
                        let v = eval(e, &syms, pc, line, true)?.unwrap_or(0);
                        buf.extend_from_slice(&v.to_le_bytes());
                    }
                }
                Item::Res(n, fill) => {
                    let n = eval(n, &syms, pc, line, true)?.unwrap_or(0);
                    let f = match fill {
                        Some(f) => eval(f, &syms, pc, line, true)?.unwrap_or(0),
                        None => 0,
                    };
                    if f > 0xFF {
                        return Err(err(line, ".res fill must be a byte".into()));
                    }
                    let buf = &mut cur.get_or_insert_with(|| (pc as u16, Vec::new())).1;
                    buf.extend(std::iter::repeat_n(f as u8, usize::from(n)));
                }
            }
            pc += u32::from(sizes[i]);
        }
        if let Some(c) = cur.take() {
            if !c.1.is_empty() {
                out.chunks.push(c);
            }
        }
        Ok(out)
    }
}

fn err(line: usize, msg: String) -> AsmError {
    AsmError { line, msg }
}

fn define_sym(
    syms: &mut BTreeMap<String, u16>,
    name: &str,
    pc: u32,
    line: usize,
) -> Result<(), AsmError> {
    if syms.insert(name.to_string(), pc as u16).is_some() {
        return Err(err(line, format!("duplicate symbol {name}")));
    }
    Ok(())
}

fn widen(mn: &str, mode: Mode) -> Option<Mode> {
    let wide = match mode {
        ZeroPage => Absolute,
        ZeroPageX => AbsoluteX,
        ZeroPageY => AbsoluteY,
        other => other,
    };
    has_mode(mn, wide).then_some(wide)
}

fn byte_item_len(e: &str) -> u32 {
    string_literal(e).map_or(1, |s| s.len() as u32)
}

fn string_literal(e: &str) -> Option<&str> {
    let t = e.trim();
    (t.len() >= 2 && t.starts_with('"') && t.ends_with('"')).then(|| &t[1..t.len() - 1])
}

fn resolve_mode(
    mn: &str,
    op: &Operand,
    syms: &BTreeMap<String, u16>,
    pc: u32,
    line: usize,
    final_pass: bool,
) -> Result<Mode, AsmError> {
    let pick = |m: Mode| -> Result<Mode, AsmError> {
        if has_mode(mn, m) {
            Ok(m)
        } else {
            Err(err(line, format!("{mn} has no {m:?} mode")))
        }
    };
    match op {
        Operand::None => {
            if has_mode(mn, Implied) {
                Ok(Implied)
            } else if has_mode(mn, Accumulator) {
                Ok(Accumulator)
            } else {
                Err(err(line, format!("{mn} needs an operand")))
            }
        }
        Operand::Acc => pick(Accumulator),
        Operand::Imm(_) => pick(Immediate),
        Operand::Ind(_) => pick(Indirect),
        Operand::IndX(_) => pick(IndirectX),
        Operand::IndY(_) => pick(IndirectY),
        Operand::Addr {
            expr,
            index,
            force_abs,
        } => {
            if has_mode(mn, Relative) {
                if index.is_some() {
                    return Err(err(line, format!("{mn} takes a plain branch target")));
                }
                return Ok(Relative);
            }
            let v = eval(expr, syms, pc, line, final_pass)?;
            let zp = !force_abs && matches!(v, Some(x) if x < 0x100);
            let (zp_mode, abs_mode) = match index {
                None => (ZeroPage, Absolute),
                Some('X') => (ZeroPageX, AbsoluteX),
                Some('Y') => (ZeroPageY, AbsoluteY),
                Some(c) => return Err(err(line, format!("bad index register {c}"))),
            };
            if zp && has_mode(mn, zp_mode) {
                Ok(zp_mode)
            } else if has_mode(mn, abs_mode) {
                Ok(abs_mode)
            } else if has_mode(mn, zp_mode) {
                // e.g. STX zp,Y with an operand that does not fit zero page.
                Err(err(
                    line,
                    format!("{mn} {zp_mode:?} operand must be < $100"),
                ))
            } else {
                Err(err(line, format!("{mn} has no {abs_mode:?} mode")))
            }
        }
    }
}

fn encode_operand(
    mn: &str,
    mode: Mode,
    op: &Operand,
    syms: &BTreeMap<String, u16>,
    pc: u32,
    line: usize,
) -> Result<Vec<u8>, AsmError> {
    let expr = match op {
        Operand::None | Operand::Acc => return Ok(Vec::new()),
        Operand::Imm(e) | Operand::Ind(e) | Operand::IndX(e) | Operand::IndY(e) => e,
        Operand::Addr { expr, .. } => expr,
    };
    let v = eval(expr, syms, pc, line, true)?.unwrap_or(0);
    match mode.len() {
        2 => {
            if mode == Relative {
                let target = i64::from(v);
                let next = i64::from(pc) + 2;
                let d = target - next;
                if !(-128..=127).contains(&d) {
                    return Err(err(line, format!("{mn} target out of range ({d})")));
                }
                Ok(vec![d as i8 as u8])
            } else {
                if v > 0xFF {
                    return Err(err(
                        line,
                        format!("{mn} operand ${v:X} does not fit a byte"),
                    ));
                }
                Ok(vec![v as u8])
            }
        }
        3 => Ok(v.to_le_bytes().to_vec()),
        _ => Ok(Vec::new()),
    }
}

/// Evaluate an expression. `Ok(None)` = an unknown symbol in a non-final
/// pass; unknown symbols are an error in the final pass.
fn eval(
    e: &str,
    syms: &BTreeMap<String, u16>,
    pc: u32,
    line: usize,
    final_pass: bool,
) -> Result<Option<u16>, AsmError> {
    let mut s = e.trim();
    let mut byte_sel: Option<char> = None;
    if let Some(rest) = s.strip_prefix('<') {
        byte_sel = Some('<');
        s = rest.trim();
    } else if let Some(rest) = s.strip_prefix('>') {
        byte_sel = Some('>');
        s = rest.trim();
    }
    if s.is_empty() {
        return Err(err(line, "empty expression".into()));
    }
    // Split into signed terms. A leading sign belongs to the first term.
    let mut total: i64 = 0;
    let mut unknown = false;
    let mut sign: i64 = 1;
    let mut term = String::new();
    let mut terms: Vec<(i64, String)> = Vec::new();
    let mut in_char = false;
    for (i, ch) in s.chars().enumerate() {
        if ch == '\'' {
            in_char = !in_char;
            term.push(ch);
            continue;
        }
        if !in_char && (ch == '+' || ch == '-') {
            if term.trim().is_empty() && i == 0 {
                if ch == '-' {
                    sign = -sign;
                }
                continue;
            }
            terms.push((sign, std::mem::take(&mut term)));
            sign = if ch == '-' { -1 } else { 1 };
            continue;
        }
        term.push(ch);
    }
    terms.push((sign, term));
    for (sg, t) in terms {
        let t = t.trim();
        let v: Option<i64> = if t == "*" {
            Some(i64::from(pc))
        } else if let Some(h) = t.strip_prefix('$') {
            Some(i64::from_str_radix(h, 16).map_err(|_| err(line, format!("bad hex number {t}")))?)
        } else if let Some(b) = t.strip_prefix('%') {
            Some(i64::from_str_radix(b, 2).map_err(|_| err(line, format!("bad binary {t}")))?)
        } else if t.len() == 3 && t.starts_with('\'') && t.ends_with('\'') {
            Some(i64::from(t.as_bytes()[1]))
        } else if t.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            Some(
                t.parse::<i64>()
                    .map_err(|_| err(line, format!("bad number {t}")))?,
            )
        } else if is_symbol(t) {
            match syms.get(t) {
                Some(&v) => Some(i64::from(v)),
                None if final_pass => return Err(err(line, format!("undefined symbol {t}"))),
                None => None,
            }
        } else {
            return Err(err(line, format!("cannot parse expression term '{t}'")));
        };
        match v {
            Some(v) => total += sg * v,
            None => unknown = true,
        }
    }
    if unknown {
        return Ok(None);
    }
    let total = match byte_sel {
        Some('<') => total & 0xFF,
        Some(_) => (total >> 8) & 0xFF,
        None => total,
    };
    if !(0..=0xFFFF).contains(&total) {
        return Err(err(line, format!("value {total} out of 16-bit range")));
    }
    Ok(Some(total as u16))
}

fn is_symbol(t: &str) -> bool {
    let mut c = t.chars();
    matches!(c.next(), Some(ch) if ch.is_ascii_alphabetic() || ch == '_' || ch == '@' || ch == '.')
        && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '@' || ch == '.')
}

fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    let mut in_chr = false;
    for (i, ch) in line.char_indices() {
        match ch {
            '"' if !in_chr => in_str = !in_str,
            '\'' if !in_str => in_chr = !in_chr,
            ';' if !in_str && !in_chr => return &line[..i],
            _ => {}
        }
    }
    line
}

fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    for ch in s.chars() {
        match ch {
            '"' => {
                in_str = !in_str;
                cur.push(ch);
            }
            ',' if !in_str => out.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn parse(src: &str) -> Result<Vec<(usize, Item)>, AsmError> {
    let mut items = Vec::new();
    for (idx, raw) in src.lines().enumerate() {
        let line = idx + 1;
        let mut rest = strip_comment(raw).trim();
        // Labels (`name:`), possibly several, before a statement.
        while let Some(colon) = rest.find(':') {
            let head = &rest[..colon];
            // `a:` is the force-absolute prefix, not a label.
            if !is_symbol(head) || head.eq_ignore_ascii_case("a") {
                break;
            }
            items.push((line, Item::Label(head.to_string())));
            rest = rest[colon + 1..].trim();
        }
        if rest.is_empty() {
            continue;
        }
        // Constant assignment `NAME = expr`.
        if let Some(eq) = rest.find('=') {
            let name = rest[..eq].trim();
            if is_symbol(name) && !name.starts_with('.') {
                items.push((
                    line,
                    Item::Const(name.to_string(), rest[eq + 1..].trim().to_string()),
                ));
                continue;
            }
        }
        let (word, args) = match rest.find(char::is_whitespace) {
            Some(i) => (&rest[..i], rest[i..].trim()),
            None => (rest, ""),
        };
        let upper = word.to_ascii_uppercase();
        match upper.as_str() {
            ".ORG" => items.push((line, Item::Org(args.to_string()))),
            ".BYTE" | ".DB" | ".BYT" => items.push((line, Item::Bytes(split_args(args)))),
            ".WORD" | ".DW" | ".ADDR" => items.push((line, Item::Words(split_args(args)))),
            ".RES" | ".DS" => {
                let a = split_args(args);
                match a.as_slice() {
                    [n] => items.push((line, Item::Res(n.clone(), None))),
                    [n, f] => items.push((line, Item::Res(n.clone(), Some(f.clone())))),
                    _ => return Err(err(line, ".res takes COUNT[, FILL]".into())),
                }
            }
            _ if upper.starts_with('.') => {
                return Err(err(line, format!("unknown directive {word}")))
            }
            _ => {
                let mn: &'static str = OPCODES
                    .iter()
                    .find(|(m, _, _)| *m == upper)
                    .map(|(m, _, _)| *m)
                    .filter(|m| is_mnemonic(m))
                    .ok_or_else(|| err(line, format!("unknown mnemonic {word}")))?;
                let operand = parse_operand(args, line)?;
                items.push((
                    line,
                    Item::Instr {
                        mn,
                        mode_hint: operand,
                    },
                ));
            }
        }
    }
    Ok(items)
}

fn parse_operand(args: &str, line: usize) -> Result<Operand, AsmError> {
    let a = args.trim();
    if a.is_empty() {
        return Ok(Operand::None);
    }
    if a.eq_ignore_ascii_case("a") {
        return Ok(Operand::Acc);
    }
    if let Some(imm) = a.strip_prefix('#') {
        return Ok(Operand::Imm(imm.trim().to_string()));
    }
    let compact: String = a.chars().filter(|c| !c.is_whitespace()).collect();
    let upper = compact.to_ascii_uppercase();
    if compact.starts_with('(') {
        if upper.ends_with(",X)") {
            return Ok(Operand::IndX(compact[1..compact.len() - 3].to_string()));
        }
        if upper.ends_with("),Y") {
            return Ok(Operand::IndY(compact[1..compact.len() - 3].to_string()));
        }
        if compact.ends_with(')') {
            return Ok(Operand::Ind(compact[1..compact.len() - 1].to_string()));
        }
        return Err(err(line, format!("bad indirect operand {a}")));
    }
    let (mut expr, index) = if upper.ends_with(",X") {
        (compact[..compact.len() - 2].to_string(), Some('X'))
    } else if upper.ends_with(",Y") {
        (compact[..compact.len() - 2].to_string(), Some('Y'))
    } else {
        (compact.clone(), None)
    };
    let mut force_abs = false;
    if expr.len() > 2 && expr[..2].eq_ignore_ascii_case("a:") {
        force_abs = true;
        expr = expr[2..].to_string();
    }
    Ok(Operand::Addr {
        expr,
        index,
        force_abs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(org: u16, src: &str) -> Vec<u8> {
        assemble_at(org, src).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn opcode_table_is_complete_and_unique() {
        assert_eq!(OPCODES.len(), 151, "official 6502 opcode count");
        let mut seen = [false; 256];
        for (_, _, op) in OPCODES {
            assert!(!seen[*op as usize], "duplicate opcode {op:02X}");
            seen[*op as usize] = true;
        }
        let mnemonics: std::collections::BTreeSet<_> = OPCODES.iter().map(|(m, _, _)| *m).collect();
        assert_eq!(mnemonics.len(), 56);
    }

    #[test]
    fn every_table_entry_round_trips_through_the_assembler() {
        for (mn, mode, op) in OPCODES {
            let operand = match mode {
                Implied => String::new(),
                Accumulator => "A".into(),
                Immediate => "#$12".into(),
                ZeroPage => "$12".into(),
                ZeroPageX => "$12,X".into(),
                ZeroPageY => "$12,Y".into(),
                Absolute => "$1234".into(),
                AbsoluteX => "$1234,X".into(),
                AbsoluteY => "$1234,Y".into(),
                Indirect => "($1234)".into(),
                IndirectX => "($12,X)".into(),
                IndirectY => "($12),Y".into(),
                Relative => "*+2".into(),
            };
            let out = bytes(0x8000, &format!("{mn} {operand}"));
            assert_eq!(out[0], *op, "{mn} {mode:?}");
            assert_eq!(out.len(), usize::from(mode.len()), "{mn} {mode:?}");
            assert_eq!(opcode_len(*op), Some(mode.len()));
        }
    }

    #[test]
    fn known_encodings() {
        assert_eq!(
            bytes(0xC358, "LDA #$03\nSTA $0700\nINC $0760"),
            vec![0xA9, 0x03, 0x8D, 0x00, 0x07, 0xEE, 0x60, 0x07]
        );
        assert_eq!(bytes(0x8000, "LDA ($0E),Y"), vec![0xB1, 0x0E]);
        assert_eq!(bytes(0x8000, "STA ($00,X)"), vec![0x81, 0x00]);
        assert_eq!(bytes(0x8000, "JMP ($FFFC)"), vec![0x6C, 0xFC, 0xFF]);
        assert_eq!(bytes(0x8000, "LDX $10,Y"), vec![0xB6, 0x10]);
        assert_eq!(
            bytes(0x8000, "LDA $10,Y"),
            vec![0xB9, 0x10, 0x00],
            "no zp,Y for LDA"
        );
        assert_eq!(bytes(0x8000, "LDA a:$0010"), vec![0xAD, 0x10, 0x00]);
        assert_eq!(bytes(0x8000, "asl\nasl a\nrts"), vec![0x0A, 0x0A, 0x60]);
        assert_eq!(
            bytes(0x8000, "LDA #'A'\nLDA #%1010"),
            vec![0xA9, 0x41, 0xA9, 0x0A]
        );
        assert_eq!(bytes(0x8000, "LDA #100"), vec![0xA9, 100]);
    }

    #[test]
    fn labels_forward_refs_and_branches() {
        let src = "
            .org $C000
        start:
            LDX #0
        loop:  INX
            CPX #5
            BNE loop        ; backward
            BEQ done        ; forward
            NOP
        done:
            JMP start
            JSR sub         ; forward absolute
        sub: RTS
        ";
        let out = assemble(src).unwrap();
        assert_eq!(out.symbol("loop"), Some(0xC002));
        assert_eq!(out.symbol("done"), Some(0xC00A));
        assert_eq!(
            out.bytes(),
            vec![
                0xA2, 0x00, 0xE8, 0xE0, 0x05, 0xD0, 0xFB, 0xF0, 0x01, 0xEA, 0x4C, 0x00, 0xC0, 0x20,
                0x10, 0xC0, 0x60
            ]
        );
    }

    #[test]
    fn forward_zero_page_symbol_keeps_absolute_size() {
        // A zero-page constant defined after its first use was sized as
        // absolute in pass 1, and pass 2 keeps that size.
        let out = assemble(".org $8000\nLDA later\nlater = $10\nRTS").unwrap();
        assert_eq!(out.bytes(), vec![0xAD, 0x10, 0x00, 0x60]);
        // Same for a forward label that lands in zero page.
        let out = assemble(".org $0000\nLDA lab\nlab: .byte 7").unwrap();
        assert_eq!(out.bytes(), vec![0xAD, 0x03, 0x00, 7]);
    }

    #[test]
    fn data_directives_and_byte_selectors() {
        let src = ".org $9000\nt: .byte 1, $02, \"AB\", <t, >t\n.word t, t+2\n.res 3, $EA\n.res 1";
        let out = assemble(src).unwrap();
        assert_eq!(
            out.bytes(),
            vec![1, 2, b'A', b'B', 0x00, 0x90, 0x00, 0x90, 0x02, 0x90, 0xEA, 0xEA, 0xEA, 0]
        );
    }

    #[test]
    fn multiple_orgs_make_chunks() {
        let out = assemble(".org $C000\nNOP\n.org $D000\nRTS\nRTS").unwrap();
        assert_eq!(
            out.chunks,
            vec![(0xC000, vec![0xEA]), (0xD000, vec![0x60, 0x60])]
        );
    }

    #[test]
    fn predefined_symbols_and_constants() {
        let mut a = Assembler::new();
        a.define("HOOK", 0xC123);
        let out = a
            .assemble("LIVES = $0700\n.org $8000\nJSR HOOK\nSTA LIVES\nSTA $07-$06")
            .unwrap();
        assert_eq!(
            out.bytes(),
            vec![0x20, 0x23, 0xC1, 0x8D, 0x00, 0x07, 0x85, 0x01]
        );
    }

    #[test]
    fn errors_are_reported_with_lines() {
        let e = assemble("NOP\nFOO $10").unwrap_err();
        assert_eq!(e.line, 2);
        assert!(assemble(".org $8000\nBNE far\n.res 200\nfar: RTS").is_err());
        assert!(assemble("LDA #$100").is_err());
        assert!(assemble("STX $1234,Y").is_err());
        assert!(assemble("JMP ($12),Y").is_err());
        assert!(assemble("a: NOP\na: NOP").is_err() || assemble("x: NOP\nx: NOP").is_err());
        assert!(assemble("LDA nowhere").is_err());
        assert!(assemble(".bogus 1").is_err());
    }

    #[test]
    fn decode_helpers() {
        assert_eq!(decode(0x20), Some(("JSR", Absolute)));
        assert_eq!(decode(0x02), None);
        assert!(ends_flow(0x60) && ends_flow(0x4C) && !ends_flow(0x20));
    }
}
