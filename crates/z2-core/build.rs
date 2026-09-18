//! Code generator: `ram-map.toml` -> `$OUT_DIR/ram_gen.rs`.
//!
//! This build script is intentionally dependency-free (it must run
//! without any `[build-dependencies]`), so it parses a strict subset of
//! TOML by hand — exactly the
//! schema documented at the top of `ram-map.toml`:
//!
//! * `[[entry]]` / `[[array]]` section headers,
//! * `key = value` lines where value is `0xHEX`, decimal, `"string"`,
//!   or `true`/`false`,
//! * `#` starts a comment outside of quoted strings.
//!
#![allow(clippy::field_reassign_with_default)]
//!
//! Anything else is a hard build error. The script validates the map
//! (identifier shape, widths, ranges inside `$0000-$07FF`, no duplicate
//! names, no overlapping byte ranges, uniform slot counts) and emits:
//!
//! * `pub const ADDR_<NAME>: u16` for every `[[entry]]`,
//! * `pub const BASE_<NAME>: u16` + `pub const <NAME>_LEN: usize` for arrays,
//! * `Ram::<name>() / Ram::set_<name>()` with doc comments carrying the
//!   address, disassembly label, source and verification flag,
//! * `Ram::enemy(slot) -> EnemyRef` / `Ram::enemy_mut(slot)` and the
//!   projectile equivalents, with one method per slot array,
//! * indexed `Ram::<name>(i) / Ram::set_<name>(i, v)` for `slot_of="none"`.
//!
//! `crates/z2-core/src/ram.rs` pulls the output in with
//! `include!(concat!(env!("OUT_DIR"), "/ram_gen.rs"))`.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;

const RAM_LEN: u32 = 0x0800;

#[derive(Debug, Default)]
struct Entry {
    addr: u32,
    width: u32,
    endian: String, // "le" (default) | "be"
    name: String,
    doc: String,
    label: String,
    source: String,
    verified: bool,
}

#[derive(Debug, Default)]
struct Array {
    slot_of: String, // "enemy" | "projectile" | "none"
    name: String,
    base: u32,
    count: u32,
    width: u32,
    doc: String,
    label: String,
    source: String,
    verified: bool,
}

#[derive(Debug, Default)]
struct Map {
    entries: Vec<Entry>,
    arrays: Vec<Array>,
}

fn fail(msg: &str) -> ! {
    panic!("z2-core build.rs: {msg}");
}

fn parse_value(raw: &str) -> String {
    let v = raw.trim().to_string();
    if v.starts_with('"') {
        if !v.ends_with('"') || v.len() < 2 {
            fail(&format!("unterminated string value: {raw}"));
        }
        return v[1..v.len() - 1].to_string();
    }
    // Unquoted: strip trailing comment, must be int or bool.
    let end = v.find('#').unwrap_or(v.len());
    v[..end].trim().to_string()
}

fn parse_u32(raw: &str, what: &str) -> u32 {
    let v = parse_value(raw);
    if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).unwrap_or_else(|_| fail(&format!("bad hex for {what}: {raw}")))
    } else {
        v.parse::<u32>()
            .unwrap_or_else(|_| fail(&format!("bad int for {what}: {raw}")))
    }
}

fn parse_str(raw: &str) -> String {
    let t = raw.trim();
    if !t.starts_with('"') {
        fail(&format!("expected quoted string, got: {raw}"));
    }
    parse_value(raw)
}

fn parse_bool(raw: &str) -> bool {
    match parse_value(raw).as_str() {
        "true" => true,
        "false" => false,
        other => fail(&format!("expected true/false, got: {other}")),
    }
}

