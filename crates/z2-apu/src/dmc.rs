//! NES DMC (delta-modulation) channel: sample playback state machine.
//!
//! Modelled per nesdev `APU DMC`: a rate timer (every CPU cycle) drives an
//! output unit that applies the 8 bits of each sample byte to a 7-bit DAC
//! level, while a memory reader refills the 1-byte sample buffer from PRG
//! ROM. Sample bytes come from a [`DmcSource`] so the synth core stays
//! ROM-free: the game wires an assets-backed source at runtime (Zelda II
//! triggers DMC via `prg6.asm` `Bank6__Code_3`, PRG `$924E-$9261`, which
//! programs `$4010/$4012/$4013/$4015`; see [`crate::engine`]).

use crate::tables::DMC_RATE;

/// Source of DMC sample bytes, read at CPU addresses `$C000-$FFFF`.
///
/// The game implementation reads from the assets image (loaded from the
/// ROM at runtime); tests and silent paths use [`SilentSource`] /
/// [`SliceSource`].
pub trait DmcSource {
    /// Read one sample byte from the given CPU address.
    fn read_byte(&mut self, cpu_addr: u16) -> u8;
}

/// DMC source that returns `0x00` for every address (power-on / stub path).
#[derive(Debug, Clone, Copy, Default)]
pub struct SilentSource;

impl DmcSource for SilentSource {
    fn read_byte(&mut self, _cpu_addr: u16) -> u8 {
        0x00
    }
}

/// DMC source backed by a byte slice, for tests.
///
/// `base` is the CPU address of `data[0]`; reads wrap within the slice.
#[derive(Debug, Clone, Copy)]
pub struct SliceSource<'a> {
    /// CPU address corresponding to `data[0]`.
    pub base: u16,
    /// Sample bytes.
    pub data: &'a [u8],
}

impl DmcSource for SliceSource<'_> {
    fn read_byte(&mut self, cpu_addr: u16) -> u8 {
        if self.data.is_empty() {
            return 0x00;
        }
        let off = cpu_addr.wrapping_sub(self.base) as usize % self.data.len();
        self.data[off]
    }
}

/// DMC source backed by the loaded cartridge PRG (owned copy, made at ROM
/// load — never embedded).
///
/// The fixed bank (`$C000-$FFFF`, last 16 KiB of PRG) is exact; the
/// switchable window (`$8000-$BFFF`) reads the first bank (approximation —
/// DMC in Zelda II fires from the fixed bank; banked DMC would need the
/// live MMC1 state, a documented gap).
#[derive(Debug, Clone)]
pub struct PrgSource {
    prg: Vec<u8>,
}

impl PrgSource {
    /// Wrap the cartridge PRG bytes (header and CHR stripped).
    #[must_use]
    pub fn new(prg: Vec<u8>) -> Self {
        Self { prg }
    }
}

impl DmcSource for PrgSource {
    fn read_byte(&mut self, cpu_addr: u16) -> u8 {
        let off = if cpu_addr >= 0xC000 {
            // Fixed bank: last 16 KiB.
            self.prg.len().saturating_sub(0x4000) + (cpu_addr - 0xC000) as usize
        } else if cpu_addr >= 0x8000 {
            // Switchable window: first bank (approximation, see above).
            (cpu_addr - 0x8000) as usize
        } else {
            return 0x00;
        };
        self.prg.get(off).copied().unwrap_or(0x00)
    }
}

/// DMC voice.
#[derive(Debug, Clone)]
pub struct Dmc {
    /// IRQ enabled (`$4010` bit 7).
    irq_enabled: bool,
    /// Loop sample (`$4010` bit 6).
    loop_flag: bool,
    /// Rate index (`$4010` bits 3..0).
    rate_idx: u8,
    /// Rate timer divider.
    timer: u16,
    /// 7-bit DAC level (0..127).
    level: u8,
    /// Sample start address (`$C000 + $4012*64`).
    sample_addr: u16,
    /// Sample length in bytes (`$4013*16 + 1`).
    sample_len: u16,
    /// Current read cursor.
    addr_cursor: u16,
    /// Bytes left to fetch (0 = idle; drives `$4015` bit 4).
    bytes_remaining: u16,
    /// 1-byte sample buffer + full flag.
    buffer: Option<u8>,
    /// Output shift register.
    shift: u8,
    /// Bits left in the shift register.
    bits_remaining: u8,
    /// True when the shifter ran dry (output frozen).
    silence: bool,
    /// Interrupt flag (set at sample end when IRQ enabled, no loop).
    irq: bool,
    /// Channel enabled via `$4015` bit 4.
    enabled: bool,
}

