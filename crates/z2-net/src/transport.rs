//! Transport abstraction: two logical channels to exactly one remote peer.

/// Logical channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Channel {
    /// Ordered, retransmitted (matchbox channel 0).
    Reliable,
    /// Unordered, no retransmits (matchbox channel 1).
    Unreliable,
}

impl Channel {
    /// Channel index as added to the matchbox socket builder.
    pub const fn index(self) -> usize {
        match self {
            Channel::Reliable => 0,
            Channel::Unreliable => 1,
        }
    }
}

/// Link status as seen by one end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    /// Waiting for signalling / the remote peer.
    Connecting,
    /// Data channels to the remote peer are open.
    Connected,
    /// The remote peer went away.
    PeerLeft,
    /// The signalling server refused us: the room already has two players.
    RoomFull,
    /// The transport failed or was closed, with a human-readable reason.
    Closed(String),
}

impl LinkState {
    /// PeerLeft, RoomFull or Closed: no further traffic is possible.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            LinkState::PeerLeft | LinkState::RoomFull | LinkState::Closed(_)
        )
    }
}

/// How far a link has come while it connects, for status lines.
///
/// Finer-grained than [`LinkState::Connecting`], which covers everything
/// before the data channels open. Transports that cannot tell the phases apart
/// report [`ConnectStage::Signalling`] until connected (the default
/// [`Transport::stage`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConnectStage {
    /// Connecting to the signalling server / joining the room.
    Signalling,
    /// In the room, waiting for the other player to arrive.
    WaitingForPeer,
    /// The other player is there; the direct peer link (WebRTC ICE + data
    /// channels) is being established.
    EstablishingLink,
    /// The data channels are open.
    Connected,
    /// The link ended (see [`Transport::state`] for why).
    Ended,
}

impl ConnectStage {
    /// Stable lowercase name (used in the web status JSON).
    pub const fn as_str(self) -> &'static str {
        match self {
            ConnectStage::Signalling => "signalling",
            ConnectStage::WaitingForPeer => "waiting",
            ConnectStage::EstablishingLink => "linking",
            ConnectStage::Connected => "connected",
            ConnectStage::Ended => "ended",
        }
    }

    /// Human-readable phrase for status lines.
    pub const fn describe(self) -> &'static str {
        match self {
            ConnectStage::Signalling => "connecting to the signal server",
            ConnectStage::WaitingForPeer => "waiting for the other player",
            ConnectStage::EstablishingLink => "establishing the peer link",
            ConnectStage::Connected => "connected",
            ConnectStage::Ended => "link ended",
        }
    }

    /// The coarse stage implied by a [`LinkState`] alone.
    pub fn from_link(state: &LinkState) -> Self {
        match state {
            LinkState::Connecting => ConnectStage::Signalling,
            LinkState::Connected => ConnectStage::Connected,
            _ => ConnectStage::Ended,
        }
    }
}

/// Send failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// No remote peer yet.
    NotConnected,
    /// The link is gone.
    Closed,
    /// Backend-specific failure.
    Backend(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::NotConnected => f.write_str("not connected"),
            TransportError::Closed => f.write_str("link closed"),
            TransportError::Backend(m) => write!(f, "transport error: {m}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// A non-blocking two-channel link to one peer. No `Send` bound (wasm).
///
/// [`crate::Session::update`] calls `recv`, then `state`, then `send` for
/// queued packets; implementations do their per-tick work in `recv`.
pub trait Transport {
    /// Queue one packet. Unreliable sends may be silently lost.
    fn send(&mut self, channel: Channel, bytes: &[u8]) -> Result<(), TransportError>;
    /// Drain every packet received from the peer since the last call.
    fn recv(&mut self) -> Vec<(Channel, Vec<u8>)>;
    /// Current link status (as of the last `recv`).
    fn state(&self) -> LinkState;
    /// Connection progress (as of the last `recv`), for status lines only;
    /// the session logic never depends on it.
    fn stage(&self) -> ConnectStage {
        ConnectStage::from_link(&self.state())
    }
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn send(&mut self, channel: Channel, bytes: &[u8]) -> Result<(), TransportError> {
        (**self).send(channel, bytes)
    }
    fn recv(&mut self) -> Vec<(Channel, Vec<u8>)> {
        (**self).recv()
    }
    fn state(&self) -> LinkState {
        (**self).state()
    }
    fn stage(&self) -> ConnectStage {
        (**self).stage()
    }
}
