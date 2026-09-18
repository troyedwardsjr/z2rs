//! Wire format round trips and malformed-input rejection (ROM-free).

use z2_net::wire::{
    BYE_LEN, HASH_LEN, HELLO_LEN, INPUT_HEADER_LEN, PING_LEN, START_HEADER_LEN, STATUS_LEN,
    TAG_BYE, TAG_HELLO, TAG_INPUT, TAG_START,
};
use z2_net::{
    room_name_valid, room_url, ByeReason, Channel, DecodeError, Hello, Message, Role, RoomUrlError,
    Start, MAX_BATCH, MAX_PACKET_LEN, WIRE_VERSION, WRAM_LEN,
};

const REASONS: [ByeReason; 9] = [
    ByeReason::Quit,
    ByeReason::Desync,
    ByeReason::VersionMismatch,
    ByeReason::RomMismatch,
    ByeReason::TrapSetMismatch,
    ByeReason::CoopFlagsMismatch,
    ByeReason::RoleConflict,
    ByeReason::Timeout,
    ByeReason::Protocol,
];

fn samples() -> Vec<(Message, usize)> {
    let mut v = vec![
        (
            Message::Hello(Hello {
                proto: 0x1234,
                role: Role::Guest,
                rom_crc32: 0xBA32_2865,
                trapset_id: 0x0123_4567_89AB_CDEF,
                coop_flags: 0x8000_0001,
                delay: 8,
                hash_interval: 60,
            }),
            HELLO_LEN,
        ),
        (
            Message::Start(Start {
                frame: 0,
                delay: 2,
                coop_flags: 3,
                hash_interval: 60,
                wram: Vec::new(),
            }),
            START_HEADER_LEN,
        ),
        (
            Message::Start(Start {
                frame: 7,
                delay: 0,
                coop_flags: 1,
                hash_interval: 0,
                wram: (0..WRAM_LEN).map(|i| (i * 31) as u8).collect(),
            }),
            START_HEADER_LEN + WRAM_LEN,
        ),
        (
            Message::Input {
                ack: 5,
                first: 9,
                pads: Vec::new(),
            },
            INPUT_HEADER_LEN,
        ),
        (
            Message::Input {
                ack: 0,
                first: u32::MAX - 1,
                pads: vec![0xFF],
            },
            INPUT_HEADER_LEN + 1,
        ),
        (
            Message::Input {
                ack: 1000,
                first: 990,
                pads: (0..MAX_BATCH as u8).collect(),
            },
            INPUT_HEADER_LEN + MAX_BATCH,
        ),
        (
            Message::Hash {
                frame: 59,
                hash: u64::MAX - 3,
            },
            HASH_LEN,
        ),
        (Message::Ping { token: 0xDEAD_BEEF }, PING_LEN),
        (Message::Pong { token: 1 }, PING_LEN),
        (
            Message::Status {
                frame: 1234,
                lead: -7,
            },
            STATUS_LEN,
        ),
        (
            Message::Status {
                frame: u32::MAX,
                lead: i8::MAX,
            },
            STATUS_LEN,
        ),
    ];
    for r in REASONS {
        v.push((Message::Bye(r), BYE_LEN));
    }
    v
}

#[test]
fn round_trip_and_lengths() {
    for (m, len) in samples() {
        let bytes = m.encode();
        assert_eq!(bytes.len(), len, "{m:?}");
        assert_eq!(bytes[0], WIRE_VERSION);
        assert_eq!(Message::decode(&bytes), Ok(m));
    }
}

// The largest message (Start with a full WRAM snapshot) must fit the packet
// budget and stay well below WebRTC's message-size limits.
const _: () = assert!(MAX_PACKET_LEN == START_HEADER_LEN + WRAM_LEN);
const _: () = assert!(MAX_PACKET_LEN < 16 * 1024);

#[test]
fn channels() {
    for (m, _) in samples() {
        let expect = match m {
            Message::Input { .. }
            | Message::Ping { .. }
            | Message::Pong { .. }
            | Message::Status { .. } => Channel::Unreliable,
            _ => Channel::Reliable,
        };
        assert_eq!(m.channel(), expect);
    }
}

