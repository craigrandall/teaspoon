//! PST password check (MS-PST 2.4.3.3): the CRC-32 of the password is stored in
//! PidTagPstPassword on the message store. The scheme (reflected 0xEDB88320, initial value 0,
//! no final XOR, over ANSI bytes) was verified against a real protected PST on 2026-10-31.

use std::fmt;

use anyhow::{Result, bail};
use outlook_pst::ltp::prop_context::PropertyValue;
use outlook_pst::messaging::store::Store;

const PROP_PST_PASSWORD: u16 = 0x67FF;
pub(crate) const ENV_VAR: &str = "TSP_PST_PASSWORD";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PasswordOutcome {
    NotProtected,
    Verified,
    Missing,
    Wrong,
    UnsupportedEncoding,
}

impl PasswordOutcome {
    /// Value of the privacy-safe `pst_password=` report key.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::NotProtected => "none",
            Self::Verified => "verified",
            _ => "refused",
        }
    }

    /// The refusal this outcome calls for, or `None` when the run may go on.
    pub(crate) fn refusal(self) -> Option<PasswordRefusal> {
        match self {
            Self::NotProtected | Self::Verified => None,
            Self::Missing => Some(PasswordRefusal::Missing),
            Self::Wrong => Some(PasswordRefusal::Wrong),
            Self::UnsupportedEncoding => Some(PasswordRefusal::UnsupportedEncoding),
        }
    }
}

/// A protected PST that `tsp` declines to read: the password is missing, wrong, or cannot be
/// verified. It is a typed error so that `main` can give it its own exit status (2, the status
/// of any refusal, see `EXIT_REFUSED`); every other failure stays a plain error (1). It travels
/// inside an `anyhow::Error`, and `downcast_ref` finds it however much context was added.
/// The messages never contain the password or the stored hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PasswordRefusal {
    Missing,
    Wrong,
    UnsupportedEncoding,
}

impl fmt::Display for PasswordRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => {
                write!(
                    f,
                    "the PST is password protected; set {ENV_VAR} to its password"
                )
            }
            Self::Wrong => write!(f, "the password in {ENV_VAR} does not match this PST"),
            Self::UnsupportedEncoding => write!(
                f,
                "the password in {ENV_VAR} contains non-ASCII characters; verifying those is not supported yet"
            ),
        }
    }
}

impl std::error::Error for PasswordRefusal {}

