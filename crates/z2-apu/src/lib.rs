//! `z2-apu`: NES APU synth + Zelda II bank 6 sound-engine port.
//!
//! Dependency-free, `std`-only, `wasm32`-clean. No ROM or table bytes live
//! here: note/envelope data is loaded from the assets image at runtime
//! through [`engine::NoteTables`].
//!
//! ## Modules
//!
//! * [`tables`] — NES hardware tables (length, duty, noise/DMC periods).
//! * [`pulse`], [`triangle`], [`noise`], [`dmc`] — the five voices.
//! * [`frame`] — 4/5-step frame counter.
//! * [`apu`] — [`Apu`](apu::Apu): register file, nonlinear mixer, resampler.
//! * [`engine`] — bank 6 sequencer port (request dispatch + voices).
//! * [`reglog`] — `$4000-$4017` write-log recorder + oracle comparison.
//! * [`audio`] — frontend FIFO sizing + underrun counter.
//!
//! ## Wiring guide (`Game` integration)
//!
//! ```ignore
//! use z2_apu::{Apu, Engine, SoundRequest, RegLog, AssetsTables};
//!
//! // Once at startup:
//! let mut apu = Apu::new(44_100);
//! apu.install_dmc_source(Box::new(AssetsDmc::new(&assets)));
//! let mut engine = Engine::new();
//! let tables = AssetsTables::new(&assets); // impls NoteTables, no copy
//! let mut log = RegLog::new();
//!
//! // Once per emulated video frame (after the CPU/RAM step):
//! let req = SoundRequest {
//!     global_on: ram[0xEA] == 0,
//!     eb: ram[0xEB], ec: ram[0xEC], ed: ram[0xED],
//!     ee: ram[0xEE], ef: ram[0xEF], world: ram[0x0707],
//! };
//! engine.tick_game_frame(&req, &tables, &mut apu, &mut log);
//! // Title screen instead uses tick_title_frame + start_title_song.
//!
//! // Game audio API shape — 44100 Hz PCM appended to `out`:
//! fn audio(&mut self, out: &mut Vec<i16>) { self.apu.audio(out); }
//! ```
//!
//! Oracle verification: hand `log` to
//! [`reglog::compare`] against the lockstep oracle's register log over
//! the warpless movie, followed by a listening pass.
//!
//! ## Legal
//!
//! Reference: `third_party/z2disassembly` (read-only). Pinned ROM identity
//! (header-stripped body): CRC32 `BA322865`, SHA1
//! `11333adb723a5975e0ecca3aee8f4747aa8d2d26`. Never commit ROMs, movies,
//! snapshots, or table bytes.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod apu;
pub mod audio;
pub mod dmc;
pub mod engine;
pub mod frame;
pub mod noise;
pub mod pulse;
pub mod reglog;
pub mod tables;
pub mod triangle;

pub use apu::{Apu, CPU_HZ, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE, OUTPUT_GAIN};
pub use audio::{nominal_frame_samples, two_frame_buffer_samples};
pub use audio::{PcmFifo, BUFFER_SLACK_SAMPLES, RATE_44100, RATE_48000};
pub use dmc::{Dmc, DmcSource, PrgSource, SilentSource, SliceSource};
pub use engine::{
    Engine, EngineReport, NoteTables, NullTables, SoundRequest, TrackSlot, Voice, World,
};
pub use engine::{PortState, SfxBus, StreamEvent, VoiceSeq};
pub use frame::{FrameCounter, FRAME_CPU_CYCLES};
pub use noise::Noise;
pub use pulse::Pulse;
pub use reglog::{compare, RegCompare, RegLog, RegWrite};
pub use tables::{DMC_RATE, LENGTH_TABLE, NOISE_PERIOD, PULSE_DUTY, TRIANGLE_SEQ};
pub use triangle::Triangle;
