//! `xtask`: developer task runner.
//!
//! Usage: `cargo xtask <subcommand>` (via `.cargo/config.toml` alias)
//! or `cargo run -p xtask -- <subcommand>`.
//!
//! Subcommand areas:
//! - ledger/porting workflow (`ports.toml`, disassembly rebuild).
//! - coverage (`ports.toml` stats + regression gate).
//! - asset extraction.
//! - movie/corpus tooling.
//! - RAM-map/facts export.

mod corpus;
mod coverage;
mod diag;
mod fuzz;
mod hdpack;
mod ledger;
mod probe;
mod verify;

use std::process::ExitCode;

fn usage() -> String {
    "z2rs xtask\n\
     \n\
     USAGE:\n\
     \x20   cargo run -p xtask -- <SUBCOMMAND>\n\
     \n\
         SUBCOMMANDS:\n\
         \x20   ledger     rebuild disassembly + regenerate ports.toml\n\
         \x20   coverage   ported/verified/total per bank, regression gate\n\
         \x20   verify     lockstep verification vs the oracle\n\
         \x20   fuzz       divergence fuzz from snapshots\n\
         \x20   probe      sub-frame PPU timing probe, oracle vs game\n\
     \x20   extract    extract assets.bin from $Z2_ROM\n\
     \x20   hdpack     HD-pack tooling: template sheets from CHR, pack check\n\
     \x20   corpus     mint labelled snapshots from movies\n\
     \x20   facts      export game facts (not yet implemented)\n\
     \x20   help       print this message\n"
        .to_owned()
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None | Some("help" | "-h" | "--help") => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Some("ledger") => {
            let rest: Vec<String> = args.collect();
            ExitCode::from(ledger::run(&rest) as u8)
        }
        Some("coverage") => {
            let rest: Vec<String> = args.collect();
            ExitCode::from(coverage::run(&rest) as u8)
        }
        Some("probe") => {
            let rest: Vec<String> = args.collect();
            ExitCode::from(probe::run(&rest) as u8)
        }
        Some("verify") => {
            let rest: Vec<String> = args.collect();
            ExitCode::from(verify::run(&rest) as u8)
        }
        Some("extract") => {
            let rest: Vec<String> = args.collect();
            ExitCode::from(z2_assets::assets_bin_io::extract_main(&rest) as u8)
        }
        Some("hdpack") => {
            let rest: Vec<String> = args.collect();
            ExitCode::from(hdpack::run(&rest) as u8)
        }
        Some("fuzz") => {
            let rest: Vec<String> = args.collect();
            ExitCode::from(fuzz::run(&rest) as u8)
        }
        Some("corpus" | "movie") => {
            let rest: Vec<String> = args.collect();
            ExitCode::from(corpus::run(&rest) as u8)
        }
        Some("facts") => {
            eprintln!("xtask facts: not yet implemented");
            ExitCode::from(2)
        }
        Some(other) => {
            eprintln!("xtask: unknown subcommand '{other}'\n\n{}", usage());
            ExitCode::from(2)
        }
    }
}
