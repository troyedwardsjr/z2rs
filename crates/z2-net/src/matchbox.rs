//! WebRTC transport over matchbox (feature `matchbox`).
//!
//! Channel 0 is reliable (ordered), channel 1 unreliable. The first remote
//! peer that connects becomes *the* peer; any later peer in the same room is
//! ignored (its packets are dropped), so an intruder cannot tear down a live
//! session. Errors never panic: matchbox's panicking `update_peers`/`send`
//! are avoided in favour of `try_update_peers`/`try_send`, and the end of the
//! message loop is surfaced as [`LinkState::Closed`] (or [`LinkState::RoomFull`]
//! when the signalling server answered HTTP 409, native only).
//!
//! Connection progress is observed by wrapping matchbox's own signaller, so
//! [`Transport::stage`] can tell "joining the room" from "waiting for the other
//! player" from "establishing the peer link". Two phases have deadlines and end
//! the link with an actionable [`LinkState::Closed`] message when missed:
//! joining the room ([`SIGNALLING_TIMEOUT_MS`]) and establishing the peer link
//! once the other player is there ([`LINK_TIMEOUT_MS`]). Waiting for the other
//! player has no deadline: a host may wait as long as it likes, and leaving is
//! the caller's decision.
//!
//! Native: the message-loop future runs on a dedicated `std::thread` under
//! `futures::executor::block_on`. wasm32: `wasm_bindgen_futures::spawn_local`
//! (the loop future is `!Send`). Call [`Transport::recv`] every frontend tick.

use std::sync::{Arc, Mutex};

use matchbox_socket::async_trait::async_trait;
use matchbox_socket::{
    DefaultSignallerBuilder, PeerEvent, PeerId, PeerRequest, PeerState, RtcIceServerConfig,
    SignalingError, Signaller, SignallerBuilder, WebRtcSocket, WebRtcSocketBuilder,
};

use crate::ice::IceConfig;
use crate::transport::{Channel, ConnectStage, LinkState, Transport, TransportError};

/// Joining the room on the signalling server must finish within this long.
pub const SIGNALLING_TIMEOUT_MS: u64 = 10_000;
/// Once the other player is in the room, the direct peer link must open
/// within this long. Two browsers on one machine take about a second.
pub const LINK_TIMEOUT_MS: u64 = 15_000;

#[cfg(not(target_arch = "wasm32"))]
type LoopHandle = Option<std::thread::JoinHandle<Result<(), String>>>;
#[cfg(target_arch = "wasm32")]
type LoopHandle = std::rc::Rc<std::cell::RefCell<Option<Result<(), String>>>>;

/// What the signalling traffic has shown so far.
#[derive(Debug, Default)]
struct Progress {
    /// The server admitted us to the room (`IdAssigned`).
    joined: bool,
    /// Peers seen in the room that are not connected yet.
    peers: Vec<PeerId>,
}

impl Progress {
    fn saw(&mut self, id: PeerId) {
        if !self.peers.contains(&id) {
            self.peers.push(id);
        }
    }
}

type SharedProgress = Arc<Mutex<Progress>>;

/// matchbox's default signaller, with every event noted in [`Progress`].
#[derive(Debug)]
struct ObservedSignallerBuilder {
    inner: DefaultSignallerBuilder,
    progress: SharedProgress,
}

struct ObservedSignaller {
    inner: Box<dyn Signaller>,
    progress: SharedProgress,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl SignallerBuilder for ObservedSignallerBuilder {
    async fn new_signaller(
        &self,
        attempts: Option<u16>,
        room_url: String,
    ) -> Result<Box<dyn Signaller>, SignalingError> {
        let inner = self.inner.new_signaller(attempts, room_url).await?;
        Ok(Box::new(ObservedSignaller {
            inner,
            progress: Arc::clone(&self.progress),
        }))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Signaller for ObservedSignaller {
    async fn send(&mut self, request: PeerRequest) -> Result<(), SignalingError> {
        self.inner.send(request).await
    }

    async fn next_message(&mut self) -> Result<PeerEvent, SignalingError> {
        let event = self.inner.next_message().await?;
        if let Ok(mut p) = self.progress.lock() {
            match &event {
                PeerEvent::IdAssigned(_) => p.joined = true,
                // NewPeer reaches the peer that must send the offer; a Signal
                // from an unknown sender is the offer reaching the other one.
                PeerEvent::NewPeer(id) => p.saw(*id),
                PeerEvent::Signal { sender, .. } => p.saw(*sender),
                PeerEvent::PeerLeft(id) => p.peers.retain(|x| x != id),
            }
        }
        Ok(event)
    }
}

/// Milliseconds on a monotonic-enough clock (only differences are used).
#[cfg(not(target_arch = "wasm32"))]
fn clock_ms() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}

#[cfg(target_arch = "wasm32")]
fn clock_ms() -> u64 {
    js_sys::Date::now() as u64
}

/// matchbox WebRTC socket as a [`Transport`] to exactly one peer.
pub struct MatchboxTransport {
    socket: WebRtcSocket,
    loop_handle: LoopHandle,
    peer: Option<PeerId>,
    state: LinkState,
    progress: SharedProgress,
    stage: ConnectStage,
    stage_since_ms: u64,
    room_url: String,
}

impl std::fmt::Debug for MatchboxTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatchboxTransport")
            .field("peer", &self.peer)
            .field("state", &self.state)
            .field("stage", &self.stage)
            .finish_non_exhaustive()
    }
}