fn parse_map(text: &str) -> Map {
    let mut map = Map::default();
    #[derive(PartialEq)]
    enum Section {
        None,
        Entry,
        Array,
    }
    let mut section = Section::None;
    let mut cur: BTreeMap<String, String> = BTreeMap::new();

    let flush = |section: &mut Section, cur: &mut BTreeMap<String, String>, map: &mut Map| {
        if cur.is_empty() {
            return;
        }
        match section {
            Section::Entry => map.entries.push(build_entry(cur)),
            Section::Array => map.arrays.push(build_array(cur)),
            Section::None => fail("key/value outside any [[entry]]/[[array]] section"),
        }
        cur.clear();
    };

    for (lineno, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "[[entry]]" {
            flush(&mut section, &mut cur, &mut map);
            section = Section::Entry;
            continue;
        }
        if t == "[[array]]" {
            flush(&mut section, &mut cur, &mut map);
            section = Section::Array;
            continue;
        }
        if section == Section::None {
            fail(&format!(
                "line {}: content before first section: {line}",
                lineno + 1
            ));
        }
        let eq = line
            .find('=')
            .unwrap_or_else(|| fail(&format!("line {}: no '=': {line}", lineno + 1)));
        let (k, v) = (
            line[..eq].trim().to_string(),
            line[eq + 1..].trim().to_string(),
        );
        if k.is_empty() || v.is_empty() {
            fail(&format!("line {}: bad pair: {line}", lineno + 1));
        }
        if cur.insert(k.clone(), v).is_some() {
            fail(&format!("line {}: duplicate key '{k}'", lineno + 1));
        }
    }
    flush(&mut section, &mut cur, &mut map);
    map
}

fn take(cur: &mut BTreeMap<String, String>, key: &str, what: &str) -> String {
    cur.remove(key)
        .unwrap_or_else(|| fail(&format!("{what} missing required key '{key}'")))
}

fn build_entry(cur: &mut BTreeMap<String, String>) -> Entry {
    let mut e = Entry::default();
    e.addr = parse_u32(&take(cur, "addr", "entry"), "entry.addr");
    e.width = parse_u32(&take(cur, "width", "entry"), "entry.width");
    e.name = parse_str(&take(cur, "name", "entry"));
    e.doc = parse_str(&take(cur, "doc", "entry"));
    e.label = parse_str(&take(cur, "label", "entry"));
    e.source = parse_str(&take(cur, "source", "entry"));
    e.endian = cur
        .remove("endian")
        .map(|v| parse_value(&v))
        .unwrap_or_else(|| "le".to_string());
    e.verified = cur
        .remove("verified")
        .map(|v| parse_bool(&v))
        .unwrap_or(false);
    if let Some(k) = cur.keys().next() {
        fail(&format!("entry '{}' has unknown key '{k}'", e.name));
    }
    e
}

fn build_array(cur: &mut BTreeMap<String, String>) -> Array {
    let mut a = Array::default();
    a.slot_of = parse_value(&take(cur, "slot_of", "array"));
    a.name = parse_str(&take(cur, "name", "array"));
    a.base = parse_u32(&take(cur, "base", "array"), "array.base");
    a.count = parse_u32(&take(cur, "count", "array"), "array.count");
    a.width = parse_u32(&take(cur, "width", "array"), "array.width");
    a.doc = parse_str(&take(cur, "doc", "array"));
    a.label = parse_str(&take(cur, "label", "array"));
    a.source = parse_str(&take(cur, "source", "array"));
    a.verified = cur
        .remove("verified")
        .map(|v| parse_bool(&v))
        .unwrap_or(false);
    if let Some(k) = cur.keys().next() {
        fail(&format!("array '{}' has unknown key '{k}'", a.name));
    }
    a
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && !s.as_bytes()[0].is_ascii_digit()
}

