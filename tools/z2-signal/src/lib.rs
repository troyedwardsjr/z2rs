//! z2-signal: signalling server for z2rs co-op netplay.
//!
//! Built on `matchbox_signaling`'s FullMesh topology. The room is the URL path
//! (`ws://host:3536/z2-myroom`); each room admits at most two peers. A third
//! connection to a full room is refused before the websocket upgrade with
//! HTTP 409, a bad room path with HTTP 400. FullMesh ignores `?next=N`, so the
//! [`RoomTable`] here is the pairing authority.
//!
//! No TLS: for `wss://` (required by browsers on https pages) put the server
//! behind a TLS-terminating reverse proxy such as caddy or nginx.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use matchbox_signaling::SignalingServer;

/// Peers allowed per room.
pub const MAX_PEERS_PER_ROOM: usize = 2;
/// An admitted connection that never received an id is forgotten after this long.
pub const PENDING_TTL_MS: u64 = 10_000;
/// Default listen address.
pub const DEFAULT_BIND: &str = "0.0.0.0:3536";
/// Longest accepted room path segment.
pub const MAX_ROOM_LEN: usize = 64;

/// Command-line help.
pub const USAGE: &str = "\
usage: z2-signal [--bind ADDR] [--verbose] [--help]
  --bind ADDR   listen address (default 0.0.0.0:3536)
  --verbose     also enable matchbox_signaling HTTP tracing
  --help        show this help

Rooms are URL paths: ws://HOST:3536/z2-<room>, two peers per room.
No TLS: for wss:// put z2-signal behind a TLS reverse proxy (caddy, nginx).
exit: 0 ok, 1 bind/serve error, 2 usage error";

/// Why a connection was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// Missing or invalid room path.
    BadRoom,
    /// Room already has two peers.
    RoomFull,
}

impl Reject {
    /// HTTP status returned to the client.
    pub fn status(self) -> StatusCode {
        match self {
            Reject::BadRoom => StatusCode::BAD_REQUEST,
            Reject::RoomFull => StatusCode::CONFLICT,
        }
    }
}

/// Extract the room from a request path (`/z2-abc`, `z2-abc`, query ignored).
pub fn room_from_path(path: Option<&str>) -> Result<String, Reject> {
    let p = path.unwrap_or("").trim_start_matches('/');
    let p = p.split(['?', '#']).next().unwrap_or("");
    let ok = (1..=MAX_ROOM_LEN).contains(&p.len())
        && p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok {
        Ok(p.to_string())
    } else {
        Err(Reject::BadRoom)
    }
}

/// Room occupancy bookkeeping (pure; time is passed in).
#[derive(Debug, Default)]
pub struct RoomTable {
    peers: BTreeMap<String, Vec<String>>,
    pending: BTreeMap<SocketAddr, (String, u64)>,
    peer_room: BTreeMap<String, String>,
}

impl RoomTable {
    /// Empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Peers plus pending admissions in `room`.
    pub fn occupancy(&self, room: &str) -> usize {
        self.peers.get(room).map_or(0, Vec::len)
            + self.pending.values().filter(|(r, _)| r == room).count()
    }

    /// Number of rooms with at least one assigned peer.
    pub fn open_rooms(&self) -> usize {
        self.peers.len()
    }

    /// Admission check for a websocket upgrade request.
    pub fn try_admit(
        &mut self,
        addr: SocketAddr,
        path: Option<&str>,
        now_ms: u64,
    ) -> Result<String, Reject> {
        self.pending
            .retain(|_, (_, t)| now_ms.saturating_sub(*t) < PENDING_TTL_MS);
        let room = room_from_path(path)?;
        if self.occupancy(&room) >= MAX_PEERS_PER_ROOM {
            return Err(Reject::RoomFull);
        }
        self.pending.insert(addr, (room.clone(), now_ms));
        Ok(room)
    }

    /// The admitted connection from `addr` got peer id `peer`; returns its room.
    pub fn on_id_assigned(&mut self, addr: SocketAddr, peer: &str) -> Option<String> {
        let (room, _) = self.pending.remove(&addr)?;
        self.peers
            .entry(room.clone())
            .or_default()
            .push(peer.to_string());
        self.peer_room.insert(peer.to_string(), room.clone());
        Some(room)
    }

    /// `peer` disconnected; returns `(room, remaining peers)`. Empty rooms are removed.
    pub fn on_peer_left(&mut self, peer: &str) -> Option<(String, usize)> {
        let room = self.peer_room.remove(peer)?;
        let remaining = match self.peers.get_mut(&room) {
            Some(list) => {
                list.retain(|p| p != peer);
                list.len()
            }
            None => 0,
        };
        if remaining == 0 {
            self.peers.remove(&room);
        }
        Some((room, remaining))
    }
}

fn lock(table: &Mutex<RoomTable>) -> MutexGuard<'_, RoomTable> {
    table.lock().unwrap_or_else(|e| e.into_inner())
}

fn log(msg: &str) {
    eprintln!("[z2-signal] {msg}");
}