fn describe(e: &matchbox_socket::Error) -> String {
    match e {
        matchbox_socket::Error::ConnectionFailed(s) => {
            format!(
                "could not connect to the signal server ({s}); check that z2-signal is \
                 running and that the signal URL is right"
            )
        }
        matchbox_socket::Error::Disconnected(s) => format!("signal server disconnected: {s}"),
    }
}

fn loop_end_state(result: Result<(), String>) -> LinkState {
    match result {
        Ok(()) => LinkState::Closed("signalling loop ended".into()),
        // z2-signal answers a third joiner with HTTP 409 Conflict.
        Err(m) if m.contains("409") => LinkState::RoomFull,
        Err(m) => LinkState::Closed(m),
    }
}

/// The server part of a room URL (`ws://host:port`), for error messages.
fn server_of(room_url: &str) -> &str {
    let scheme_end = room_url.find("://").map_or(0, |i| i + 3);
    match room_url[scheme_end..].find('/') {
        Some(i) => &room_url[..scheme_end + i],
        None => room_url,
    }
}

impl MatchboxTransport {
    /// Open `room_url` (e.g. from [`crate::room_url`]) and start the message
    /// loop. `ice`: `None` = [`crate::DEFAULT_ICE_URLS`]; an [`IceConfig`] with
    /// no URLs = no ICE servers (same machine / LAN only).
    ///
    /// Never blocks and never fails eagerly: problems (bad URL, unreachable
    /// server, room full, a peer link that cannot be established) show up
    /// later through [`Transport::state`], with the phase in
    /// [`Transport::stage`].
    pub fn connect(room_url: &str, ice: Option<IceConfig>) -> Self {
        let ice = ice.unwrap_or_default();
        let progress = SharedProgress::default();
        let (socket, fut) = WebRtcSocketBuilder::new(room_url)
            .add_reliable_channel()
            .add_unreliable_channel()
            // Fail a refused connection after one retry (about 3 s) instead of
            // three; SIGNALLING_TIMEOUT_MS covers a server that never answers.
            .reconnect_attempts(Some(2))
            .ice_server(RtcIceServerConfig {
                urls: ice.urls,
                username: ice.username,
                credential: ice.credential,
            })
            .signaller_builder(Arc::new(ObservedSignallerBuilder {
                inner: DefaultSignallerBuilder::default(),
                progress: Arc::clone(&progress),
            }))
            .build();
        let (loop_handle, state) = spawn_loop(fut);
        let stage = ConnectStage::from_link(&state);
        Self {
            socket,
            loop_handle,
            peer: None,
            state,
            progress,
            stage,
            stage_since_ms: clock_ms(),
            room_url: room_url.to_string(),
        }
    }

    /// The chosen remote peer's id, once connected.
    pub fn peer_id(&self) -> Option<String> {
        self.peer.map(|p| p.to_string())
    }

    /// Our id as assigned by the signalling server.
    pub fn local_id(&mut self) -> Option<String> {
        self.socket.id().map(|p| p.to_string())
    }

    /// Close the data channels (the message loop then winds down and the
    /// signalling connection closes). Safe at any stage and more than once.
    pub fn close(&mut self) {
        self.socket.close();
        self.set_terminal(LinkState::Closed("closed locally".into()));
    }

