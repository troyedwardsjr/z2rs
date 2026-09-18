//! Wire format: `[WIRE_VERSION, tag, payload...]`, little-endian, exact length.
//!
//! | tag  | message | payload (after version + tag)                                             | total |
//! |------|---------|---------------------------------------------------------------------------|-------|
//! | 0x01 | Hello   | proto u16, role u8, rom_crc32 u32, trapset_id u64, coop_flags u32, delay u8, hash_interval u16 | 24 |
//! | 0x02 | Start   | frame u32, delay u8, coop_flags u32, hash_interval u16, wram_len u16 (0 or 8192), wram | 15 + wram_len |
//! | 0x10 | Input   | ack u32, first u32, count u8 (0..=32), pads[count]                         | 11 + count |
//! | 0x20 | Hash    | frame u32, hash u64                                                        | 14 |
//! | 0x30 | Ping    | token u32                                                                  | 6 |
//! | 0x31 | Pong    | token u32                                                                  | 6 |
//! | 0x32 | Status  | frame u32, lead i8 (rollback time sync)                                    | 7 |
//! | 0x40 | Bye     | reason u8                                                                  | 3 |
//!
//! `Input.ack` = number of leading remote frames the sender holds (all frames
//! `< ack` known); `first` = frame of `pads[0]`. `count == 0` is an ack-only
//! packet. Decoding rejects anything malformed, including trailing bytes; a
//! different leading version byte is [`DecodeError::BadVersion`].

use crate::transport::Channel;
use crate::{MAX_BATCH, MAX_PACKET_LEN, WIRE_VERSION, WRAM_LEN};

/// Tag of [`Message::Hello`].
pub const TAG_HELLO: u8 = 0x01;
/// Tag of [`Message::Start`].
pub const TAG_START: u8 = 0x02;
/// Tag of [`Message::Input`].
pub const TAG_INPUT: u8 = 0x10;
/// Tag of [`Message::Hash`].
pub const TAG_HASH: u8 = 0x20;
/// Tag of [`Message::Ping`].
pub const TAG_PING: u8 = 0x30;
/// Tag of [`Message::Pong`].
pub const TAG_PONG: u8 = 0x31;
/// Tag of [`Message::Status`].
pub const TAG_STATUS: u8 = 0x32;
/// Tag of [`Message::Bye`].
pub const TAG_BYE: u8 = 0x40;

/// Encoded Hello length.
pub const HELLO_LEN: usize = 24;
/// Encoded Start length without the WRAM bytes.
pub const START_HEADER_LEN: usize = 15;
/// Encoded Input length without the pads.
pub const INPUT_HEADER_LEN: usize = 11;
/// Encoded Hash length.
pub const HASH_LEN: usize = 14;
/// Encoded Ping/Pong length.
pub const PING_LEN: usize = 6;
/// Encoded Status length.
pub const STATUS_LEN: usize = 7;
/// Encoded Bye length.
pub const BYE_LEN: usize = 3;

/// Which player a peer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// Player 1 (pad1), camera owner, authoritative for delay/flags/WRAM.
    Host,
    /// Player 2 (pad2).
    Guest,
}

impl Role {
    /// Wire byte.
    pub const fn as_u8(self) -> u8 {
        match self {
            Role::Host => 0,
            Role::Guest => 1,
        }
    }

    /// From a wire byte.
    pub const fn from_u8(v: u8) -> Option<Role> {
        match v {
            0 => Some(Role::Host),
            1 => Some(Role::Guest),
            _ => None,
        }
    }

    /// The other role.
    pub const fn other(self) -> Role {
        match self {
            Role::Host => Role::Guest,
            Role::Guest => Role::Host,
        }
    }
}

/// Why a peer is leaving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ByeReason {
    /// User quit.
    Quit,
    /// State hashes differed.
    Desync,
    /// Protocol version mismatch.
    VersionMismatch,
    /// ROM identity mismatch.
    RomMismatch,
    /// Registered trap-set mismatch.
    TrapSetMismatch,
    /// Co-op flags not supported by the guest (or zero).
    CoopFlagsMismatch,
    /// Both peers claimed the same role.
    RoleConflict,
    /// Handshake or stall timeout.
    Timeout,
    /// Malformed or contradictory message.
    Protocol,
}

impl ByeReason {
    /// Wire byte.
    pub const fn as_u8(self) -> u8 {
        match self {
            ByeReason::Quit => 0,
            ByeReason::Desync => 1,
            ByeReason::VersionMismatch => 2,
            ByeReason::RomMismatch => 3,
            ByeReason::TrapSetMismatch => 4,
            ByeReason::CoopFlagsMismatch => 5,
            ByeReason::RoleConflict => 6,
            ByeReason::Timeout => 7,
            ByeReason::Protocol => 8,
        }
    }

