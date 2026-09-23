# z2rs developer shortcuts.
#
# Portable macOS / Linux (`sh` only). Run from the worktree root.
#
# ROM policy (see LEGAL.md): the Zelda II (USA) ROM is never committed and
# never copied into the tree. ROM-gated targets read it read-only via
# $Z2_ROM. Without $Z2_ROM they print guidance and exit 2 — never fail
# cryptically.
#
# Out-of-tree corpus (movies/snapshots, see README.md): $Z2_CORPUS
# (default: the sibling ../z2-corpus checkout, override as needed).

.DEFAULT_GOAL := help
.PHONY: hd-sheets hd-pack hd-play help build test test-rom verify-smoke extract fuzz-smoke run run-debug run-release run-web run-web-net signal check-net netplay-e2e netplay-e2e-rollback netplay-play netplay-e2e-setup corpus-mint fmt clippy clean

# Out-of-tree corpus checkout (movies + minted snapshots live here, never in
# this repo). Override: `make corpus-mint Z2_CORPUS=/path/to/corpus`.
Z2_CORPUS ?= $(CURDIR)/../z2-corpus
# Native run inputs. `make run ROM=/path/to/zelda2.nes [MOVIE=/path/to/x.bk2]`;
# ROM defaults to $Z2_ROM, MOVIE defaults to empty (live input).
ROM ?= $(Z2_ROM)
MOVIE ?=
# Extra flags passed through to the native app, e.g.
#   make run ARGS="--widescreen 16:9 --coop-local"
ARGS ?=
# HD pack spritesheets (`make hd-sheets`, `make hd-pack`, `make hd-play`). All
# three directories hold art drawn from your ROM, so they default to a folder
# outside the repository (LEGAL.md); the tools refuse a path inside it.
HD_ART ?= $(HOME)/z2-art
HD_SHOTS ?= $(HD_ART)/captures
HD_SHEETS ?= $(HD_ART)/sheets
HD_PACK ?= $(HD_ART)/my-pack
HD_SCALE ?= 4
# Signalling server bind address for `make signal`.
SIGNAL_BIND ?= 0.0.0.0:3536
# Player-1 input track `make netplay-e2e` replays through the live session to
# reach a side-view area (the only place player 2 exists). Out-of-tree, like
# every other movie: `make netplay-e2e Z2_MOVIE=/path/to/track.fm2`.
Z2_MOVIE ?=
NETPLAY_E2E_MOVIE ?= $(if $(Z2_MOVIE),$(Z2_MOVIE),$(Z2_CORPUS)/movies/warpless.fm2)
# Fuzz smoke input snapshot (minted via `make corpus-mint`).
SNAPSHOT ?= $(Z2_SNAPSHOT)
# Web server port.
PORT ?= 8080