impl Dmc {
    /// Create an idle DMC voice (level 0).
    #[must_use]
    pub fn new() -> Self {
        Self {
            irq_enabled: false,
            loop_flag: false,
            rate_idx: 0,
            timer: 0,
            level: 0,
            sample_addr: 0xC000,
            sample_len: 1,
            addr_cursor: 0xC000,
            bytes_remaining: 0,
            buffer: None,
            shift: 0,
            bits_remaining: 0,
            silence: true,
            irq: false,
            enabled: false,
        }
    }

    /// `$4010`: `IL--RRRR` (IRQ enable, loop, rate index).
    pub fn write_control(&mut self, v: u8) {
        self.irq_enabled = v & 0x80 != 0;
        self.loop_flag = v & 0x40 != 0;
        self.rate_idx = v & 0x0F;
        if !self.irq_enabled {
            self.irq = false;
        }
    }

    /// `$4011`: direct DAC load (bits 6..0). Stalls nothing.
    pub fn write_output(&mut self, v: u8) {
        self.level = v & 0x7F;
    }

    /// `$4012`: sample address = `$C000 + V*64`.
    pub fn write_address(&mut self, v: u8) {
        self.sample_addr = 0xC000 | (u16::from(v) << 6);
    }

    /// `$4013`: sample length = `V*16 + 1` bytes.
    pub fn write_length(&mut self, v: u8) {
        self.sample_len = (u16::from(v) << 4) | 1;
    }

    /// `$4015` bit 4. Disabling stops playback and clears the IRQ flag;
    /// enabling with an empty sample restarts it from the top.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.bytes_remaining = 0;
            self.irq = false;
        } else if self.bytes_remaining == 0 {
            self.restart();
        }
    }

    /// Restart the sample from its programmed address/length.
    fn restart(&mut self) {
        self.addr_cursor = self.sample_addr;
        self.bytes_remaining = self.sample_len;
    }

    /// Current rate-timer period in CPU cycles.
    #[must_use]
    pub fn timer_period(&self) -> u16 {
        DMC_RATE[usize::from(self.rate_idx)]
    }

    /// Advance the channel by one CPU cycle (rate timer + memory reader +
    /// output unit).
    pub fn clock(&mut self, src: &mut dyn DmcSource) {
        if self.timer == 0 {
            self.timer = self.timer_period();
            self.clock_output_unit(src);
        } else {
            self.timer -= 1;
        }
    }

    /// One output-unit tick: fetch, (re)load the shifter, apply one bit.
    fn clock_output_unit(&mut self, src: &mut dyn DmcSource) {
        // 1. Memory reader: keep the 1-byte buffer full while data remains.
        if self.bytes_remaining > 0 && self.buffer.is_none() {
            // Hardware stalls the CPU up to 4 cycles here; cycle accuracy
            // of the stall is the CPU's business, not the APU's.
            let b = src.read_byte(self.addr_cursor);
            self.buffer = Some(b);
            self.addr_cursor = match self.addr_cursor {
                0xFFFF => 0x8000, // DMC wraps $FFFF -> $8000.
                a => a + 1,
            };
            self.bytes_remaining -= 1;
            if self.bytes_remaining == 0 {
                if self.loop_flag {
                    self.restart();
                } else if self.irq_enabled {
                    self.irq = true;
                }
            }
        }
        // 2. Reload the shifter when empty.
        if self.bits_remaining == 0 {
            self.bits_remaining = 8;
            match self.buffer.take() {
                Some(b) => {
                    self.shift = b;
                    self.silence = false;
                }
                None => self.silence = true,
            }
        }
        // 3. Apply bit 0 to the DAC level.
        if !self.silence {
            if self.shift & 1 == 1 {
                if self.level <= 125 {
                    self.level += 2;
                }
            } else if self.level >= 2 {
                self.level -= 2;
            }
            self.shift >>= 1;
            self.bits_remaining -= 1;
        }
    }

    /// Current 7-bit DAC level (0..127).
    #[must_use]
    pub fn output(&self) -> u8 {
        self.level
    }

    /// True while sample bytes remain (drives `$4015` bit 4).
    #[must_use]
    pub fn active(&self) -> bool {
        self.bytes_remaining > 0
    }

    /// Interrupt flag (drives `$4015` bit 7).
    #[must_use]
    pub fn irq(&self) -> bool {
        self.irq
    }

    /// Clear the interrupt flag (on `$4015` disable or IRQ acknowledge).
    pub fn clear_irq(&mut self) {
        self.irq = false;
    }

    /// Bytes remaining (for tests).
    #[must_use]
    pub fn bytes_remaining(&self) -> u16 {
        self.bytes_remaining
    }
}