pub(crate) fn pst_password_crc(password: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in password {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc
}

/// Pure decision logic, separate from the store so it can be unit-tested without a PST.
pub(crate) fn evaluate(stored_crc: u32, supplied: Option<&str>) -> PasswordOutcome {
    if stored_crc == 0 {
        return PasswordOutcome::NotProtected;
    }
    let Some(password) = supplied.filter(|p| !p.is_empty()) else {
        return PasswordOutcome::Missing;
    };
    if !password.is_ascii() {
        return PasswordOutcome::UnsupportedEncoding;
    }
    if pst_password_crc(password.as_bytes()) == stored_crc {
        PasswordOutcome::Verified
    } else {
        PasswordOutcome::Wrong
    }
}

fn stored_crc(store: &dyn Store) -> Result<u32> {
    match store.properties().get(PROP_PST_PASSWORD) {
        None => Ok(0),
        Some(PropertyValue::Integer32(v)) => Ok(*v as u32),
        Some(_) => {
            bail!("PidTagPstPassword has an unexpected type; cannot verify the PST password")
        }
    }
}

pub(crate) fn supplied_from_env() -> Result<Option<String>> {
    match std::env::var(ENV_VAR) {
        Ok(v) if v.is_empty() => Ok(None),
        Ok(v) => Ok(Some(v)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => bail!("{ENV_VAR} is not valid Unicode"),
    }
}

/// Refuses, with a [`PasswordRefusal`] (which never contains the password or the stored hash),
/// unless the PST is unprotected or the supplied password verifies. Any other failure (a store
/// property of an unexpected type, a variable that is not valid Unicode) is a plain error, and
/// reaches `main` as exit status 1.
pub(crate) fn enforce(store: &dyn Store, supplied: Option<String>) -> Result<PasswordOutcome> {
    let outcome = evaluate(stored_crc(store)?, supplied.as_deref());
    match outcome.refusal() {
        None => Ok(outcome),
        Some(refusal) => Err(anyhow::Error::new(refusal)),
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::*;

    #[test]
    fn crc_vectors() {
        assert_eq!(pst_password_crc(b""), 0);
        assert_eq!(pst_password_crc(b"a"), 0x3AB5_51CE);
        assert_eq!(pst_password_crc(b"abc"), 0xCA65_98D0);
        assert_eq!(pst_password_crc(b"123456789"), 0x2DFD_2D88);
    }

    #[test]
    fn decisions() {
        let stored = pst_password_crc(b"abc");
        assert_eq!(evaluate(0, None), PasswordOutcome::NotProtected);
        assert_eq!(evaluate(0, Some("anything")), PasswordOutcome::NotProtected);
        assert_eq!(evaluate(stored, None), PasswordOutcome::Missing);
        assert_eq!(evaluate(stored, Some("")), PasswordOutcome::Missing);
        assert_eq!(evaluate(stored, Some("abc")), PasswordOutcome::Verified);
        assert_eq!(evaluate(stored, Some("abd")), PasswordOutcome::Wrong);
        assert_eq!(
            evaluate(stored, Some("pässword")),
            PasswordOutcome::UnsupportedEncoding
        );
    }

    #[test]
    fn report_keys() {
        assert_eq!(PasswordOutcome::NotProtected.key(), "none");
        assert_eq!(PasswordOutcome::Verified.key(), "verified");
    }

    #[test]
    fn outcomes_map_to_refusals() {
        assert_eq!(PasswordOutcome::NotProtected.refusal(), None);
        assert_eq!(PasswordOutcome::Verified.refusal(), None);
        assert_eq!(
            PasswordOutcome::Missing.refusal(),
            Some(PasswordRefusal::Missing)
        );
        assert_eq!(
            PasswordOutcome::Wrong.refusal(),
            Some(PasswordRefusal::Wrong)
        );
        assert_eq!(
            PasswordOutcome::UnsupportedEncoding.refusal(),
            Some(PasswordRefusal::UnsupportedEncoding)
        );
    }

    #[test]
    fn exactly_the_refused_outcomes_have_a_refusal() {
        let stored = pst_password_crc(b"abc");
        let cases = [
            evaluate(0, None),
            evaluate(stored, None),
            evaluate(stored, Some("abc")),
            evaluate(stored, Some("abd")),
            evaluate(stored, Some("pässword")),
        ];
        for outcome in cases {
            assert_eq!(
                outcome.refusal().is_some(),
                outcome.key() == "refused",
                "{outcome:?}"
            );
        }
    }

    #[test]
    fn refusal_messages_are_unchanged_and_hold_no_password() {
        assert_eq!(
            PasswordRefusal::Missing.to_string(),
            "the PST is password protected; set TSP_PST_PASSWORD to its password"
        );
        assert_eq!(
            PasswordRefusal::Wrong.to_string(),
            "the password in TSP_PST_PASSWORD does not match this PST"
        );
        assert_eq!(
            PasswordRefusal::UnsupportedEncoding.to_string(),
            "the password in TSP_PST_PASSWORD contains non-ASCII characters; verifying those is not supported yet"
        );
    }

    #[test]
    fn a_refusal_is_found_through_anyhow_and_added_context() {
        let err = anyhow::Error::new(PasswordRefusal::Wrong);
        assert_eq!(
            err.downcast_ref::<PasswordRefusal>(),
            Some(&PasswordRefusal::Wrong)
        );
        let wrapped = Err::<(), anyhow::Error>(err)
            .context("while opening the PST")
            .unwrap_err();
        assert_eq!(
            wrapped.downcast_ref::<PasswordRefusal>(),
            Some(&PasswordRefusal::Wrong)
        );
    }
}