help:
	@echo "z2rs — Zelda II reconstruction (see README.md; ROM policy: LEGAL.md)"
	@echo ""
	@echo "usage: make <target> [ROM=path] [MOVIE=path] [SNAPSHOT=path]"
	@echo ""
	@echo "  build        cargo build --workspace (no ROM needed)"
	@echo "  test         cargo test --workspace (no ROM needed; ROM-gated tests skip)"
	@echo "  test-rom     cargo test --workspace with ROM coverage (needs Z2_ROM)"
	@echo "  verify-smoke 30-frame oracle self-check + informational game check (needs Z2_ROM)"
	@echo "  extract      extract assets.bin from ROM to a temp file (needs Z2_ROM)"
	@echo "  fuzz-smoke   5-seed x 60-frame divergence fuzz (needs Z2_ROM + SNAPSHOT=)"
	@echo "  run          native windowed app, optimized build (needs ROM)"
	@echo "  run-debug    native debug build for interpreter debugging (slow)"
	@echo "  run-release  alias target for the optimized play build"
	@echo "  run-web      wasm-pack build + serve crates/z2-web/site on PORT (needs wasm-pack, python3)"
	@echo "  run-web-net  as run-web but with HD packs + online co-op (--features hd,netplay)"
	@echo "  signal       run the netplay signalling server on SIGNAL_BIND (default 0.0.0.0:3536)"
	@echo "  check-net    z2-net tests + both wasm builds (default and --features netplay)"
	@echo "  netplay-e2e  two real browser windows play online co-op; proves input crosses the"
	@echo "               wire and both stay in lockstep (needs Z2_ROM + a movie + setup below)"
	@echo "  netplay-e2e-rollback  the same for rollback (default mode) over a simulated slow,"
	@echo "               lossy link: rollbacks happen, local input lands after input_delay"
	@echo "               frames, confirmed state hashes agree; reports wasm costs"
	@echo "  netplay-play two visible browser windows already in a rollback session (left P1,"
	@echo "               right P2) for playing by hand (needs Z2_ROM + setup below)"
	@echo "  netplay-e2e-setup  one-off: npm install playwright + download chromium (dev only)"
	@echo "  corpus-mint  delegate snapshot minting to xtask (needs Z2_CORPUS + Z2_ROM)"
	@echo "  hd-sheets    capture the whole game and write paintable spritesheets to HD_SHEETS:"
	@echo "               every sprite pose and every scene's tileset (needs Z2_ROM + Z2_CORPUS)"
	@echo "  hd-pack      cut the painted HD_SHEETS into an HD pack in HD_PACK and validate it"
	@echo "  hd-play      play with HD_PACK loaded (16:9 widescreen, HD_SCALE)"
	@echo "  fmt          cargo fmt --all"
	@echo "  clippy       cargo clippy --workspace --all-targets -- -D warnings"
	@echo "  clean        cargo clean"
	@echo "  help         this text (default)"
	@echo ""
	@echo "env: Z2_ROM=$${Z2_ROM:-<unset>}  Z2_CORPUS=$(Z2_CORPUS)"
	@echo "     Z2_SNAPSHOT=/path/to/label.z2snap (for fuzz-smoke)"

build:
	cargo build --workspace

test:
	cargo test --workspace

test-rom:
	@[ -n "$(Z2_ROM)" ] || { echo "test-rom: Z2_ROM is not set."; echo "  Set it to your own Zelda II (USA) dump, e.g.:"; echo "    export Z2_ROM=/path/to/zelda2.nes   (see LEGAL.md)"; exit 2; }
	@[ -f "$(Z2_ROM)" ] || { echo "test-rom: Z2_ROM='$(Z2_ROM)' is not a file."; exit 2; }
	cargo test --workspace

verify-smoke:
	@[ -n "$(Z2_ROM)" ] || { echo "verify-smoke: Z2_ROM is not set."; echo "  Set it to your own Zelda II (USA) dump, e.g.:"; echo "    export Z2_ROM=/path/to/zelda2.nes   (see LEGAL.md)"; exit 2; }
	@[ -f "$(Z2_ROM)" ] || { echo "verify-smoke: Z2_ROM='$(Z2_ROM)' is not a file."; exit 2; }
	@echo "== oracle self-check: 30 blank-input frames (must pass) =="
	cargo xtask verify --frames 30 --dut oracle --rom "$(Z2_ROM)"
	@echo "== game check: 30 blank-input frames (informational) =="
	@cargo xtask verify --frames 30 --dut game --rom "$(Z2_ROM)"; st=$$?; \
	if [ $$st -eq 0 ]; then \
		echo "verify-smoke: game matches oracle for 30 frames (boot frontier closed!)"; \
	elif [ $$st -eq 1 ]; then \
		echo "verify-smoke: game diverges (run 'cargo xtask verify --continue' for details); pipeline OK"; \
	else exit $$st; fi

extract:
	@[ -n "$(Z2_ROM)" ] || { echo "extract: Z2_ROM is not set."; echo "  Set it to your own Zelda II (USA) dump, e.g.:"; echo "    export Z2_ROM=/path/to/zelda2.nes   (see LEGAL.md)"; exit 2; }
	@[ -f "$(Z2_ROM)" ] || { echo "extract: Z2_ROM='$(Z2_ROM)' is not a file."; exit 2; }
	cargo xtask extract --rom "$(Z2_ROM)" --out "$${TMPDIR:-/tmp}/z2-assets-smoke.bin"

