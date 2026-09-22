# Co-op and widescreen

z2rs can run two Links, locally or over the internet, and it can paint extra scenery beside the NES picture. Both are off unless you ask for them, and neither touches the verification path. The game only sees a second controller when you turn co-op on.

```sh
# two players, one machine, 16:9
cargo run --release -p z2-native -- --rom "$Z2_ROM" --coop-local --widescreen 16:9

# online: one signalling server, one host, one guest
cargo run --release -p z2-signal                                     # machine S
cargo run --release -p z2-native -- --rom "$Z2_ROM" --coop-host myroom --signal ws://S:3536
cargo run --release -p z2-native -- --rom "$Z2_ROM" --coop-join myroom --signal ws://S:3536
```

## How widescreen works

The NES draws 256 pixels across because that is all the hardware has: two screens of background map, scrolled. z2rs does not stretch that picture. While the software PPU renders a frame, it records what it drew on every scanline: which tile, from which CHR page, in which palette, at which scroll position. After the frame ends, z2rs decodes the extra columns straight from the game's own memory. Side-view areas come from the level RAM the game builds at `$6000`, and the overworld comes from the run-length map it keeps in WRAM at `$7C00`. Those tiles go into the margins with the palette that scanline was using, so the scenery continues instead of repeating or blurring.

The centre 256 columns are copied byte for byte from the normal frame. That copy is what keeps widescreen out of the verification path, and the game never learns the screen got wider.

Widescreen also recovers two black strips. The game blanks its leftmost 8 pixels with a mask bit, and on the overworld it hides its rightmost 8 pixels behind a column of opaque black sprites. z2rs repaints both from the same tile data, so the picture runs edge to edge without the seams the original hardware needed.

## How co-op works

Nobody wrote a second character. Every frame, the game's own player update and draw routines run a second time with player 2's state swapped into the memory addresses the original code reads, and the state is swapped back out afterwards. Player 2 gets the real physics, sword collision and damage handling, because it is the ROM's own code running twice.

The camera follows player 1, and player 2 is clamped into the visible screen so you cannot strand each other. The clamp does no collision check, so a fast-scrolling player 1 can drag player 2 through solid tiles.

Player 2 only exists in side-view areas. On the overworld there is no player 2: player 1 walks the map, and player 2 reappears beside them in the next area. The overworld runs a different driver built around a single map position, and entering and leaving areas is global state, so giving each player a map position would be a much larger change. The same goes for the title screen, file select, area loads and the death screen. While player 2 is hidden, the window title says `[coop P2 hidden]`.

The swap restores every byte it touched, so with co-op off the original game is byte for byte identical. That was checked against the reference emulator across thousands of frames from three different TAS runs.

## How online play works

Online play uses rollback by default, on the desktop and in the browser. Input-delay lockstep is still available with `--net-mode lockstep`, the Mode select in the page, or `?net=lockstep`.

With rollback, your own input applies after a short delay (2 frames by default) and the other player's input is predicted until it arrives. When a prediction turns out wrong, z2rs re-simulates the frames since then from a saved state, within the same tick.

Both machines run the same build and the same frame numbers. Only controller bytes cross the wire, never positions or sprites. The host is player 1. Each tick, a peer sends its input for a frame a few frames ahead (`--net-delay`, default 2) so the packet has time to arrive before that frame is stepped. Inputs are acknowledged and resent, with a reliable channel behind the unreliable one, so a dropped packet causes a stall and not a desync. Every 60 frames each side hashes RAM, WRAM and OAM and compares the result with the other side, so a divergence gets reported instead of going unnoticed.

The transport is WebRTC data channels through matchbox. The desktop app and the browser tab use the same path and can play each other. `z2-signal` is the introduction service. It allows two peers per room and refuses a third, and it only carries the handshake. After that, traffic is peer to peer.

There is no late join. The guest receives the host's save memory when the session starts and both sides reset together. If the ROM hash, protocol version or feature flags differ, the handshake refuses to start.

## Limits

These apply to both the desktop app and the browser build.

Widescreen:

- The margins hold no sprites, so enemies and projectiles still appear at the original screen edge.
- Dialogue boxes and the pause and spell pane do not extend into the margins, and title and menu screens leave them blank.
- Where the level data runs out (the west end of a town, for instance) the margin falls back to the backdrop colour.
- The overworld's own edge blanking (`PPUMASK $18` on the left, a column of opaque black sprites on the right) is repainted by `--fill-left-clip` and `--fill-right-clip`, both on by default. With an HD pack loaded, the right-hand seam remains.

Co-op:

- Player 2 exists in side-view areas only, not on the overworld.
- Player 2 cannot use doors, elevators, NPCs, shops or spells.
- Enemies target player 1.
- There is one save and one set of stats.

Online play:

- There is no late join. Both peers restart from power-on with the host's save.
- Pause, fast-forward, save states and movies are disabled during a session, and a hash mismatch ends it.
- The guest autosaves to `sram-coop.sav` and never over its own `sram.sav`.
- The signalling server has no TLS and no authentication. Put it behind a reverse proxy for `wss://` (browsers on an https page require it) and use a room name nobody can guess, because anyone who knows the name can take the free slot.
- WebRTC reveals each peer's public IP address to the other peer and to the STUN server. Symmetric NATs need a TURN server (`netplay.ice_*` in the config).
