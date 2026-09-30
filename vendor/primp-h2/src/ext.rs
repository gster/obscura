//! Extensions specific to the HTTP/2 protocol.

use crate::hpack::BytesStr;

use bytes::Bytes;
use std::fmt;

/// An initial HEADERS-frame weight, independent of HTTP Priority fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeadersWeight(u8);

impl HeadersWeight {
    /// Accepts protocol weights 1 through 256, not their encoded byte value.
    pub fn new(weight: u16) -> Option<Self> {
        (1..=256).contains(&weight).then(|| Self((weight - 1) as u8))
    }

    /// Returns the protocol weight.
    pub fn weight(self) -> u16 { u16::from(self.0) + 1 }

    pub(crate) fn encoded(self) -> u8 { self.0 }
}

/// Chromium's legacy HTTP/2 dependency band for a browser-owned request.
/// Zero is highest. This is separate from both frame weight and RFC 9218.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeadersPriority(u8);

impl HeadersPriority {
    /// Accepts Chromium legacy priority bands zero through seven.
    pub fn new(priority: u8) -> Option<Self> {
        (priority <= 7).then_some(Self(priority))
    }

    pub(crate) fn band(self) -> usize { usize::from(self.0) }
}

/// An automatic RFC 9218 field projected only after HTTP/2 is selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExtensiblePriority { urgency: u8, incremental: bool }

impl ExtensiblePriority {
    /// Accepts RFC 9218 urgency values zero through seven.
    pub fn new(urgency: u8, incremental: bool) -> Option<Self> {
        (urgency <= 7).then_some(Self { urgency, incremental })
    }

    /// Default urgency and nonincremental delivery serialize to no field.
    pub fn field_value(self) -> Option<String> {
        match (self.urgency, self.incremental) {
            (3, false) => None,
            (3, true) => Some("i".into()),
            (urgency, false) => Some(format!("u={urgency}")),
            (urgency, true) => Some(format!("u={urgency}, i")),
        }
    }
}

/// Represents the `:protocol` pseudo-header used by
/// the [Extended CONNECT Protocol].
///
/// [Extended CONNECT Protocol]: https://datatracker.ietf.org/doc/html/rfc8441#section-4
#[derive(Clone, Eq, PartialEq)]
pub struct Protocol {
    value: BytesStr,
}

impl Protocol {
    /// Converts a static string to a protocol name.
    pub const fn from_static(value: &'static str) -> Self {
        Self {
            value: BytesStr::from_static(value),
        }
    }

    /// Returns a str representation of the header.
    pub fn as_str(&self) -> &str {
        self.value.as_str()
    }

    pub(crate) fn try_from(bytes: Bytes) -> Result<Self, std::str::Utf8Error> {
        Ok(Self {
            value: BytesStr::try_from(bytes)?,
        })
    }
}

impl<'a> From<&'a str> for Protocol {
    fn from(value: &'a str) -> Self {
        Self {
            value: BytesStr::from(value),
        }
    }
}

impl AsRef<[u8]> for Protocol {
    fn as_ref(&self) -> &[u8] {
        self.value.as_ref()
    }
}

impl fmt::Debug for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.value.fmt(f)
    }
}