fuzz-smoke:
	@[ -n "$(Z2_ROM)" ] || { echo "fuzz-smoke: Z2_ROM is not set."; echo "  Set it to your own Zelda II (USA) dump, e.g.:"; echo "    export Z2_ROM=/path/to/zelda2.nes   (see LEGAL.md)"; exit 2; }
	@[ -f "$(Z2_ROM)" ] || { echo "fuzz-smoke: Z2_ROM='$(Z2_ROM)' is not a file."; exit 2; }
	@[ -n "$(SNAPSHOT)" ] || { echo "fuzz-smoke: no snapshot given."; echo "  Mint one first (make corpus-mint) then pass it:"; echo "    make fuzz-smoke SNAPSHOT=\$$Z2_CORPUS/snapshots/<label>.z2snap"; echo "  or export Z2_SNAPSHOT."; exit 2; }
	@[ -f "$(SNAPSHOT)" ] || { echo "fuzz-smoke: SNAPSHOT='$(SNAPSHOT)' is not a file."; exit 2; }
	cargo xtask fuzz --snapshot "$(SNAPSHOT)" --seeds 5 --frames 60 --dut game --rom "$(Z2_ROM)"

run:
	$(MAKE) run-release ROM="$(ROM)" MOVIE="$(MOVIE)"

run-debug:
	@[ -n "$(ROM)" ] || { echo "run-debug: no ROM given."; echo "  Set Z2_ROM or pass ROM=, e.g.:"; echo "    make run-debug ROM=/path/to/zelda2.nes [MOVIE=/path/to/movie.bk2]"; echo "  (see LEGAL.md; without a ROM the app starts in synthetic no-cartridge mode via:)"; echo "    cargo run -p z2-native"; exit 2; }
	@[ -f "$(ROM)" ] || { echo "run-debug: ROM='$(ROM)' is not a file."; exit 2; }
	@if [ -n "$(MOVIE)" ]; then \
		[ -f "$(MOVIE)" ] || { echo "run-debug: MOVIE='$(MOVIE)' is not a file."; exit 2; }; \
		cargo run -p z2-native -- --rom "$(ROM)" --movie "$(MOVIE)"; \
	else \
		cargo run -p z2-native -- --rom "$(ROM)"; \
	fi

# Optimized play build. The debug interpreter is intentionally retained as
# run-debug for interpreter work, but run should be the fast play path.
run-release:
	@[ -n "$(ROM)" ] || { echo "run-release: no ROM given (see make run)."; exit 2; }
	@[ -f "$(ROM)" ] || { echo "run-release: ROM='$(ROM)' is not a file."; exit 2; }
	@if [ -n "$(MOVIE)" ]; then \
		[ -f "$(MOVIE)" ] || { echo "run-release: MOVIE='$(MOVIE)' is not a file."; exit 2; }; \
		cargo run --release -p z2-native -- --rom "$(ROM)" --movie "$(MOVIE)" $(ARGS); \
	else \
		cargo run --release -p z2-native -- --rom "$(ROM)" $(ARGS); \
	fi

# Netplay signalling server. Rooms are ws://HOST:3536/z2-<room>, two peers each.
# No TLS: put it behind a reverse proxy for wss:// (browsers on https need it).
signal:
	cargo run --release -p z2-signal -- --bind "$(SIGNAL_BIND)"

check-net:
	cargo test -p z2-net
	cargo build -p z2-web --target wasm32-unknown-unknown
	cargo build -p z2-web --target wasm32-unknown-unknown --features hd
	cargo build -p z2-web --target wasm32-unknown-unknown --features netplay
	cargo build -p z2-web --target wasm32-unknown-unknown --features hd,netplay

run-web:
	@command -v wasm-pack >/dev/null 2>&1 || { echo "run-web: wasm-pack not found."; echo "  Install it (https://rustwasm.github.io/wasm-pack/installer/), then retry."; echo "  Fallback wasm-clean check: cargo build -p z2-web --target wasm32-unknown-unknown"; exit 2; }
	@command -v python3 >/dev/null 2>&1 || { echo "run-web: python3 not found (needed to serve the static site)."; exit 2; }
	wasm-pack build crates/z2-web --target web --out-dir site/pkg
	@echo "serving crates/z2-web/site on http://localhost:$(PORT)/ (Ctrl-C to stop)"
	@echo "open the page, then drop your Zelda II (USA) .nes (hash-gated in-tab; never uploaded)"
	cd crates/z2-web/site && python3 -m http.server "$(PORT)"

