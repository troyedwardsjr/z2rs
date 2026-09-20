# Contributing to z2rs

## Setup

1. Install Rust (stable) and add the WebAssembly target: `rustup target add wasm32-unknown-unknown`.
2. Turn on the ROM guard hook. It is mandatory, and it refuses commits that contain ROM-derived files: `git config core.hooksPath .githooks`.
3. Do not copy a ROM into the repository. To run the tools that need one, point `Z2_ROM` at your own dump: `export Z2_ROM=/path/to/your/zelda2.nes`.
4. The disassembly used as a porting reference is a git submodule. `cargo xtask ledger` needs it: `git submodule update --init third_party/z2disassembly`.

## Read the legal policy first

Read [LEGAL.md](LEGAL.md) before anything else. The short version: never commit ROM bytes, fixtures derived from the ROM, or leaked source. Describe what the game does. Do not transcribe how the original code expresses it.

## Ground rules

Keep every crate building for the host and for `wasm32-unknown-unknown`. A library crate may not take an OS-only or native-only dependency unless it has a portable fallback.

`cargo fmt --check`, `cargo clippy`, `cargo test` without a ROM, and the `z2-web` wasm build must all pass.

If you add a data file, a table of constants or a new reference source, record where it came from in [PROVENANCE.md](PROVENANCE.md) in the same pull request.

## Tests that need a ROM, movies or snapshots

These tests skip themselves when their input is missing. They ask a helper for the path and return early when there is none, so `cargo test --workspace` passes with or without a ROM.

A variable such as `Z2_ROM`, `Z2_CORPUS`, `Z2_MOVIES` or `Z2_ORACLE_SRAM` counts as absent when it is unset, when it is empty, or when it does not name an existing file or directory. Checking `env::var(..).is_err()` alone is a bug. With that check, an exported but empty `Z2_ROM` reads as "ROM present", and the test panics when it should skip.

Use the test helper in each crate and do not write your own guard. `crates/{z2-core,z2-verify,z2-native}/tests/common/mod.rs` provide `var_present`, `rom_path`, `rom_bytes`, `env_file`, `env_dir`, `env_read_dir`, `corpus_snapshots`, `env_path_file` and `file_present`. Each one prints a single `skipping <test>: <VAR> not set to an existing file` line and returns `None`. Crates without such a helper (`z2-web`, `z2-assets`, `z2-ppu`, `z2-render`) inline the same check. `crates/z2-ppu/tests/golden.rs` shows how. Note that `crates/z2-render/tests/common/mod.rs` holds builders for synthetic fixtures, not these helpers, so the ROM tests in that crate inline the check too.

A test must skip when its inputs are absent, and it must never fail just because they are present. Suppose the path that runs with a ROM is an unconditional `panic!` or `todo!` for unfinished work. That test goes red only for the developer who has a real ROM and a real corpus, which is the wrong person to punish. Mark such a test `#[ignore]`, say why in the reason string, and keep the body. The unfinished work then stays visible in the ignored list and the suite stays green.

Keep `#[ignore]` for tests that cannot pass on demand: tests that are too slow to run by default even with a ROM (full TAS movie replays), benchmarks that only report timings, and the placeholders described above. Never use `#[ignore]` to mean "needs a ROM". The skip helpers do that job. The tests ignored today are:

- `warp_glitch_movie_zero_traps_no_divergence` in `crates/z2-core/tests/interp_traps.rs`, a slow replay of a full movie
- the timing benchmark in `crates/z2-render/tests/bench.rs`
- three placeholders in `crates/z2-core/tests/player_magic_tests.rs` and `crates/z2-core/tests/player_movement_tests.rs`

The skip decides only whether a test runs. When its inputs are present, a test must still assert everything it asserts today. Never weaken an assertion to make one of these tests pass.

## Before you open a pull request

```sh
cargo fmt --all
cargo build --workspace
cargo test --workspace
cargo build -p z2-web --target wasm32-unknown-unknown
```