#[test]
fn every_prefix_is_truncated() {
    for (m, _) in samples() {
        let bytes = m.encode();
        for n in 1..bytes.len() {
            assert_eq!(
                Message::decode(&bytes[..n]),
                Err(DecodeError::Truncated),
                "{m:?} prefix {n}"
            );
        }
    }
}

#[test]
fn malformed_rejected() {
    assert_eq!(Message::decode(&[]), Err(DecodeError::Empty));
    assert_eq!(
        Message::decode(&[2, TAG_BYE, 0]),
        Err(DecodeError::BadVersion(2))
    );
    assert_eq!(
        Message::decode(&[WIRE_VERSION, 0x7F]),
        Err(DecodeError::BadTag(0x7F))
    );
    assert_eq!(
        Message::decode(&vec![WIRE_VERSION; MAX_PACKET_LEN + 1]),
        Err(DecodeError::TooLong(MAX_PACKET_LEN + 1))
    );

    let mut trailing = Message::Bye(ByeReason::Quit).encode();
    trailing.push(0);
    assert_eq!(Message::decode(&trailing), Err(DecodeError::TrailingBytes));

    assert_eq!(
        Message::decode(&[WIRE_VERSION, TAG_BYE, 9]),
        Err(DecodeError::BadReason(9))
    );

    let mut hello = samples()[0].0.encode();
    assert_eq!(hello[1], TAG_HELLO);
    hello[4] = 2; // role byte
    assert_eq!(Message::decode(&hello), Err(DecodeError::BadRole(2)));

    let mut input = vec![WIRE_VERSION, TAG_INPUT];
    input.extend_from_slice(&0u32.to_le_bytes());
    input.extend_from_slice(&0u32.to_le_bytes());
    input.push(MAX_BATCH as u8 + 1);
    input.extend(std::iter::repeat_n(0, MAX_BATCH + 1));
    assert_eq!(
        Message::decode(&input),
        Err(DecodeError::BadCount(MAX_BATCH as u8 + 1))
    );

    let mut overflow = vec![WIRE_VERSION, TAG_INPUT];
    overflow.extend_from_slice(&0u32.to_le_bytes());
    overflow.extend_from_slice(&u32::MAX.to_le_bytes());
    overflow.extend_from_slice(&[2, 0, 0]);
    assert_eq!(Message::decode(&overflow), Err(DecodeError::BadCount(2)));

    let mut start = vec![WIRE_VERSION, TAG_START];
    start.extend_from_slice(&0u32.to_le_bytes());
    start.push(2);
    start.extend_from_slice(&1u32.to_le_bytes());
    start.extend_from_slice(&60u16.to_le_bytes());
    start.extend_from_slice(&5u16.to_le_bytes());
    start.extend_from_slice(&[0; 5]);
    assert_eq!(Message::decode(&start), Err(DecodeError::BadWramLen(5)));
}

#[test]
fn garbage_never_panics() {
    let mut x: u64 = 0x1234_5678_9ABC_DEF1;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..20_000 {
        let len = (next() % 64) as usize;
        let mut buf: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        if let Some(b) = buf.first_mut() {
            if next() % 2 == 0 {
                *b = WIRE_VERSION;
            }
        }
        if let Ok(m) = Message::decode(&buf) {
            assert_eq!(m.encode(), buf, "accepted packets re-encode identically");
        }
    }
}

#[test]
fn room_urls() {
    assert_eq!(
        room_url("ws://h:3536/", "abc").as_deref(),
        Ok("ws://h:3536/z2-abc")
    );
    assert_eq!(
        room_url("wss://example.com", "A_b-9").as_deref(),
        Ok("wss://example.com/z2-A_b-9")
    );
    assert_eq!(room_url("http://h", "abc"), Err(RoomUrlError::BadSignalUrl));
    assert_eq!(room_url("ws://", "abc"), Err(RoomUrlError::BadSignalUrl));
    assert_eq!(room_url("ws://h/x", "abc"), Err(RoomUrlError::BadSignalUrl));
    assert_eq!(room_url("ws://h", "a b"), Err(RoomUrlError::BadRoom));
    assert!(!room_name_valid(""));
    assert!(!room_name_valid(&"x".repeat(33)));
    assert!(room_name_valid(&"x".repeat(32)));
}
