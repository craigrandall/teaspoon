//! PST password check (MS-PST 2.4.3.3): the CRC-32 of the password is stored in
//! PidTagPstPassword on the message store. The scheme (reflected 0xEDB88320, initial value 0,
//! no final XOR, over ANSI bytes) was verified against a real protected PST on 2026-10-31.

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
}

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

/// Refuses (with an error that never contains the password or the stored hash) unless the
/// PST is unprotected or the supplied password verifies.
pub(crate) fn enforce(store: &dyn Store, supplied: Option<String>) -> Result<PasswordOutcome> {
    let outcome = evaluate(stored_crc(store)?, supplied.as_deref());
    match outcome {
        PasswordOutcome::NotProtected | PasswordOutcome::Verified => Ok(outcome),
        PasswordOutcome::Missing => {
            bail!("the PST is password protected; set {ENV_VAR} to its password")
        }
        PasswordOutcome::Wrong => bail!("the password in {ENV_VAR} does not match this PST"),
        PasswordOutcome::UnsupportedEncoding => {
            bail!(
                "the password in {ENV_VAR} contains non-ASCII characters; verifying those is not supported yet"
            )
        }
    }
}

#[cfg(test)]
mod tests {
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
}