# Same as run-web plus online co-op. Kept separate so the default bundle stays
# as small as it is today.
run-web-net:
	@command -v wasm-pack >/dev/null 2>&1 || { echo "run-web-net: wasm-pack not found."; exit 2; }
	@command -v python3 >/dev/null 2>&1 || { echo "run-web-net: python3 not found."; exit 2; }
	wasm-pack build crates/z2-web --target web --out-dir site/pkg -- --features hd,netplay
	@echo "serving crates/z2-web/site on http://localhost:$(PORT)/ (Ctrl-C to stop)"
	@echo "start the signalling server too: make signal"
	cd crates/z2-web/site && python3 -m http.server "$(PORT)"

# One-off developer bootstrap. Playwright is a TEST tool only: the site itself
# still has no npm dependency (index.html / app.js / worklet.js import nothing
# from node_modules, and run-web serves with python3). node_modules/ is ignored.
netplay-e2e-setup:
	@command -v npm >/dev/null 2>&1 || { echo "netplay-e2e-setup: npm not found."; echo "  Install Node.js 18+ (https://nodejs.org), then retry."; exit 2; }
	npm install
	npx playwright install chromium
	@echo "ready — now: make netplay-e2e"

# Online co-op, end to end, in two real browser windows. First the page's own
# live path must connect Host + Join within 10 s (Z2_E2E_CONNECT_MS) while the
# game keeps running, Leave must cancel, and a browser must connect to the
# ROM-free native peer (examples/net_peer). Then the STRONG
# claim: a key held in one window moves the matching player in the OTHER
# window's game state (and reverses on the opposite key), and both windows
# finish on the same frame with the same lockstep state hash and zero desyncs.
# Screenshots + a summary.json land in a scratch dir outside the repository.
netplay-e2e:
	@[ -n "$(Z2_ROM)" ] || { echo "netplay-e2e: Z2_ROM is not set."; echo "  Set it to your own Zelda II (USA) dump, e.g.:"; echo "    export Z2_ROM=/path/to/zelda2.nes   (see LEGAL.md)"; exit 2; }
	@[ -f "$(Z2_ROM)" ] || { echo "netplay-e2e: Z2_ROM='$(Z2_ROM)' is not a file."; exit 2; }
	@command -v node >/dev/null 2>&1 || { echo "netplay-e2e: node not found."; echo "  Install Node.js 18+ (https://nodejs.org), then: make netplay-e2e-setup"; exit 2; }
	@node -e "require.resolve('playwright')" >/dev/null 2>&1 || { echo "netplay-e2e: the playwright npm module is not installed."; echo "  One-off bootstrap (dev tooling only, never committed):"; echo "    make netplay-e2e-setup"; exit 2; }
	@command -v wasm-pack >/dev/null 2>&1 || { echo "netplay-e2e: wasm-pack not found."; echo "  Install it (https://rustwasm.github.io/wasm-pack/installer/), then retry."; exit 2; }
	@[ -f "$(NETPLAY_E2E_MOVIE)" ] || { echo "netplay-e2e: no movie at '$(NETPLAY_E2E_MOVIE)'."; echo "  The run replays a player-1 input track through the live session to walk"; echo "  into a side-view area, which is the only place player 2 exists"; echo "  (README.md). Fetch the out-of-tree corpus (README.md), or:"; echo "    make netplay-e2e Z2_MOVIE=/path/to/track.fm2"; echo "  It must be .fm2 text; .bk2 is a ZIP and is rejected by design."; exit 2; }
	wasm-pack build crates/z2-web --target web --out-dir site/pkg -- --features hd,netplay
	cargo build --release -p z2-signal
	cargo build --release -p z2-net --features matchbox --example net_peer
	Z2_MOVIE="$(NETPLAY_E2E_MOVIE)" node crates/z2-web/site/netplay-e2e.mjs

