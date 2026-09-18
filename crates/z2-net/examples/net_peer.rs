//! A ROM-free native netplay peer, for connection tests against the browser.
//!
//! It opens the same `MatchboxTransport` the desktop app uses, hosts a
//! lockstep session and steps idle pads (no game), printing each connect stage
//! with a timestamp. `site/netplay-e2e.mjs` runs it against a browser guest to
//! prove a browser and a native peer reach a running session.
//!
//! ```text
//! cargo run --release -p z2-net --features matchbox --example net_peer -- \
//!     --signal ws://127.0.0.1:3536 --room abc --trapset 0123456789abcdef \
//!     [--crc ba322865] [--ice "none"] [--frames 300] [--timeout-ms 20000]
//! ```
//!
//! Hashing is disabled (the host's interval wins), because there is no game
//! state to hash. Exit 0 once `--frames` frames were stepped, 1 on a closed
//! session or timeout, 2 on bad arguments.

use std::time::{Duration, Instant};

use z2_net::{room_url, IceConfig, MatchboxTransport, Session, SessionConfig, SessionEvent};
use z2_net::{ConnectStage, Transport, COOP_TWO_LINKS};

struct Args {
    signal: String,
    room: String,
    crc: u32,
    trapset: u64,
    ice: IceConfig,
    frames: u32,
    timeout: Duration,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        signal: z2_net::DEFAULT_SIGNAL_URL.to_string(),
        room: String::new(),
        // Zelda II (USA) body CRC32, as z2_assets::rom::EXPECTED_BODY_CRC32.
        crc: 0xBA32_2865,
        trapset: 0,
        ice: IceConfig::default(),
        frames: 300,
        timeout: Duration::from_secs(20),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || it.next().ok_or(format!("{flag} expects a value"));
        match flag.as_str() {
            "--signal" => a.signal = val()?,
            "--room" => a.room = val()?,
            "--crc" => {
                a.crc = u32::from_str_radix(&val()?, 16).map_err(|e| format!("--crc: {e}"))?;
            }
            "--trapset" => {
                a.trapset =
                    u64::from_str_radix(&val()?, 16).map_err(|e| format!("--trapset: {e}"))?;
            }
            "--ice" => a.ice = IceConfig::parse(&val()?).map_err(|e| e.to_string())?,
            "--frames" => a.frames = val()?.parse().map_err(|e| format!("--frames: {e}"))?,
            "--timeout-ms" => {
                a.timeout = Duration::from_millis(
                    val()?.parse().map_err(|e| format!("--timeout-ms: {e}"))?,
                );
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(a)
}

fn main() {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("net_peer: {e}");
            std::process::exit(2);
        }
    };
    let url = match room_url(&args.signal, &args.room) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("net_peer: {e}");
            std::process::exit(2);
        }
    };
    let mut cfg = SessionConfig::host(args.crc, args.trapset, COOP_TWO_LINKS);
    cfg.hash_interval = 0;
    let mut session = Session::new(cfg).expect("valid host config");
    let mut transport = MatchboxTransport::connect(&url, Some(args.ice));
    let t0 = Instant::now();
    let ms = || t0.elapsed().as_millis() as u64;
    let mut stage = None::<ConnectStage>;
    let mut started_at = None;
    let mut stepped = 0u32;
    println!("net_peer: hosting {url}");
    loop {
        let now = ms();
        session.update(&mut transport, now);
        if stage != Some(transport.stage()) {
            stage = Some(transport.stage());
            println!("{now:>6} ms stage {}", transport.stage().as_str());
        }
        for ev in session.take_events() {
            match ev {
                SessionEvent::Started { delay, .. } => {
                    started_at = Some(now);
                    println!("{now:>6} ms started (delay {delay})");
                }
                SessionEvent::Closed(reason) => {
                    println!("NET-PEER-FAIL closed at {now} ms: {reason}");
                    std::process::exit(1);
                }
                _ => {}
            }
        }
        for _ in 0..2 {
            session.latch_local(0);
            if session.poll().is_none() {
                break;
            }
            stepped += 1;
        }
        session.update(&mut transport, now);
        if stepped >= args.frames {
            println!(
                "NET-PEER-OK frames={stepped} started_ms={}",
                started_at.unwrap_or(0)
            );
            // Let the last inputs and the goodbye leave before exiting.
            session.close();
            for t in 0..20 {
                session.update(&mut transport, ms() + t);
                std::thread::sleep(Duration::from_millis(10));
            }
            return;
        }
        if t0.elapsed() > args.timeout {
            println!(
                "NET-PEER-FAIL timeout after {} ms: stage {:?}, session {:?}, {stepped} frames",
                ms(),
                transport.stage(),
                session.state()
            );
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}
