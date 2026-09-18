//! Coverage: read `ports.toml` and print ported/verified/total per
//! bank. With `--check`, fail (exit 3) when any bank regresses below its
//! `[policy]` floor — the CI regression gate.

use std::path::{Path, PathBuf};

#[derive(Default, Clone)]
struct Counts {
    total: u32,
    code: u32,
    data: u32,
    tables: u32,
    ambiguous: u32,
    review: u32,
    unported: u32,
    ported: u32,
    verified: u32,
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

type BankTables = ([Counts; 8], [u32; 8], [u32; 8]);

fn parse_ports(path: &Path) -> Result<BankTables, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let banks: [Counts; 8] = Default::default();
    let mut banks = banks;
    let (mut pol_p, mut pol_v) = ([0u32; 8], [0u32; 8]);
    let mut bank: Option<u8> = None;
    let mut kind = String::new();
    let mut status = String::new();
    let mut review = false;
    let mut in_routine = false;
    let mut in_policy = false;

    let flush =
        |bank: Option<u8>, kind: &str, status: &str, review: bool, banks: &mut [Counts; 8]| {
            let Some(b) = bank else { return };
            if (b as usize) >= 8 {
                return;
            }
            let c = &mut banks[b as usize];
            c.total += 1;
            match kind {
                "code" => c.code += 1,
                "data" => c.data += 1,
                "jump-table" => c.tables += 1,
                _ => c.ambiguous += 1,
            }
            match status {
                "ported" => c.ported += 1,
                "verified" => {
                    c.ported += 1;
                    c.verified += 1;
                }
                _ => c.unported += 1,
            }
            if review {
                c.review += 1;
            }
        };

    for line in text.lines() {
        let t = line.trim();
        if t == "[[routine]]" {
            if in_routine {
                flush(bank, &kind, &status, review, &mut banks);
            }
            in_routine = true;
            in_policy = false;
            bank = None;
            kind.clear();
            status.clear();
            review = false;
            continue;
        }
        if t == "[policy]" {
            if in_routine {
                flush(bank, &kind, &status, review, &mut banks);
                in_routine = false;
            }
            in_policy = true;
            continue;
        }
        if t.starts_with('[') {
            if in_routine {
                flush(bank, &kind, &status, review, &mut banks);
                in_routine = false;
            }
            in_policy = false;
            continue;
        }
        let Some(eq) = t.find('=') else { continue };
        let (k, v) = (t[..eq].trim(), t[eq + 1..].trim());
        if in_policy {
            let dst = if k == "min_ported_per_bank" {
                Some(&mut pol_p)
            } else if k == "min_verified_per_bank" {
                Some(&mut pol_v)
            } else {
                None
            };
            if let Some(arr) = dst {
                for (i, n) in v
                    .trim_matches(|c| c == '[' || c == ']')
                    .split(',')
                    .enumerate()
                    .take(8)
                {
                    arr[i] = n.trim().parse().unwrap_or(0);
                }
            }
            continue;
        }
        if !in_routine {
            continue;
        }
        match k {
            "bank" => bank = v.parse().ok(),
            "kind" => {
                kind = unescape(v.trim_matches('"'));
            }
            "status" => {
                status = unescape(v.trim_matches('"'));
            }
            "review" => review = v == "true",
            _ => {}
        }
    }
    if in_routine {
        flush(bank, &kind, &status, review, &mut banks);
    }
    Ok((banks, pol_p, pol_v))
}

pub fn usage() -> &'static str {
    "xtask coverage [--ports FILE] [--check]\n\
     \x20   prints ported/verified/total per bank from ports.toml.\n\
     \x20   --check enforces the [policy] floors (fails on regression).\n"
}

pub fn run(raw: &[String]) -> i32 {
    let mut ports = PathBuf::from("ports.toml");
    let mut check = false;
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--ports" => {
                i += 1;
                match raw.get(i) {
                    Some(p) => ports = PathBuf::from(p),
                    None => {
                        eprintln!("coverage: --ports needs a value");
                        return 2;
                    }
                }
            }
            "--check" => check = true,
            "--help" | "-h" => {
                print!("{}", usage());
                return 0;
            }
            other => {
                eprintln!("coverage: unknown flag '{other}'\n\n{}", usage());
                return 2;
            }
        }
        i += 1;
    }

    let (banks, pol_p, pol_v) = match parse_ports(&ports) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("coverage: {e}");
            return 3;
        }
    };

    println!(
        "{:<5} {:>6} {:>6} {:>6} {:>6} {:>6} {:>7} {:>8} {:>9} {:>9}",
        "bank",
        "total",
        "code",
        "data",
        "tables",
        "ambig",
        "review",
        "ported",
        "verified",
        "%ported"
    );
    let mut t = Counts::default();
    for (b, c) in banks.iter().enumerate() {
        let pct = if c.total > 0 {
            100.0 * c.ported as f64 / c.total as f64
        } else {
            0.0
        };
        println!(
            "{:<5} {:>6} {:>6} {:>6} {:>6} {:>6} {:>7} {:>8} {:>9} {:>8.1}%",
            format!("{b}"),
            c.total,
            c.code,
            c.data,
            c.tables,
            c.ambiguous,
            c.review,
            c.ported,
            c.verified,
            pct
        );
        t.total += c.total;
        t.code += c.code;
        t.data += c.data;
        t.tables += c.tables;
        t.ambiguous += c.ambiguous;
        t.review += c.review;
        t.unported += c.unported;
        t.ported += c.ported;
        t.verified += c.verified;
    }
    let pct = if t.total > 0 {
        100.0 * t.ported as f64 / t.total as f64
    } else {
        0.0
    };
    println!(
        "{:<5} {:>6} {:>6} {:>6} {:>6} {:>6} {:>7} {:>8} {:>9} {:>8.1}%",
        "all", t.total, t.code, t.data, t.tables, t.ambiguous, t.review, t.ported, t.verified, pct
    );

    if check {
        let mut regressed = false;
        for b in 0..8 {
            if banks[b].ported < pol_p[b] {
                eprintln!(
                    "coverage regression: bank {b} ported {} < floor {}",
                    banks[b].ported, pol_p[b]
                );
                regressed = true;
            }
            if banks[b].verified < pol_v[b] {
                eprintln!(
                    "coverage regression: bank {b} verified {} < floor {}",
                    banks[b].verified, pol_v[b]
                );
                regressed = true;
            }
        }
        if regressed {
            return 3;
        }
        println!("coverage --check ok: no bank below its policy floor");
    }
    0
}
