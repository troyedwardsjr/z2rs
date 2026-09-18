//! ICE server settings (STUN / TURN) shared by every frontend.
//!
//! One text form is used everywhere a person types ICE servers (the browser
//! panel's ICE field, the desktop `--ice` flag and its `netplay.ice_url`
//! config key), so the same string works in both apps:
//!
//! ```text
//! stun:stun.l.google.com:19302 turn:turn.example.com:3478 username=alice credential=secret
//! ```
//!
//! * URLs are separated by spaces or commas and must start with `stun:`,
//!   `stuns:`, `turn:` or `turns:`.
//! * `username=` / `credential=` apply to the TURN URLs (WebRTC takes one
//!   credential pair per server entry, and the transport opens one entry).
//! * An empty string or `none` means **no ICE servers**: host candidates only,
//!   which is enough on one machine or one LAN and connects fastest.
//! * `default` means [`DEFAULT_ICE_URLS`].
//!
//! Pure string handling, no dependencies, available without the `matchbox`
//! feature so frontends can validate input before opening anything.

/// STUN servers used when nothing is configured (the same pair matchbox uses).
///
/// STUN only discovers each peer's public address; it relays nothing. Peers
/// behind symmetric NATs or strict firewalls additionally need a TURN server
/// (see README.md).
pub const DEFAULT_ICE_URLS: [&str; 2] = [
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302",
];

/// The [`DEFAULT_ICE_URLS`] in the text form [`IceConfig::parse`] reads.
pub const DEFAULT_ICE_SPEC: &str = "stun:stun.l.google.com:19302 stun:stun1.l.google.com:19302";

/// One ICE server entry: its URLs plus optional TURN credentials.
#[derive(Clone, PartialEq, Eq)]
pub struct IceConfig {
    /// e.g. `["turn:turn.example.com:3478"]`. Empty = no ICE servers.
    pub urls: Vec<String>,
    /// TURN username.
    pub username: Option<String>,
    /// TURN credential. Never logged by this crate.
    pub credential: Option<String>,
}

// Hand-written so the credential never reaches a log line through `{:?}`.
impl std::fmt::Debug for IceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IceConfig")
            .field("urls", &self.urls)
            .field("username", &self.username)
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl Default for IceConfig {
    /// [`DEFAULT_ICE_URLS`], no credentials.
    fn default() -> Self {
        Self {
            urls: DEFAULT_ICE_URLS.iter().map(|u| (*u).to_string()).collect(),
            username: None,
            credential: None,
        }
    }
}

/// Why an ICE setting was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IceParseError(String);

impl std::fmt::Display for IceParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ICE servers: {} (expected space-separated stun:/turn: URLs, optional \
             username=... credential=..., or 'none' for same machine / LAN only)",
            self.0
        )
    }
}

impl std::error::Error for IceParseError {}

impl IceConfig {
    /// No ICE servers: host candidates only (same machine or same LAN).
    pub fn none() -> Self {
        Self {
            urls: Vec::new(),
            username: None,
            credential: None,
        }
    }

    /// True when no ICE server is configured.
    pub fn is_none(&self) -> bool {
        self.urls.is_empty()
    }

    /// Parse the shared text form (see the module docs).
    pub fn parse(spec: &str) -> Result<Self, IceParseError> {
        let mut out = Self::none();
        for token in spec.split([' ', ',', '\t', '\n']).filter(|t| !t.is_empty()) {
            if let Some(v) = token.strip_prefix("username=") {
                out.username = Some(v.to_string());
            } else if let Some(v) = token.strip_prefix("credential=") {
                out.credential = Some(v.to_string());
            } else if token.eq_ignore_ascii_case("none") {
                // Explicit "no servers"; only meaningful alone.
            } else if token.eq_ignore_ascii_case("default") {
                out.urls
                    .extend(DEFAULT_ICE_URLS.iter().map(|u| (*u).to_string()));
            } else {
                let lower = token.to_ascii_lowercase();
                let known = ["stun:", "stuns:", "turn:", "turns:"]
                    .iter()
                    .any(|p| lower.starts_with(p) && lower.len() > p.len());
                if !known {
                    return Err(IceParseError(format!("'{token}' is not a stun:/turn: URL")));
                }
                out.urls.push(token.to_string());
            }
        }
        if out.urls.is_empty() && (out.username.is_some() || out.credential.is_some()) {
            return Err(IceParseError(
                "username/credential given without a turn: URL".to_string(),
            ));
        }
        let has_turn = out
            .urls
            .iter()
            .any(|u| u.to_ascii_lowercase().starts_with("turn"));
        if has_turn && (out.username.is_none() || out.credential.is_none()) {
            return Err(IceParseError(
                "a turn: URL needs username=... and credential=...".to_string(),
            ));
        }
        Ok(out)
    }

    /// Short, credential-free description for status lines and logs.
    pub fn describe(&self) -> String {
        if self.urls.is_empty() {
            return "no ICE servers (same machine / LAN only)".to_string();
        }
        let turn = self
            .urls
            .iter()
            .any(|u| u.to_ascii_lowercase().starts_with("turn"));
        format!(
            "{} ICE server URL{}{}",
            self.urls.len(),
            if self.urls.len() == 1 { "" } else { "s" },
            if turn { " incl. TURN" } else { "" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_none_mean_no_servers() {
        assert!(IceConfig::parse("").unwrap().is_none());
        assert!(IceConfig::parse("  none ").unwrap().is_none());
        assert!(IceConfig::none().is_none());
    }

    #[test]
    fn default_spec_round_trips() {
        assert_eq!(
            IceConfig::parse(DEFAULT_ICE_SPEC).unwrap(),
            IceConfig::default()
        );
        assert_eq!(IceConfig::parse("default").unwrap(), IceConfig::default());
    }

    #[test]
    fn urls_and_credentials_parse() {
        let c = IceConfig::parse(
            "stun:a.example:3478, turn:b.example:3478?transport=udp username=u credential=p",
        )
        .unwrap();
        assert_eq!(
            c.urls,
            vec!["stun:a.example:3478", "turn:b.example:3478?transport=udp"]
        );
        assert_eq!(c.username.as_deref(), Some("u"));
        assert_eq!(c.credential.as_deref(), Some("p"));
        assert!(c.describe().contains("TURN"));
    }

    #[test]
    fn rejects_bad_input_with_a_reason() {
        let e = IceConfig::parse("http://nope").unwrap_err().to_string();
        assert!(e.contains("http://nope"), "{e}");
        let e = IceConfig::parse("turn:x.example").unwrap_err().to_string();
        assert!(e.contains("username"), "{e}");
        assert!(IceConfig::parse("username=u").is_err());
        assert!(IceConfig::parse("stun:").is_err());
    }

    #[test]
    fn debug_never_prints_the_credential() {
        let c = IceConfig::parse("turn:x.example username=u credential=hunter2").unwrap();
        assert!(!format!("{c:?}").contains("hunter2"));
    }
}