    /// From a wire byte.
    pub const fn from_u8(v: u8) -> Option<ByeReason> {
        Some(match v {
            0 => ByeReason::Quit,
            1 => ByeReason::Desync,
            2 => ByeReason::VersionMismatch,
            3 => ByeReason::RomMismatch,
            4 => ByeReason::TrapSetMismatch,
            5 => ByeReason::CoopFlagsMismatch,
            6 => ByeReason::RoleConflict,
            7 => ByeReason::Timeout,
            8 => ByeReason::Protocol,
            _ => return None,
        })
    }
}

/// Handshake greeting, sent by both peers when the link comes up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// [`crate::PROTO_VERSION`] of the sender.
    pub proto: u16,
    /// Sender's role.
    pub role: Role,
    /// ROM body CRC32 (both frontends gate on the same ROM).
    pub rom_crc32: u32,
    /// Identity of the registered trap table (ported-routine set).
    pub trapset_id: u64,
    /// Host: the session's co-op flags. Guest: the flags it supports.
    pub coop_flags: u32,
    /// Sender's requested input delay (host's value wins).
    pub delay: u8,
    /// Sender's hash interval (host's value wins).
    pub hash_interval: u16,
}

/// Host -> guest: session parameters and the host's WRAM (save) snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    /// First frame of the session (always 0 in protocol 1).
    pub frame: u32,
    /// Effective input delay.
    pub delay: u8,
    /// Effective co-op flags.
    pub coop_flags: u32,
    /// Effective hash interval (0 = hashing disabled).
    pub hash_interval: u16,
    /// Empty (both keep power-on WRAM) or exactly [`WRAM_LEN`] bytes.
    pub wram: Vec<u8>,
}

/// One protocol message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Handshake greeting.
    Hello(Hello),
    /// Session start (host -> guest).
    Start(Start),
    /// Batched pads plus acknowledgement.
    Input {
        /// All sender-side remote frames `< ack` are known to the sender.
        ack: u32,
        /// Frame of `pads[0]`.
        first: u32,
        /// 0..=[`MAX_BATCH`] consecutive pads.
        pads: Vec<u8>,
    },
    /// State hash after stepping `frame`.
    Hash {
        /// Frame the hash was taken after.
        frame: u32,
        /// FNV-1a 64 of the state bytes.
        hash: u64,
    },
    /// RTT probe; echoed as Pong.
    Ping {
        /// Opaque token (sender's `now_ms` low bits).
        token: u32,
    },
    /// RTT echo.
    Pong {
        /// Token from the Ping.
        token: u32,
    },
    /// Rollback time sync: the sender's next frame to simulate and its
    /// estimate of how many frames it runs ahead of the receiver (clamped).
    Status {
        /// Sender's current frame.
        frame: u32,
        /// Sender's own lead estimate in frames (positive = sender ahead).
        lead: i8,
    },
    /// Orderly close.
    Bye(ByeReason),
}