fn validate(map: &Map) {
    if map.entries.is_empty() && map.arrays.is_empty() {
        fail("ram-map.toml defines no entries");
    }
    // Names unique + identifier-shaped; widths sane; ranges in RAM.
    let mut names: BTreeMap<&str, &str> = BTreeMap::new();
    let mut ranges: Vec<(u32, u32, String)> = Vec::new();
    for e in &map.entries {
        if !is_ident(&e.name) {
            fail(&format!("entry name '{}' must be snake_case", e.name));
        }
        if e.width != 1 && e.width != 2 {
            fail(&format!("entry '{}' width must be 1 or 2", e.name));
        }
        if e.endian != "le" && e.endian != "be" {
            fail(&format!("entry '{}' endian must be le/be", e.name));
        }
        if e.addr + e.width > RAM_LEN {
            fail(&format!("entry '{}' escapes $0000-$07FF", e.name));
        }
        if names.insert(&e.name, "entry").is_some() {
            fail(&format!("duplicate name '{}'", e.name));
        }
        ranges.push((e.addr, e.addr + e.width, format!("entry {}", e.name)));
    }
    for a in &map.arrays {
        if !is_ident(&a.name) {
            fail(&format!("array name '{}' must be snake_case", a.name));
        }
        if a.slot_of != "enemy" && a.slot_of != "projectile" && a.slot_of != "none" {
            fail(&format!(
                "array '{}' slot_of must be enemy/projectile/none",
                a.name
            ));
        }
        if a.width != 1 {
            fail(&format!("array '{}' width must be 1", a.name));
        }
        if a.count == 0 || a.count > 64 {
            fail(&format!("array '{}' count out of range", a.name));
        }
        if a.base + a.count * a.width > RAM_LEN {
            fail(&format!("array '{}' escapes $0000-$07FF", a.name));
        }
        if a.slot_of != "none" && !a.name.starts_with(&format!("{}_", a.slot_of)) {
            fail(&format!(
                "array '{}' must be named '{}_<field>'",
                a.name, a.slot_of
            ));
        }
        if names.insert(&a.name, "array").is_some() {
            fail(&format!("duplicate name '{}'", a.name));
        }
        ranges.push((
            a.base,
            a.base + a.count * a.width,
            format!("array {}", a.name),
        ));
    }
    // No overlapping byte ranges.
    ranges.sort();
    for w in ranges.windows(2) {
        if w[0].1 > w[1].0 {
            fail(&format!("overlap: {} vs {}", w[0].2, w[1].2));
        }
    }
    // Uniform slot counts per slot kind.
    for kind in ["enemy", "projectile"] {
        let mut counts: Vec<u32> = map
            .arrays
            .iter()
            .filter(|a| a.slot_of == kind)
            .map(|a| a.count)
            .collect();
        counts.dedup();
        if counts.len() > 1 {
            fail(&format!("{kind} arrays disagree on count: {counts:?}"));
        }
    }
    // Generated method names must not collide *within the impl that owns
    // them*: singles + indexed arrays share `impl Ram`, while each slot
    // kind has its own ref structs (`EnemyRef`, `ProjectileRef`, ...).
    let mut ram_methods: BTreeMap<String, String> = BTreeMap::new();
    let mut slot_methods: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut claim_ram = |m: String, by: &str| {
        if ram_methods.insert(m.clone(), by.to_string()).is_some() {
            fail(&format!("generated Ram method '{m}' claimed twice ({by})"));
        }
    };
    let mut claim_slot = |kind: &str, m: String, by: &str| {
        let set = slot_methods.entry(kind.to_string()).or_default();
        if set.insert(m.clone(), by.to_string()).is_some() {
            fail(&format!(
                "generated {kind}-slot method '{m}' claimed twice ({by})"
            ));
        }
    };
    for e in &map.entries {
        claim_ram(e.name.clone(), "entry");
        claim_ram(format!("set_{}", e.name), "entry");
    }
    for a in &map.arrays {
        if a.slot_of == "none" {
            claim_ram(a.name.clone(), "array");
            claim_ram(format!("set_{}", a.name), "array");
        } else {
            let field = a.name[a.slot_of.len() + 1..].to_string();
            claim_slot(&a.slot_of, field.clone(), "slot");
            claim_slot(&a.slot_of, format!("set_{field}"), "slot");
        }
    }
    // Hand-written Ram API surface the `impl Ram` methods must not collide with.
    for reserved in [
        "new",
        "from_slice",
        "as_slice",
        "read",
        "write",
        "enemy",
        "enemy_mut",
        "projectile",
        "projectile_mut",
    ] {
        if ram_methods.contains_key(reserved) {
            fail(&format!(
                "map name '{reserved}' collides with hand-written Ram API"
            ));
        }
    }
}

fn fmt_addr(addr: u32) -> String {
    format!("${addr:04X}")
}

