//! The [`Secret`] newtype: a value that never renders itself.
//!
//! Secrets are a type, not a string (ADR-0011 decision 8). `Debug`, `Display`, and `Serialize`
//! all render `<redacted>`, so a derived `Debug` in a log record cannot leak the value. The
//! plaintext is reachable only through an explicit [`Secret::expose`] call at a wire boundary.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A redacting string wrapper for API keys, tokens, and other credentials.
///
/// ```
/// use mta_domain::Secret;
///
/// let key = Secret::new("sk-live-123");
/// assert_eq!(format!("{key:?}"), "Secret(<redacted>)");
/// assert_eq!(key.to_string(), "<redacted>");
/// assert_eq!(key.expose(), "sk-live-123");
/// ```
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a plaintext secret.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the plaintext. Call this only where the value is about to be used, never when
    /// formatting a record.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Returns `true` when no value is configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("<redacted>")
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_redact() {
        let secret = Secret::new("correct-horse-battery-staple");
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        assert_eq!(secret.to_string(), "<redacted>");
        assert!(!format!("{secret:?}").contains("correct-horse"));
    }

    #[test]
    fn json_redacts_and_round_trips() {
        let secret = Secret::new("sk-live-abc");
        let json = serde_json::to_string(&secret).expect("serialize");
        assert_eq!(json, "\"<redacted>\"");

        let parsed: Secret = serde_json::from_str("\"sk-live-abc\"").expect("deserialize");
        assert_eq!(parsed.expose(), "sk-live-abc");
    }

    #[test]
    fn expose_is_the_only_way_out() {
        let secret = Secret::from("value");
        assert_eq!(secret.expose(), "value");
        assert!(!secret.is_empty());
        assert!(Secret::default().is_empty());
    }
}
