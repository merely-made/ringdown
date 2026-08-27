//! The version banner read at connection time.
//!
//! Before any JSON-RPC happens, the client *reads* the response characteristic
//! once and gets back a short plain-text banner naming the firmware versions of
//! both processors. This is separate from the `GetVersion` RPC method and comes
//! earlier: it arrives during connection setup, over a GATT read rather than a
//! notification.
//!
//! Two formats exist, because the older one could only report one processor:
//!
//! ```text
//! S1.2.3_E2.7.0\n     both versions: STM (audio DSP), then ESP (connectivity)
//! @version 2.7.0\n    older firmware: ESP only; STM is implicitly 1.2.2
//! ```
//!
//! # Provenance
//!
//! Recovered by static analysis; unconfirmed against hardware until the Phase 1
//! control run. See `design_docs/2026-08-27_antinode_founding.md`, Findings F9
//! and F10.

use core::fmt;

/// The implied STM version when a device reports only its ESP version.
///
/// Firmware old enough to use the `@version` form predates the STM being
/// reported at all, and the vendor's client assumes this value. Adopted here so
/// both banner forms yield a complete pair, but treat it as an assumption
/// inherited from the vendor rather than something the device said.
pub const IMPLIED_STM_VERSION: Version = Version {
    major: 1,
    minor: 2,
    patch: 2,
};

/// A three-part firmware version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    /// Major version.
    pub major: u16,
    /// Minor version.
    pub minor: u16,
    /// Patch version; zero when the device reported only two components.
    pub patch: u16,
}

impl Version {
    /// Parse a dotted version, tolerating missing trailing components.
    ///
    /// `"2.7"` yields `2.7.0`, matching how the vendor's client reads these.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let mut parts = text.split('.');
        let major = parts.next()?.trim().parse().ok()?;
        let minor = parts
            .next()
            .and_then(|p| p.trim().parse().ok())
            .unwrap_or(0);
        let patch = parts
            .next()
            .and_then(|p| p.trim().parse().ok())
            .unwrap_or(0);
        if parts.next().is_some() {
            return None;
        }
        Some(Version {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The firmware versions of the guitar's two processors.
///
/// The instrument runs two: one handles connectivity and speaks this protocol,
/// the other runs the audio DSP. They are flashed independently and version
/// independently, which is why both are reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Banner {
    /// The audio DSP processor's firmware version.
    pub stm: Version,
    /// The connectivity processor's firmware version.
    pub esp: Version,
    /// Whether `stm` was actually reported, or assumed because the device used
    /// the older single-version banner.
    pub stm_was_implied: bool,
}

impl Banner {
    /// Parse a banner read from the response characteristic.
    pub fn parse(text: &str) -> Option<Banner> {
        let text = text.trim_end_matches(['\n', '\r']);

        if let Some(rest) = text.strip_prefix('S') {
            let (stm, esp) = rest.split_once("_E")?;
            return Some(Banner {
                stm: Version::parse(stm)?,
                esp: Version::parse(esp)?,
                stm_was_implied: false,
            });
        }

        if let Some(rest) = text.strip_prefix("@version ") {
            return Some(Banner {
                stm: IMPLIED_STM_VERSION,
                esp: Version::parse(rest)?,
                stm_was_implied: true,
            });
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn parses_the_two_processor_banner() {
        let b = Banner::parse("S1.2.3_E2.7.0\n").unwrap();
        assert_eq!(
            b.stm,
            Version {
                major: 1,
                minor: 2,
                patch: 3
            }
        );
        assert_eq!(
            b.esp,
            Version {
                major: 2,
                minor: 7,
                patch: 0
            }
        );
        assert!(!b.stm_was_implied);
    }

    #[test]
    fn parses_the_legacy_single_version_banner_and_flags_the_assumption() {
        let b = Banner::parse("@version 2.7.0\n").unwrap();
        assert_eq!(
            b.esp,
            Version {
                major: 2,
                minor: 7,
                patch: 0
            }
        );
        assert_eq!(b.stm, IMPLIED_STM_VERSION);
        assert!(
            b.stm_was_implied,
            "a caller must be able to tell an assumed version from a reported one"
        );
    }

    #[test]
    fn tolerates_a_missing_newline_and_crlf() {
        assert!(Banner::parse("S1.2.3_E2.7.0").is_some());
        assert!(Banner::parse("S1.2.3_E2.7.0\r\n").is_some());
    }

    #[test]
    fn short_versions_fill_with_zero() {
        assert_eq!(
            Version::parse("2.7"),
            Some(Version {
                major: 2,
                minor: 7,
                patch: 0
            })
        );
        assert_eq!(
            Version::parse("3"),
            Some(Version {
                major: 3,
                minor: 0,
                patch: 0
            })
        );
    }

    #[test]
    fn rejects_things_that_are_not_banners() {
        // A JSON-RPC reply must not be mistaken for a banner.
        assert!(Banner::parse(r#"{"jsonrpc":"2.0","id":1}"#).is_none());
        assert!(Banner::parse("").is_none());
        assert!(
            Banner::parse("S1.2.3").is_none(),
            "missing the _E separator"
        );
        assert!(Banner::parse("Snonsense_Emore").is_none());
        assert!(Banner::parse("@version ").is_none());
        assert!(Version::parse("1.2.3.4").is_none());
    }

    #[test]
    fn versions_display_and_order() {
        assert_eq!(format!("{}", Version::parse("2.7.1").unwrap()), "2.7.1");
        assert!(Version::parse("2.7.0") < Version::parse("2.7.1"));
        assert!(Version::parse("1.9.9") < Version::parse("2.0.0"));
    }
}