fn doc_block(
    addr_text: &str,
    size_note: &str,
    doc: &str,
    label: &str,
    source: &str,
    verified: bool,
) -> String {
    format!(
        "/// RAM {addr_text} ({size_note}) — {doc}\n/// Disassembly label: `{label}` · source: {source} · verified-against-label: {verified}.",
    )
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn generate(map: &Map) -> String {
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED by crates/z2-core/build.rs from ram-map.toml — DO NOT EDIT.\n");
    out.push_str("// Every address below comes from ram-map.toml; see that file for sources.\n\n");

    // ---- address constants
    for e in &map.entries {
        out.push_str(&format!(
            "/// RAM address of `{}` (see [`Ram::{}`]).\n",
            e.name, e.name
        ));
        out.push_str(&format!(
            "pub const ADDR_{}: u16 = 0x{:04X};\n",
            e.name.to_uppercase(),
            e.addr
        ));
    }
    for a in &map.arrays {
        out.push_str(&format!("/// Base RAM address of `{}` array.\n", a.name));
        out.push_str(&format!(
            "pub const BASE_{}: u16 = 0x{:04X};\n",
            a.name.to_uppercase(),
            a.base
        ));
        out.push_str(&format!("/// Length of the `{}` array.\n", a.name));
        out.push_str(&format!(
            "pub const {}_LEN: usize = {};\n",
            a.name.to_uppercase(),
            a.count
        ));
    }
    out.push('\n');

    // ---- single-entry accessors
    out.push_str("impl Ram {\n");
    for e in &map.entries {
        let addr = fmt_addr(e.addr);
        let (range, size_note) = if e.width == 1 {
            (addr.clone(), "1 byte".to_string())
        } else {
            let order = if e.endian == "be" {
                "big-endian"
            } else {
                "little-endian"
            };
            (
                format!("{}-{}", fmt_addr(e.addr), fmt_addr(e.addr + 1)),
                format!("2 bytes, {order}"),
            )
        };
        out.push_str(&doc_block(
            &range, &size_note, &e.doc, &e.label, &e.source, e.verified,
        ));
        out.push('\n');
        if e.width == 1 {
            out.push_str(&format!(
                "pub fn {n}(&self) -> u8 {{ self.mem[0x{addr:04X}] }}\n",
                n = e.name,
                addr = e.addr
            ));
            out.push_str(&format!("/// Write `{n}` (RAM {range}).\n", n = e.name));
            out.push_str(&format!(
                "pub fn set_{n}(&mut self, v: u8) {{ self.mem[0x{addr:04X}] = v; }}\n",
                n = e.name,
                addr = e.addr
            ));
        } else {
            let (hi, lo) = if e.endian == "be" {
                (e.addr, e.addr + 1)
            } else {
                (e.addr + 1, e.addr)
            };
            out.push_str(&format!("pub fn {n}(&self) -> u16 {{ ((self.mem[0x{hi:04X}] as u16) << 8) | self.mem[0x{lo:04X}] as u16 }}\n", n = e.name));
            out.push_str(&format!("/// Write `{n}` (RAM {range}).\n", n = e.name));
            out.push_str(&format!(
                "pub fn set_{n}(&mut self, v: u16) {{ self.mem[0x{hi:04X}] = (v >> 8) as u8; self.mem[0x{lo:04X}] = v as u8; }}\n",
                n = e.name
            ));
        }
    }

    // ---- indexed (slot_of = "none") arrays
    for a in map.arrays.iter().filter(|a| a.slot_of == "none") {
        let range = format!(
            "{}-{} (index 0-{})",
            fmt_addr(a.base),
            fmt_addr(a.base + a.count - 1),
            a.count - 1
        );
        out.push_str(&doc_block(
            &range, "1 byte", &a.doc, &a.label, &a.source, a.verified,
        ));
        out.push('\n');
        out.push_str(&format!(
            "pub fn {n}(&self, index: usize) -> u8 {{ assert!(index < {c}, \"{n}: index {{}} out of range (len {c})\", index); self.mem[0x{b:04X} + index] }}\n",
            n = a.name, c = a.count, b = a.base
        ));
        out.push_str(&format!(
            "/// Write `{n}[index]` (RAM {range}).\n",
            n = a.name
        ));
        out.push_str(&format!(
            "pub fn set_{n}(&mut self, index: usize, v: u8) {{ assert!(index < {c}, \"set_{n}: index {{}} out of range (len {c})\", index); self.mem[0x{b:04X} + index] = v; }}\n",
            n = a.name, c = a.count, b = a.base
        ));
    }
    out.push_str("}\n\n");

    // ---- slot refs (enemy / projectile)
    for kind in ["enemy", "projectile"] {
        let group: Vec<&Array> = map.arrays.iter().filter(|a| a.slot_of == kind).collect();
        if group.is_empty() {
            continue;
        }
        let count = group[0].count;
        let ty = capitalize(kind); // Enemy | Projectile
        let upper = kind.to_uppercase();
        out.push_str(&format!(
            "/// Number of {kind} slots (see `Ram::{kind}`).\n"
        ));
        out.push_str(&format!(
            "pub const {upper}_SLOT_COUNT: usize = {count};\n\n"
        ));

        // Immutable view.
        out.push_str(&format!(
            "/// Immutable view of one {kind} slot (`Ram::{kind}(slot)`).\n"
        ));
        out.push_str("/// Slot `s` addresses `base + s`; Data Crystal numbers the occupants\n");
        out.push_str(&format!(
            "/// in reverse (slot 0 holds the highest-numbered {kind}).\n"
        ));
        out.push_str(&format!("#[derive(Debug, Clone, Copy)]\npub struct {ty}Ref<'a> {{ ram: &'a Ram, slot: usize }}\n"));
        out.push_str(&format!("impl<'a> {ty}Ref<'a> {{\n"));
        out.push_str(&format!("/// Slot index (0..{upper}_SLOT_COUNT).\n    pub fn slot(&self) -> usize {{ self.slot }}\n"));
        for a in &group {
            let field = &a.name[kind.len() + 1..];
            let range = format!("{} + slot", fmt_addr(a.base));
            out.push_str(&doc_block(
                &range, "1 byte", &a.doc, &a.label, &a.source, a.verified,
            ));
            out.push('\n');
            out.push_str(&format!(
                "pub fn {f}(&self) -> u8 {{ self.ram.mem[0x{b:04X} + self.slot] }}\n",
                f = field,
                b = a.base
            ));
        }
        out.push_str("}\n\n");

        // Mutable view.
        out.push_str(&format!(
            "/// Mutable view of one {kind} slot (`Ram::{kind}_mut(slot)`).\n"
        ));
        out.push_str(&format!(
            "#[derive(Debug)]\npub struct {ty}RefMut<'a> {{ ram: &'a mut Ram, slot: usize }}\n"
        ));
        out.push_str(&format!("impl<'a> {ty}RefMut<'a> {{\n"));
        out.push_str(&format!("/// Slot index (0..{upper}_SLOT_COUNT).\n    pub fn slot(&self) -> usize {{ self.slot }}\n"));
        for a in &group {
            let field = &a.name[kind.len() + 1..];
            out.push_str(&format!(
                "/// Write {kind} slot {{slot}} `{field}` (RAM {} + slot).\n",
                fmt_addr(a.base)
            ));
            out.push_str(&format!(
                "pub fn set_{f}(&mut self, v: u8) {{ self.ram.mem[0x{b:04X} + self.slot] = v; }}\n",
                f = field,
                b = a.base
            ));
        }
        out.push_str("}\n\n");

        // Constructors on Ram.
        out.push_str("impl Ram {\n");
        out.push_str(&format!(
            "/// Borrow {kind} slot `slot` (0..{upper}_SLOT_COUNT).\n"
        ));
        out.push_str(&format!(
            "///\n/// # Panics\n///\n/// Panics if `slot >= {upper}_SLOT_COUNT`.\n"
        ));
        out.push_str(&format!(
            "pub fn {k}(&self, slot: usize) -> {T}Ref<'_> {{ assert!(slot < {U}_SLOT_COUNT, \"{k}: slot {{}} out of range\", slot); {T}Ref {{ ram: self, slot }} }}\n",
            k = kind, T = ty, U = upper
        ));
        out.push_str(&format!(
            "/// Mutably borrow {kind} slot `slot` (0..{upper}_SLOT_COUNT).\n"
        ));
        out.push_str(&format!(
            "///\n/// # Panics\n///\n/// Panics if `slot >= {upper}_SLOT_COUNT`.\n"
        ));
        out.push_str(&format!(
            "pub fn {k}_mut(&mut self, slot: usize) -> {T}RefMut<'_> {{ assert!(slot < {U}_SLOT_COUNT, \"{k}_mut: slot {{}} out of range\", slot); {T}RefMut {{ ram: self, slot }} }}\n",
            k = kind, T = ty, U = upper
        ));
        out.push_str("}\n\n");
    }

    out
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let toml_path = manifest_dir.join("../../ram-map.toml");
    println!("cargo:rerun-if-changed={}", toml_path.display());
    let text = fs::read_to_string(&toml_path).expect("cannot read ram-map.toml");
    let map = parse_map(&text);
    validate(&map);
    let code = generate(&map);
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("ram_gen.rs"), code).expect("cannot write ram_gen.rs");
    println!(
        "cargo:warning=ram-map: {} entries + {} arrays code-generated",
        map.entries.len(),
        map.arrays.len()
    );
}
