//! ROM-gated stress test of the whole pipeline: every preset plus random
//! flag combinations over many seeds.
//!
//! For every run it checks that
//! * the pipeline succeeds (no write-ownership conflict, no give-up),
//! * the output is deterministic (a second run is byte-identical, checked
//!   on a subset of the seeds to keep the test time down),
//! * every byte that differs from the vanilla ROM was written through the
//!   tracked [`z2_rando::rom::Rom`] writers (so it has an owning module),
//! * new bytes placed in vanilla `$FF` padding lie inside a range of
//!   [`z2_rando::rom::FREE_SPACE_REGISTRY`] (owned by the module that wrote
//!   them, or the shared pool),
//! * the protected ranges (reset stubs, vectors) are untouched.
//!
//! Run with `Z2_ROM=/path/to/zelda2.nes cargo test -p z2-rando --release
//! --test stress_rom -- --ignored`. `Z2_STRESS_SEEDS` overrides the seed
//! count (default 200).

use z2_rando::flags::{Flags, Preset};
use z2_rando::rng::Rng;
use z2_rando::rom::{
    shared_write_allowed, Owner, FREE_SPACE_REGISTRY, PRG_BANK_LEN, PROTECTED_RANGES,
    VANILLA_PRG_LEN,
};
use z2_rando::{randomize, Output, RandoError};

/// Vanilla `$FF` runs at least this long count as padding.
const PAD_RUN: usize = 12;

fn rom() -> Option<Vec<u8>> {
    match std::env::var("Z2_ROM") {
        Ok(p) if std::path::Path::new(&p).is_file() => {
            Some(z2_assets::rom::open().expect("Z2_ROM"))
        }
        _ => {
            eprintln!("skipping: Z2_ROM not set to an existing file");
            None
        }
    }
}

/// Vanilla bank and CPU address of an output PRG offset (`None` for the
/// expansion banks 8-14).
fn vanilla_site(prg_units: u8, off: usize) -> Option<(u8, u16, usize)> {
    let bank = off / PRG_BANK_LEN;
    let within = off % PRG_BANK_LEN;
    let fixed = usize::from(prg_units) - 1;
    let (vbank, base) = if bank == fixed {
        (7usize, 0xC000)
    } else if bank < 7 {
        (bank, 0x8000)
    } else {
        return None;
    };
    Some((
        vbank as u8,
        (base + within) as u16,
        vbank * PRG_BANK_LEN + within,
    ))
}

