#![warn(missing_docs)]
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
// z2rs patch: upstream 0.14.0 still calls the deprecated `try_next`; keep
// builds of the vendored copy as quiet as the registry copy.
#![allow(deprecated)]

mod error;
#[cfg(feature = "ggrs")]
mod ggrs_socket;
mod webrtc_socket;

pub use async_trait;
pub use error::{Error, SignalingError};
pub use matchbox_protocol::PeerId;
pub use webrtc_socket::{
    ChannelConfig, DefaultSignallerBuilder, MessageLoopFuture, Packet, PeerEvent, PeerRequest, PeerSignal, PeerState,
    RtcIceServerConfig, Signaller, SignallerBuilder, WebRtcChannel, WebRtcSocket,
    WebRtcSocketBuilder, error::ChannelError,
};