impl Default for Dmc {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dmc_with(src_bytes: &[u8]) -> (Dmc, SliceSource<'_>) {
        let mut d = Dmc::new();
        d.write_control(0x0F); // fastest rate, no irq/loop
        d.write_address(0x00); // $C000
        d.write_length(0x00); // 1 byte
        (
            d,
            SliceSource {
                base: 0xC000,
                data: src_bytes,
            },
        )
    }

    #[test]
    fn address_and_length_decode() {
        let mut d = Dmc::new();
        d.write_address(0xC0);
        d.write_length(0x10);
        d.set_enabled(true);
        // $C000 + 0xC0*64 = $F000; 0x10*16+1 = 257.
        assert_eq!(d.sample_addr, 0xF000);
        assert_eq!(d.sample_len, 257);
        assert_eq!(d.bytes_remaining(), 257);
        assert!(d.active());
    }

    #[test]
    fn direct_load_sets_level() {
        let mut d = Dmc::new();
        d.write_output(0x40);
        assert_eq!(d.output(), 0x40);
        d.write_output(0xFF);
        assert_eq!(d.output(), 0x7F, "only 7 bits");
    }

    #[test]
    fn all_ones_sample_raises_level_by_2_per_bit() {
        let (mut d, mut src) = dmc_with(&[0xFF]);
        d.set_enabled(true);
        d.write_output(0x00);
        // Rate 0 = 428 cycles/tick; run enough ticks for 8 bits + fetch.
        for _ in 0..(429 * 12) {
            d.clock(&mut src);
        }
        assert_eq!(d.output(), 16, "8 one-bits * +2");
        assert!(!d.active(), "1-byte sample consumed");
    }

    #[test]
    fn all_zero_sample_lowers_level_and_clamps() {
        let (mut d, mut src) = dmc_with(&[0x00]);
        d.set_enabled(true);
        d.write_output(0x10);
        for _ in 0..(429 * 12) {
            d.clock(&mut src);
        }
        assert_eq!(d.output(), 0, "8 zero-bits * -2 from 16, clamped at 0");
    }

    #[test]
    fn empty_sample_leaves_output_frozen_and_silent() {
        let mut d = Dmc::new();
        d.write_control(0x0F);
        d.set_enabled(false); // no sample programmed
        let mut src = SilentSource;
        d.write_output(0x20);
        for _ in 0..5000 {
            d.clock(&mut src);
        }
        assert_eq!(d.output(), 0x20, "dry shifter never touches the level");
        assert!(!d.active());
    }

    #[test]
    fn loop_restarts_sample_and_irq_fires_without_loop() {
        // Looping sample never idles and never IRQs.
        let (mut d, mut src) = dmc_with(&[0xFF]);
        d.write_control(0x4F); // loop + fastest
        d.set_enabled(true);
        for _ in 0..(429 * 40) {
            d.clock(&mut src);
        }
        assert!(d.active(), "looping sample never runs dry");
        assert!(!d.irq());
        // Non-looping sample with IRQ enabled raises IRQ at the end.
        let (mut d2, mut src2) = dmc_with(&[0xFF]);
        d2.write_control(0x8F); // irq + fastest
        d2.set_enabled(true);
        for _ in 0..(429 * 12) {
            d2.clock(&mut src2);
        }
        assert!(!d2.active());
        assert!(d2.irq());
        d2.set_enabled(false); // disable clears IRQ
        assert!(!d2.irq());
    }

    #[test]
    fn disable_stops_playback_immediately() {
        let (mut d, mut src) = dmc_with(&[0xFF; 64]);
        d.write_address(0x00);
        d.write_length(0x03); // 49 bytes
        d.set_enabled(true);
        for _ in 0..5000 {
            d.clock(&mut src);
        }
        assert!(d.bytes_remaining() < 49);
        d.set_enabled(false);
        assert_eq!(d.bytes_remaining(), 0);
        assert!(!d.active());
    }
}