# Rollback netplay, end to end, in two browser windows over a simulated bad
# link (z2.ext.net.simulate: Z2_SIM_LATENCY_MS/JITTER_MS/LOSS_PCT, default
# 50 ms +-20 ms and 5% loss each way). Reports wasm save/load/frame/tick costs,
# connects through the live page in rollback mode, then proves rollbacks
# happen in both windows, a local key reaches the local simulation after
# exactly input_delay frames, input crosses the wire both ways, and both
# windows agree on every confirmed-frame state hash with zero desyncs.
netplay-e2e-rollback:
	@[ -n "$(Z2_ROM)" ] || { echo "netplay-e2e-rollback: Z2_ROM is not set."; echo "  Set it to your own Zelda II (USA) dump, e.g.:"; echo "    export Z2_ROM=/path/to/zelda2.nes   (see LEGAL.md)"; exit 2; }
	@[ -f "$(Z2_ROM)" ] || { echo "netplay-e2e-rollback: Z2_ROM='$(Z2_ROM)' is not a file."; exit 2; }
	@command -v node >/dev/null 2>&1 || { echo "netplay-e2e-rollback: node not found."; echo "  Install Node.js 18+ (https://nodejs.org), then: make netplay-e2e-setup"; exit 2; }
	@node -e "require.resolve('playwright')" >/dev/null 2>&1 || { echo "netplay-e2e-rollback: the playwright npm module is not installed."; echo "    make netplay-e2e-setup"; exit 2; }
	@command -v wasm-pack >/dev/null 2>&1 || { echo "netplay-e2e-rollback: wasm-pack not found."; echo "  Install it (https://rustwasm.github.io/wasm-pack/installer/), then retry."; exit 2; }
	@[ -f "$(NETPLAY_E2E_MOVIE)" ] || { echo "netplay-e2e-rollback: no movie at '$(NETPLAY_E2E_MOVIE)'."; echo "  Fetch the out-of-tree corpus (README.md), or:"; echo "    make netplay-e2e-rollback Z2_MOVIE=/path/to/track.fm2"; exit 2; }
	wasm-pack build crates/z2-web --target web --out-dir site/pkg -- --features hd,netplay
	cargo build --release -p z2-signal
	Z2_MOVIE="$(NETPLAY_E2E_MOVIE)" node crates/z2-web/site/netplay-e2e-rollback.mjs

# Two visible browser windows side by side, already in one rollback session
# (left hosts = player 1, right joins = player 2, ICE none), with $Z2_ROM
# loaded. Play with the keyboard; closing both windows or Ctrl-C stops the
# signal server and the static servers.
netplay-play:
	@[ -n "$(Z2_ROM)" ] || { echo "netplay-play: Z2_ROM is not set."; echo "  Set it to your own Zelda II (USA) dump, e.g.:"; echo "    export Z2_ROM=/path/to/zelda2.nes   (see LEGAL.md)"; exit 2; }
	@[ -f "$(Z2_ROM)" ] || { echo "netplay-play: Z2_ROM='$(Z2_ROM)' is not a file."; exit 2; }
	@command -v node >/dev/null 2>&1 || { echo "netplay-play: node not found."; echo "  Install Node.js 18+ (https://nodejs.org), then: make netplay-e2e-setup"; exit 2; }
	@node -e "require.resolve('playwright')" >/dev/null 2>&1 || { echo "netplay-play: the playwright npm module is not installed."; echo "  One-off bootstrap (dev tooling only, never committed):"; echo "    make netplay-e2e-setup"; exit 2; }
	@command -v wasm-pack >/dev/null 2>&1 || { echo "netplay-play: wasm-pack not found."; echo "  Install it (https://rustwasm.github.io/wasm-pack/installer/), then retry."; exit 2; }
	wasm-pack build crates/z2-web --target web --out-dir site/pkg -- --features hd,netplay
	cargo build --release -p z2-signal
	node crates/z2-web/site/netplay-play.mjs

corpus-mint:
	@[ -d "$(Z2_CORPUS)" ] || { echo "corpus-mint: Z2_CORPUS='$(Z2_CORPUS)' is not a directory."; echo "  Check out (or point at) the out-of-tree corpus, e.g.:"; echo "    make corpus-mint Z2_CORPUS=/path/to/z2-corpus   (see README.md)"; echo "  It must contain corpus/movies/ (.fm2/.bk2 fetched per README.md);"; echo "  snapshots land in corpus/snapshots/ and are never committed."; exit 2; }
	@[ -n "$(Z2_ROM)" ] || { echo "corpus-mint: Z2_ROM is not set (minting replays the oracle against your ROM)."; echo "    export Z2_ROM=/path/to/zelda2.nes   (see LEGAL.md)"; exit 2; }
	@echo "minting snapshots from $(Z2_CORPUS)/movies into $(Z2_CORPUS)/snapshots (see README.md)"
	Z2_CORPUS="$(Z2_CORPUS)" cargo xtask corpus mint

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