    fn set_terminal(&mut self, st: LinkState) {
        if !self.state.is_terminal() {
            self.state = st;
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn check_loop(&mut self) {
        if self.loop_handle.as_ref().is_some_and(|h| h.is_finished()) {
            if let Some(h) = self.loop_handle.take() {
                let result = h
                    .join()
                    .unwrap_or_else(|_| Err("matchbox message loop panicked".into()));
                self.set_terminal(loop_end_state(result));
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn check_loop(&mut self) {
        let finished = self.loop_handle.borrow_mut().take();
        if let Some(result) = finished {
            self.set_terminal(loop_end_state(result));
        }
    }

    /// Re-derive the stage and enforce the per-stage deadlines.
    fn update_stage(&mut self) {
        let now = clock_ms();
        let stage = if self.state.is_terminal() {
            ConnectStage::Ended
        } else if self.peer.is_some() {
            ConnectStage::Connected
        } else {
            match self.progress.lock() {
                Ok(p) if !p.peers.is_empty() => ConnectStage::EstablishingLink,
                Ok(p) if p.joined => ConnectStage::WaitingForPeer,
                _ => ConnectStage::Signalling,
            }
        };
        if stage != self.stage {
            self.stage = stage;
            self.stage_since_ms = now;
        }
        let waited = now.saturating_sub(self.stage_since_ms);
        let timeout = match self.stage {
            ConnectStage::Signalling if waited >= SIGNALLING_TIMEOUT_MS => Some(format!(
                "could not join the room on the signal server {} within {} s; check that \
                 z2-signal is running there and that the signal URL is right",
                server_of(&self.room_url),
                SIGNALLING_TIMEOUT_MS / 1000
            )),
            ConnectStage::EstablishingLink if waited >= LINK_TIMEOUT_MS => Some(format!(
                "the other player is in the room but the peer link did not open within {} s; \
                 on one machine or LAN set ICE servers to 'none' and check the firewall, \
                 across the internet make sure UDP is not blocked or add a TURN server \
                 (see README.md)",
                LINK_TIMEOUT_MS / 1000
            )),
            _ => None,
        };
        if let Some(msg) = timeout {
            self.socket.close();
            self.set_terminal(LinkState::Closed(msg));
            self.stage = ConnectStage::Ended;
            self.stage_since_ms = now;
        }
    }
}

impl Drop for MatchboxTransport {
    fn drop(&mut self) {
        // Ends the message loop, which closes the signalling connection, so a
        // peer that leaves mid-connect frees its place in the room at once.
        self.socket.close();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_loop(fut: matchbox_socket::MessageLoopFuture) -> (LoopHandle, LinkState) {
    let spawned = std::thread::Builder::new()
        .name("z2-net-matchbox".into())
        .spawn(move || futures::executor::block_on(fut).map_err(|e| describe(&e)));
    match spawned {
        Ok(h) => (Some(h), LinkState::Connecting),
        Err(e) => (
            None,
            LinkState::Closed(format!("could not start network thread: {e}")),
        ),
    }
}

#[cfg(target_arch = "wasm32")]
fn spawn_loop(fut: matchbox_socket::MessageLoopFuture) -> (LoopHandle, LinkState) {
    let cell: LoopHandle = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = std::rc::Rc::clone(&cell);
    wasm_bindgen_futures::spawn_local(async move {
        let result = fut.await.map_err(|e| describe(&e));
        *slot.borrow_mut() = Some(result);
    });
    (cell, LinkState::Connecting)
}

impl Transport for MatchboxTransport {
    fn send(&mut self, channel: Channel, bytes: &[u8]) -> Result<(), TransportError> {
        if self.state.is_terminal() {
            return Err(TransportError::Closed);
        }
        let Some(peer) = self.peer else {
            return Err(TransportError::NotConnected);
        };
        let ch = self
            .socket
            .get_channel_mut(channel.index())
            .map_err(|e| TransportError::Backend(e.to_string()))?;
        ch.try_send(bytes.to_vec().into_boxed_slice(), peer)
            .map_err(|e| TransportError::Backend(e.to_string()))
    }

    fn recv(&mut self) -> Vec<(Channel, Vec<u8>)> {
        self.check_loop();
        // Err means the loop is gone; its result (with the reason) is picked
        // up by check_loop on this or the next call.
        if let Ok(changes) = self.socket.try_update_peers() {
            for (id, st) in changes {
                match st {
                    PeerState::Connected => {
                        if self.peer.is_none() && !self.state.is_terminal() {
                            self.peer = Some(id);
                            self.state = LinkState::Connected;
                        }
                    }
                    PeerState::Disconnected => {
                        if self.peer == Some(id) {
                            self.set_terminal(LinkState::PeerLeft);
                        }
                    }
                }
            }
        }
        self.update_stage();
        let mut out = Vec::new();
        for channel in [Channel::Reliable, Channel::Unreliable] {
            if let Ok(ch) = self.socket.get_channel_mut(channel.index()) {
                for (from, packet) in ch.receive() {
                    if Some(from) == self.peer {
                        out.push((channel, packet.into_vec()));
                    }
                }
            }
        }
        out
    }

    fn state(&self) -> LinkState {
        self.state.clone()
    }

    fn stage(&self) -> ConnectStage {
        self.stage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_of_strips_the_room_path() {
        assert_eq!(
            server_of("ws://127.0.0.1:3536/z2-abc"),
            "ws://127.0.0.1:3536"
        );
        assert_eq!(server_of("wss://example.org/z2-x"), "wss://example.org");
        assert_eq!(server_of("ws://host"), "ws://host");
    }

    #[test]
    fn a_409_is_room_full() {
        assert_eq!(
            loop_end_state(Err("HTTP error: 409 Conflict".into())),
            LinkState::RoomFull
        );
    }
}
