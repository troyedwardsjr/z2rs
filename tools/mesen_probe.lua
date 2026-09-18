-- Second-opinion RAM probe for the lockstep oracle.
--
-- Runs under Mesen 2 headless testrunner:
--   Mesen --testrunner zelda2.nes tools/mesen_probe.lua
-- Mesen loads the game, runs this script while emulating at maximum speed,
-- and exits with this script's emu.stop() code (0 = probe completed).
--
-- What it does: with NO input (attract/title path), it hashes CPU RAM
-- $0000-$07FF and WRAM $6000-$7FFF at the end of every 60th frame for 600
-- frames and logs the hashes. Compare the log against the tetanes oracle's
-- boot/title hashes. Any mismatch on frame 0 means the
-- two emulators disagree about power-on state; a later mismatch means they
-- diverge during boot/title — either way, the tetanes side is suspect and
-- must be investigated before lockstep results are trusted.
--
-- API notes (Mesen 2 Lua, see mesen.ca/docs/apireference.html):
-- emu.read(address) defaults to main CPU memory; emu.addEventCallback with
-- emu.eventType.endFrame fires once per frame; emu.log writes to the
-- testrunner console; emu.stop(code) ends the run. If your Mesen build names
-- the frame event differently, adjust the single addEventCallback line below.

local FRAMES = 600
local LOG_EVERY = 60

local frame = 0
local bootHash = nil

-- FNV-1a 32-bit over a byte reader (kept in double precision; exact for
-- 32-bit values in Lua's 64-bit floats only up to 2^53 — we mask to 32 bits
-- every step, so this stays exact).
local function fnv1a(bytes)
  local h = 2166136261
  for i = 1, #bytes do
    h = (h ~ bytes[i]) & 0xFFFFFFFF
    h = (h * 16777619) & 0xFFFFFFFF
  end
  return h
end

local function readRange(first, last)
  local out = {}
  for a = first, last do
    out[#out + 1] = emu.read(a) & 0xFF
  end
  return out
end

local function probe()
  local ram = readRange(0x0000, 0x07FF)
  local wram = readRange(0x6000, 0x7FFF)
  local hRam = fnv1a(ram)
  local hWram = fnv1a(wram)
  if bootHash == nil then
    bootHash = string.format("ram=%08X wram=%08X", hRam, hWram)
    emu.log("probe boot: " .. bootHash)
  end
  return hRam, hWram
end

emu.addEventCallback(function()
  frame = frame + 1
  if frame % LOG_EVERY == 0 or frame == 1 then
    local hRam, hWram = probe()
    emu.log(string.format("probe frame %d: ram=%08X wram=%08X", frame, hRam, hWram))
  end
  if frame >= FRAMES then
    emu.log(string.format("probe done: %d frames, boot %s", FRAMES, bootHash))
    emu.stop(0)
  end
end, emu.eventType.endFrame)