/// Build (not bind) the room-of-2 FullMesh server.
// The `Err` type of on_connection_request is matchbox's (axum) `Response`, which
// is large; the callback signature is not ours to change.
#[allow(clippy::result_large_err)]
pub fn build_server(addr: SocketAddr, verbose: bool) -> SignalingServer {
    let table = Arc::new(Mutex::new(RoomTable::new()));
    let epoch = Instant::now();
    let admit = Arc::clone(&table);
    let assign = Arc::clone(&table);
    let leave = table;
    let builder = SignalingServer::full_mesh_builder(addr)
        .on_connection_request(move |meta| {
            let now = epoch.elapsed().as_millis() as u64;
            match lock(&admit).try_admit(meta.origin, meta.path.as_deref(), now) {
                Ok(room) => {
                    log(&format!("admit room={room} from={}", meta.origin));
                    Ok(true)
                }
                Err(r) => {
                    log(&format!(
                        "reject path={:?} from={} reason={r:?}",
                        meta.path, meta.origin
                    ));
                    Err((r.status(), format!("{r:?}")).into_response())
                }
            }
        })
        .on_id_assignment(move |(addr, peer)| {
            let peer = peer.to_string();
            match lock(&assign).on_id_assigned(addr, &peer) {
                Some(room) => log(&format!("connected room={room} peer={peer}")),
                None => log(&format!("assigned unknown connection {addr} peer={peer}")),
            }
        })
        .on_peer_disconnected(move |peer| {
            let peer = peer.to_string();
            if let Some((room, remaining)) = lock(&leave).on_peer_left(&peer) {
                log(&format!(
                    "disconnected room={room} peer={peer} remaining={remaining}"
                ));
                if remaining == 0 {
                    log(&format!("room closed {room}"));
                }
            }
        });
    let builder = if verbose { builder.trace() } else { builder };
    builder.build()
}

/// Bind and serve until the process ends.
pub async fn serve(addr: SocketAddr, verbose: bool) -> Result<(), matchbox_signaling::Error> {
    let mut server = build_server(addr, verbose);
    let bound = server.bind()?;
    log(&format!(
        "listening on ws://{bound} (rooms: /z2-<name>, {MAX_PEERS_PER_ROOM} peers each; no TLS)"
    ));
    server.serve().await
}

/// Parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Print [`USAGE`].
    Help,
    /// Run the server.
    Run {
        /// Listen address.
        bind: SocketAddr,
        /// Enable HTTP tracing.
        verbose: bool,
    },
}

/// Parse arguments (without the program name).
pub fn parse_args(args: &[String]) -> Result<Command, String> {
    let mut bind: SocketAddr = DEFAULT_BIND
        .parse()
        .map_err(|e| format!("default bind address: {e}"))?;
    let mut verbose = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--verbose" => verbose = true,
            "--bind" => {
                let v = it.next().ok_or("--bind expects ADDR (e.g. 0.0.0.0:3536)")?;
                bind = v
                    .parse()
                    .map_err(|e| format!("--bind {v}: {e} (expected IP:PORT)"))?;
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Command::Run { bind, verbose })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn room_paths() {
        assert_eq!(room_from_path(Some("/z2-abc")), Ok("z2-abc".into()));
        assert_eq!(room_from_path(Some("z2-abc?next=2")), Ok("z2-abc".into()));
        assert_eq!(room_from_path(None), Err(Reject::BadRoom));
        assert_eq!(room_from_path(Some("/")), Err(Reject::BadRoom));
        assert_eq!(room_from_path(Some("/a b")), Err(Reject::BadRoom));
        assert_eq!(room_from_path(Some(&"x".repeat(65))), Err(Reject::BadRoom));
    }

    #[test]
    fn two_per_room_and_release() {
        let mut t = RoomTable::new();
        assert!(t.try_admit(addr(1), Some("r"), 0).is_ok());
        assert!(t.try_admit(addr(2), Some("r"), 0).is_ok());
        assert_eq!(t.try_admit(addr(3), Some("r"), 0), Err(Reject::RoomFull));
        assert!(t.try_admit(addr(4), Some("other"), 0).is_ok());
        assert_eq!(t.on_id_assigned(addr(1), "p1"), Some("r".into()));
        assert_eq!(t.on_id_assigned(addr(2), "p2"), Some("r".into()));
        assert_eq!(t.try_admit(addr(3), Some("r"), 0), Err(Reject::RoomFull));
        assert_eq!(t.on_peer_left("p1"), Some(("r".into(), 1)));
        assert!(t.try_admit(addr(3), Some("r"), 0).is_ok());
        assert_eq!(t.on_peer_left("nobody"), None);
    }

    #[test]
    fn stale_pending_expires() {
        let mut t = RoomTable::new();
        assert!(t.try_admit(addr(1), Some("r"), 0).is_ok());
        assert!(t.try_admit(addr(2), Some("r"), 0).is_ok());
        assert!(t.try_admit(addr(3), Some("r"), PENDING_TTL_MS).is_ok());
        assert_eq!(t.occupancy("r"), 1);
    }

    #[test]
    fn empty_room_is_removed() {
        let mut t = RoomTable::new();
        t.try_admit(addr(1), Some("r"), 0).unwrap();
        t.on_id_assigned(addr(1), "p1");
        assert_eq!(t.open_rooms(), 1);
        assert_eq!(t.on_peer_left("p1"), Some(("r".into(), 0)));
        assert_eq!(t.open_rooms(), 0);
    }

    #[test]
    fn args() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_args(&s(&["--help"])), Ok(Command::Help));
        assert_eq!(
            parse_args(&s(&["--bind", "127.0.0.1:9000", "--verbose"])),
            Ok(Command::Run {
                bind: addr(9000),
                verbose: true
            })
        );
        assert!(parse_args(&s(&["--bind"])).is_err());
        assert!(parse_args(&s(&["--bind", "nope"])).is_err());
        assert!(parse_args(&s(&["--what"])).is_err());
    }
}
