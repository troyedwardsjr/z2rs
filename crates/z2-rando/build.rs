//! Build script: turn the fixed-bank (bank 7) entries of the routine ledger
//! (`ports.toml` at the workspace root) into a small Rust table used by
//! `trap_policy` to know which byte ranges each routine covers.
//!
//! Only names' addresses, sizes, kinds and call edges are used: no ROM bytes
//! are involved. When the ledger is missing (a trimmed source tree), an
//! empty table is generated and `trap_policy` falls back to untrapping every
//! fixed-bank trap whenever the fixed bank changed at all.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;

struct Entry {
    name: String,
    bank: Option<u8>,
    addr: Option<u16>,
    size: Option<u16>,
    kind: String,
    callees: Vec<String>,
}

fn parse_list(v: &str) -> Vec<String> {
    v.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_num(v: &str) -> Option<u32> {
    let v = v.trim();
    if let Some(h) = v.strip_prefix("0x") {
        u32::from_str_radix(h, 16).ok()
    } else {
        v.parse().ok()
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ledger = manifest.join("../../ports.toml");
    println!("cargo:rerun-if-changed={}", ledger.display());
    println!("cargo:rerun-if-changed=build.rs");
    let text = fs::read_to_string(&ledger).unwrap_or_default();

    let mut entries: Vec<Entry> = Vec::new();
    let mut cur: Option<Entry> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && !line.starts_with("[[routine]]") && line.ends_with(']') {
            if let Some(e) = cur.take() {
                entries.push(e);
            }
            continue;
        }
        if line == "[[routine]]" {
            if let Some(e) = cur.take() {
                entries.push(e);
            }
            cur = Some(Entry {
                name: String::new(),
                bank: None,
                addr: None,
                size: None,
                kind: String::new(),
                callees: Vec::new(),
            });
            continue;
        }
        let Some(e) = cur.as_mut() else { continue };
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "name" => e.name = v.trim().trim_matches('"').to_string(),
            "bank" => e.bank = parse_num(v).map(|n| n as u8),
            "addr" => e.addr = parse_num(v).map(|n| n as u16),
            "size" => e.size = parse_num(v).map(|n| n as u16),
            "kind" => e.kind = v.trim().trim_matches('"').to_string(),
            "callees" => e.callees = parse_list(v),
            _ => {}
        }
    }
    if let Some(e) = cur.take() {
        entries.push(e);
    }

    let bank7: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.bank == Some(7) && e.addr.is_some_and(|a| a >= 0xC000) && e.size.is_some())
        .collect();
    let by_name: BTreeMap<&str, u16> = bank7
        .iter()
        .map(|e| (e.name.as_str(), e.addr.unwrap()))
        .collect();

    let mut sorted = bank7.clone();
    sorted.sort_by_key(|e| e.addr.unwrap());
    let mut out = String::from("&[\n");
    for e in sorted {
        let code = e.kind != "data" && e.kind != "jump-table";
        let callees: Vec<String> = e
            .callees
            .iter()
            .filter(|c| !c.contains('@'))
            .filter_map(|c| by_name.get(c.as_str()))
            .map(|a| format!("0x{a:04X}"))
            .collect();
        out.push_str(&format!(
            "    LedgerRoutine {{ addr: 0x{:04X}, size: {}, code: {}, callees: &[{}] }},\n",
            e.addr.unwrap(),
            e.size.unwrap(),
            code,
            callees.join(", ")
        ));
    }
    out.push(']');
    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bank7_ledger.rs");
    fs::write(dest, out).unwrap();
}