# --- HD pack spritesheets (tools/hd-sheets/paint_sheets.py) ---
# hd-sheets replays the corpus movie into every town, palace, boss and field,
# records which CHR tile drew each pixel, and lays the result out as sheets:
# one per sprite palette (whole figures, one slot per animation frame) and one
# tileset per scene. Paint over them, then hd-pack slices them back into a pack.
HD_SHOTS_BIN = cargo run --release -q -p z2-native --example article_shots --

hd-sheets:
	@[ -n "$(Z2_ROM)" ] || { echo "hd-sheets: Z2_ROM is not set."; echo "  Set it to your own Zelda II (USA) dump, e.g.:"; echo "    export Z2_ROM=/path/to/zelda2.nes   (see LEGAL.md)"; exit 2; }
	@[ -f "$(Z2_CORPUS)/movies/anypct.bk2" ] || { echo "hd-sheets: no $(Z2_CORPUS)/movies/anypct.bk2."; echo "  The captures replay the corpus movies; point Z2_CORPUS at the directory holding movies/anypct.bk2."; exit 2; }
	python3 tools/hd-sheets/specs.py "$(HD_SHOTS)/specs"
	$(HD_SHOTS_BIN) --movie "$(Z2_CORPUS)/movies/anypct.bk2" --spec "$(HD_SHOTS)/specs/sheets.spec" --out "$(HD_SHOTS)/moves" --scale 1 --tilemap
	$(HD_SHOTS_BIN) --movie "$(Z2_CORPUS)/movies/anypct.bk2" --spec "$(HD_SHOTS)/specs/scenes.spec" --out "$(HD_SHOTS)/scenes" --scale 1 --tilemap
	$(HD_SHOTS_BIN) --movie "$(Z2_CORPUS)/movies/anypct.bk2" --spec "$(HD_SHOTS)/specs/special.spec" --out "$(HD_SHOTS)/special" --scale 1 --tilemap
	@if [ -f "$(Z2_CORPUS)/movies/hundred-percent.bk2" ]; then \
		$(HD_SHOTS_BIN) --movie "$(Z2_CORPUS)/movies/hundred-percent.bk2" --spec "$(HD_SHOTS)/specs/m100.spec" --out "$(HD_SHOTS)/m100" --scale 1 --tilemap; \
	else echo "hd-sheets: no hundred-percent.bk2, skipping the title/name-entry/game-over captures"; fi
	$(HD_SHOTS_BIN) --spec "$(HD_SHOTS)/specs/title.spec" --out "$(HD_SHOTS)/title" --scale 1 --tilemap
	python3 tools/hd-sheets/paint_sheets.py build --out "$(HD_SHEETS)" --scale $(HD_SCALE) \
		--shots "$(HD_SHOTS)/moves" "$(HD_SHOTS)/scenes" "$(HD_SHOTS)/special" "$(HD_SHOTS)/m100" "$(HD_SHOTS)/title"
	@echo ""
	@echo "hd-sheets: paint over $(HD_SHEETS)/sheets/*.png, then run: make hd-pack"

hd-pack:
	@[ -f "$(HD_SHEETS)/sheets.json" ] || { echo "hd-pack: no $(HD_SHEETS)/sheets.json; run make hd-sheets first"; exit 2; }
	python3 tools/hd-sheets/paint_sheets.py cut --sheets "$(HD_SHEETS)" --out "$(HD_PACK)" --name "$${HD_PACK_NAME:-My HD pack}"
	cargo xtask hdpack check --pack "$(HD_PACK)"

hd-play:
	@[ -n "$(ROM)" ] || { echo "hd-play: no ROM (set Z2_ROM or ROM=)"; exit 2; }
	@[ -f "$(HD_PACK)/pack.json" ] || { echo "hd-play: no $(HD_PACK)/pack.json; run make hd-pack first"; exit 2; }
	cargo run --release -p z2-native -- --rom "$(ROM)" --hd-pack "$(HD_PACK)" --hd-scale $(HD_SCALE) --widescreen 16:9 $(ARGS)

clean:
	cargo clean