fn padding_mask(vanilla: &[u8]) -> Vec<bool> {
    let prg = &vanilla[..VANILLA_PRG_LEN];
    let mut mask = vec![false; prg.len()];
    let mut i = 0;
    while i < prg.len() {
        if prg[i] == 0xFF {
            let mut j = i;
            // Runs never cross a bank boundary.
            let bank_end = (i / PRG_BANK_LEN + 1) * PRG_BANK_LEN;
            while j < bank_end && prg[j] == 0xFF {
                j += 1;
            }
            if j - i >= PAD_RUN {
                mask[i..j].iter_mut().for_each(|m| *m = true);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    mask
}

fn check(vanilla: &[u8], pad: &[bool], out: &Output, what: &str) {
    let prg_len = usize::from(out.prg_units) * PRG_BANK_LEN;
    let (prg, chr) = out.body.split_at(prg_len);
    let vchr = &vanilla[VANILLA_PRG_LEN..];

    // Owner per output byte from the write runs.
    let mut prg_owner: Vec<Option<&str>> = vec![None; prg.len()];
    let mut chr_owner: Vec<Option<&str>> = vec![None; chr.len()];
    for r in &out.write_runs {
        let dst = if r.chr {
            &mut chr_owner
        } else {
            &mut prg_owner
        };
        for o in &mut dst[r.start..r.start + r.len] {
            *o = Some(r.owner);
        }
    }

    for (i, (&a, &b)) in chr.iter().zip(vchr).enumerate() {
        if a != b {
            assert!(
                chr_owner[i].is_some(),
                "{what}: CHR byte {i:#X} changed without a tracked writer"
            );
        }
    }

    for (off, &now) in prg.iter().enumerate() {
        let Some((vbank, addr, voff)) = vanilla_site(out.prg_units, off) else {
            continue; // expansion banks belong to the allocator
        };
        if now == vanilla[voff] {
            continue;
        }
        let owner = prg_owner[off].unwrap_or_else(|| {
            panic!("{what}: PRG bank {vbank} ${addr:04X} changed without a tracked writer")
        });
        for &(b, s, e, name) in PROTECTED_RANGES {
            assert!(
                !(b == vbank && u32::from(addr) >= u32::from(s) && u32::from(addr) < e),
                "{what}: {owner} changed protected {name} at bank {b} ${addr:04X}"
            );
        }
        if pad[voff] {
            let claim = FREE_SPACE_REGISTRY.iter().find(|r| {
                r.bank == vbank && u32::from(addr) >= u32::from(r.start) && u32::from(addr) < r.end
            });
            let Some(claim) = claim else {
                panic!("{what}: {owner} wrote unregistered padding at bank {vbank} ${addr:04X}");
            };
            if let Owner::Module(m) | Owner::ModuleAlloc(m) = claim.owner {
                assert!(
                    m == owner || shared_write_allowed(false, off, m, owner),
                    "{what}: {owner} wrote into {m}'s range at bank {vbank} ${addr:04X}"
                );
            }
        }
    }
}

#[test]
#[ignore = "needs Z2_ROM"]
fn every_preset_and_random_flags_stay_in_bounds() {
    let Some(vanilla) = rom() else { return };
    let pad = padding_mask(&vanilla);
    let n: u64 = std::env::var("Z2_STRESS_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);

    // (name, flags, must succeed)
    let mut cases: Vec<(String, Flags, bool)> = Vec::new();
    for p in Preset::ALL {
        for s in 0..3 {
            cases.push((format!("{p:?}#{s}"), p.flags(), true));
        }
    }
    let mut rng = Rng::new(0x5EED_2025);
    for i in 0..n {
        if i % 4 == 0 {
            cases.push((format!("MaxRando-case{i}"), Preset::MaxRando.flags(), true));
        } else {
            cases.push((format!("random-case{i}"), Flags::random(&mut rng), false));
        }
    }

    let mut gave_up = Vec::new();
    let random_runs = cases.iter().filter(|c| !c.2).count();
    for (k, (name, flags, must)) in cases.iter().enumerate() {
        let seed = format!("stress-{k}");
        let what = format!("{name} seed {seed:?} flags {}", flags.to_flag_string());
        let out = match randomize(&vanilla, &seed, flags) {
            Ok(o) => o,
            Err(e @ RandoError::GaveUp { .. }) if !must => {
                // Fully random switches can ask for something no layout
                // satisfies (for example the longest palaces plus extra
                // boss-exit rooms in the 63 map slots of a palace bank).
                eprintln!("gave up (tolerated): {what}: {e}");
                gave_up.push(format!("{what}: {e}"));
                continue;
            }
            Err(e) => panic!("{what}: {e}"),
        };
        check(&vanilla, &pad, &out, &what);
        if k % 10 == 0 {
            let again = randomize(&vanilla, &seed, flags).unwrap();
            assert!(again == out, "{what}: second run differs");
        }
    }
    eprintln!(
        "{} runs, {} random-flag runs gave up",
        cases.len(),
        gave_up.len()
    );
    // Palace map slots are budgeted when lengths are resolved, so running
    // out of them is never tolerated.
    let capacity: Vec<&String> = gave_up
        .iter()
        .filter(|g| g.contains("rooms") && g.contains("more than"))
        .collect();
    assert!(
        capacity.is_empty(),
        "palace map capacity exhausted:\n{capacity:?}"
    );
    assert!(
        gave_up.len() * 10 <= random_runs,
        "{} of {random_runs} random-flag runs gave up:\n{}",
        gave_up.len(),
        gave_up.join("\n")
    );
}

/// Every registry range is `$FF` in the vanilla ROM, inside its bank
/// window, and overlaps no other range and no protected range.
#[test]
#[ignore = "needs Z2_ROM"]
fn registry_ranges_are_vanilla_padding() {
    let Some(vanilla) = rom() else { return };
    let off = |b: u8, a: u32| {
        let base = if b == 7 { 0xC000 } else { 0x8000 };
        assert!(a >= base && a <= base + 0x4000, "bank {b} ${a:04X}");
        usize::from(b) * PRG_BANK_LEN + (a - base) as usize
    };
    for r in FREE_SPACE_REGISTRY {
        let (s, e) = (off(r.bank, u32::from(r.start)), off(r.bank, r.end));
        assert!(s < e, "{r:?}");
        assert!(
            vanilla[s..e].iter().all(|&b| b == 0xFF),
            "bank {} ${:04X}-${:04X} is not $FF fill",
            r.bank,
            r.start,
            r.end
        );
        for q in FREE_SPACE_REGISTRY {
            if std::ptr::eq(r, q) || q.bank != r.bank {
                continue;
            }
            assert!(
                r.end <= u32::from(q.start) || q.end <= u32::from(r.start),
                "{r:?} overlaps {q:?}"
            );
        }
        for &(b, ps, pe, name) in PROTECTED_RANGES {
            assert!(
                b != r.bank || r.end <= u32::from(ps) || pe <= u32::from(r.start),
                "{r:?} overlaps the {name}"
            );
        }
    }
}