/// Why a packet was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Zero-length packet.
    Empty,
    /// Longer than [`MAX_PACKET_LEN`].
    TooLong(usize),
    /// Leading version byte is not [`WIRE_VERSION`].
    BadVersion(u8),
    /// Unknown tag.
    BadTag(u8),
    /// Packet ends early.
    Truncated,
    /// Bytes left after the message.
    TrailingBytes,
    /// Invalid role byte.
    BadRole(u8),
    /// Invalid Bye reason byte.
    BadReason(u8),
    /// Input pad count above [`MAX_BATCH`], or the frame range overflows.
    BadCount(u8),
    /// Start WRAM length other than 0 or [`WRAM_LEN`].
    BadWramLen(u16),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Empty => f.write_str("empty packet"),
            DecodeError::TooLong(n) => write!(f, "packet too long ({n} bytes)"),
            DecodeError::BadVersion(v) => write!(f, "wire version {v}, expected {WIRE_VERSION}"),
            DecodeError::BadTag(t) => write!(f, "unknown tag {t:#04x}"),
            DecodeError::Truncated => f.write_str("truncated packet"),
            DecodeError::TrailingBytes => f.write_str("trailing bytes"),
            DecodeError::BadRole(r) => write!(f, "bad role {r}"),
            DecodeError::BadReason(r) => write!(f, "bad bye reason {r}"),
            DecodeError::BadCount(c) => write!(f, "bad input count {c}"),
            DecodeError::BadWramLen(n) => write!(f, "bad WRAM length {n}"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl Message {
    /// Default channel: Input/Ping/Pong unreliable, everything else reliable.
    pub fn channel(&self) -> Channel {
        match self {
            Message::Input { .. }
            | Message::Ping { .. }
            | Message::Pong { .. }
            | Message::Status { .. } => Channel::Unreliable,
            _ => Channel::Reliable,
        }
    }

    /// Encode to a new buffer.
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(16);
        o.push(WIRE_VERSION);
        match self {
            Message::Hello(h) => {
                o.push(TAG_HELLO);
                o.extend_from_slice(&h.proto.to_le_bytes());
                o.push(h.role.as_u8());
                o.extend_from_slice(&h.rom_crc32.to_le_bytes());
                o.extend_from_slice(&h.trapset_id.to_le_bytes());
                o.extend_from_slice(&h.coop_flags.to_le_bytes());
                o.push(h.delay);
                o.extend_from_slice(&h.hash_interval.to_le_bytes());
            }
            Message::Start(s) => {
                o.push(TAG_START);
                o.extend_from_slice(&s.frame.to_le_bytes());
                o.push(s.delay);
                o.extend_from_slice(&s.coop_flags.to_le_bytes());
                o.extend_from_slice(&s.hash_interval.to_le_bytes());
                let len = u16::try_from(s.wram.len()).unwrap_or(u16::MAX);
                o.extend_from_slice(&len.to_le_bytes());
                o.extend_from_slice(&s.wram[..usize::from(len).min(s.wram.len())]);
            }
            Message::Input { ack, first, pads } => {
                o.push(TAG_INPUT);
                o.extend_from_slice(&ack.to_le_bytes());
                o.extend_from_slice(&first.to_le_bytes());
                let n = pads.len().min(MAX_BATCH);
                o.push(n as u8);
                o.extend_from_slice(&pads[..n]);
            }
            Message::Hash { frame, hash } => {
                o.push(TAG_HASH);
                o.extend_from_slice(&frame.to_le_bytes());
                o.extend_from_slice(&hash.to_le_bytes());
            }
            Message::Ping { token } => {
                o.push(TAG_PING);
                o.extend_from_slice(&token.to_le_bytes());
            }
            Message::Pong { token } => {
                o.push(TAG_PONG);
                o.extend_from_slice(&token.to_le_bytes());
            }
            Message::Status { frame, lead } => {
                o.push(TAG_STATUS);
                o.extend_from_slice(&frame.to_le_bytes());
                o.push(*lead as u8);
            }
            Message::Bye(r) => {
                o.push(TAG_BYE);
                o.push(r.as_u8());
            }
        }
        o
    }

    /// Decode one packet (strict).
    pub fn decode(bytes: &[u8]) -> Result<Message, DecodeError> {
        let Some(&version) = bytes.first() else {
            return Err(DecodeError::Empty);
        };
        if bytes.len() > MAX_PACKET_LEN {
            return Err(DecodeError::TooLong(bytes.len()));
        }
        if version != WIRE_VERSION {
            return Err(DecodeError::BadVersion(version));
        }
        let mut r = Reader { b: bytes, pos: 1 };
        let msg = match r.u8()? {
            TAG_HELLO => {
                let proto = r.u16()?;
                let role_b = r.u8()?;
                let role = Role::from_u8(role_b).ok_or(DecodeError::BadRole(role_b))?;
                Message::Hello(Hello {
                    proto,
                    role,
                    rom_crc32: r.u32()?,
                    trapset_id: r.u64()?,
                    coop_flags: r.u32()?,
                    delay: r.u8()?,
                    hash_interval: r.u16()?,
                })
            }
            TAG_START => {
                let frame = r.u32()?;
                let delay = r.u8()?;
                let coop_flags = r.u32()?;
                let hash_interval = r.u16()?;
                let len = r.u16()?;
                if len != 0 && usize::from(len) != WRAM_LEN {
                    return Err(DecodeError::BadWramLen(len));
                }
                let wram = r.take(usize::from(len))?.to_vec();
                Message::Start(Start {
                    frame,
                    delay,
                    coop_flags,
                    hash_interval,
                    wram,
                })
            }
            TAG_INPUT => {
                let ack = r.u32()?;
                let first = r.u32()?;
                let count = r.u8()?;
                if usize::from(count) > MAX_BATCH || first.checked_add(u32::from(count)).is_none() {
                    return Err(DecodeError::BadCount(count));
                }
                let pads = r.take(usize::from(count))?.to_vec();
                Message::Input { ack, first, pads }
            }
            TAG_HASH => Message::Hash {
                frame: r.u32()?,
                hash: r.u64()?,
            },
            TAG_PING => Message::Ping { token: r.u32()? },
            TAG_PONG => Message::Pong { token: r.u32()? },
            TAG_STATUS => Message::Status {
                frame: r.u32()?,
                lead: r.u8()? as i8,
            },
            TAG_BYE => {
                let b = r.u8()?;
                Message::Bye(ByeReason::from_u8(b).ok_or(DecodeError::BadReason(b))?)
            }
            t => return Err(DecodeError::BadTag(t)),
        };
        if r.pos != bytes.len() {
            return Err(DecodeError::TrailingBytes);
        }
        Ok(msg)
    }
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::Truncated)?;
        let s = self.b.get(self.pos..end).ok_or(DecodeError::Truncated)?;
        self.pos = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        <[u8; N]>::try_from(self.take(N)?).map_err(|_| DecodeError::Truncated)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
}
