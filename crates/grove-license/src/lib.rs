//! grove-license — offline verification of Grove Pro / Teams license keys.
//!
//! Keys are minted and Ed25519-signed by the store (elyracode.com / elyra-web)
//! and verified here **offline** against a baked-in public key. No network call
//! is needed to check a license, so Pro features keep working without a
//! connection; an online revocation check (with a long grace period) is layered
//! on separately.
//!
//! Key format (a single pasteable string):
//! ```text
//! GROVE-<base64url(payload_json)>.<base64url(ed25519_sig)>
//! ```
//! where `payload_json` is [`LicenseClaims`] and the signature is over the
//! `base64url(payload_json)` bytes.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

/// The store's Ed25519 public key (hex). The matching secret seed lives only on
/// elyra-web. Replace this for production before shipping paid builds.
pub const PUBLIC_KEY_HEX: &str = "be6f87b6fff94f8c120256d20019ce92b970e2a18c6316f6ca705105bce5bf9f";

/// The human-facing prefix every key carries.
pub const KEY_PREFIX: &str = "GROVE-";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LicenseError {
    #[error("not a Grove license key")]
    Malformed,
    #[error("license signature is invalid")]
    BadSignature,
    #[error("license has expired")]
    Expired,
    #[error("license verifier is misconfigured")]
    Misconfigured,
}

/// The signed contents of a license key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LicenseClaims {
    /// Format version.
    pub v: u8,
    /// Unique license id (also the record id on the store).
    pub id: String,
    /// Plan: "pro" or "teams".
    pub plan: String,
    /// Seats included (1 for solo Pro).
    #[serde(default = "one")]
    pub seats: u32,
    /// The customer's email (whom the license was issued to).
    pub email: String,
    /// Issued-at (unix seconds).
    pub iat: i64,
    /// Expiry (unix seconds).
    pub exp: i64,
}

fn one() -> u32 {
    1
}

impl LicenseClaims {
    pub fn is_teams(&self) -> bool {
        self.plan.eq_ignore_ascii_case("teams")
    }
    pub fn is_pro(&self) -> bool {
        self.plan.eq_ignore_ascii_case("pro") || self.is_teams()
    }
}

/// Verify a license key against the baked-in public key at time `now_unix`.
pub fn verify(key: &str, now_unix: i64) -> Result<LicenseClaims> {
    verify_with(key, PUBLIC_KEY_HEX, now_unix)
}

/// Verify against an explicit public key (hex) — used in tests and by tooling.
pub fn verify_with(key: &str, public_key_hex: &str, now_unix: i64) -> Result<LicenseClaims> {
    let verifying = verifying_key(public_key_hex)?;

    let body = key
        .strip_prefix(KEY_PREFIX)
        .ok_or(LicenseError::Malformed)?;
    let (msg_b64, sig_b64) = body.split_once('.').ok_or(LicenseError::Malformed)?;

    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| LicenseError::Malformed)?;
    let sig_arr: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .map_err(|_| LicenseError::Malformed)?;
    let signature = Signature::from_bytes(&sig_arr);

    verifying
        .verify_strict(msg_b64.as_bytes(), &signature)
        .map_err(|_| LicenseError::BadSignature)?;

    let payload = URL_SAFE_NO_PAD
        .decode(msg_b64)
        .map_err(|_| LicenseError::Malformed)?;
    let claims: LicenseClaims =
        serde_json::from_slice(&payload).map_err(|_| LicenseError::Malformed)?;

    if claims.exp <= now_unix {
        return Err(LicenseError::Expired);
    }
    Ok(claims)
}

fn verifying_key(public_key_hex: &str) -> Result<VerifyingKey> {
    let bytes = hex_decode(public_key_hex).ok_or(LicenseError::Misconfigured)?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| LicenseError::Misconfigured)?;
    VerifyingKey::from_bytes(&arr).map_err(|_| LicenseError::Misconfigured)
}

/// Decode an even-length hex string.
///
/// Works on bytes rather than string slices. The previous version sliced
/// `&s[i..i + 2]`, which panics when the index is not a UTF-8 character
/// boundary — `hex_decode("€x")` was a panic rather than a `None`. Only ever
/// called with a compile-time constant today, so it was latent, but a public
/// function that panics on its documented input type is a trap for the next
/// caller.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .chunks(2)
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        })
        .collect()
}

pub type Result<T> = std::result::Result<T, LicenseError>;

#[cfg(test)]
mod tests {
    /// Regression: the old decoder sliced `&s[i..i + 2]`, which panics when the
    /// index is not a UTF-8 boundary. Multi-byte input must be `None`, not a
    /// crash.
    #[test]
    fn non_ascii_hex_is_refused_not_panicked_on() {
        for input in [
            "\u{20ac}x",
            "\u{20ac}\u{20ac}",
            "ab\u{e9}d",
            "\u{1F600}\u{1F600}",
        ] {
            assert_eq!(hex_decode(input), None, "{input:?} should decode to None");
        }
    }

    #[test]
    fn valid_hex_still_decodes() {
        assert_eq!(hex_decode("00ff10"), Some(vec![0x00, 0xff, 0x10]));
        assert_eq!(hex_decode("DEADBEEF"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
        assert_eq!(hex_decode(""), Some(vec![]));
        assert_eq!(hex_decode("abc"), None, "odd length");
        assert_eq!(hex_decode("zz"), None, "not hex digits");
        // The real public key must still parse, or nothing verifies.
        assert!(hex_decode(PUBLIC_KEY_HEX).is_some());
    }

    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use ed25519_dalek::{Signer, SigningKey};

    /// Mint a key the way elyra-web will, then verify it here — the round trip.
    fn mint(seed: &[u8; 32], claims: &LicenseClaims) -> (String, String) {
        let sk = SigningKey::from_bytes(seed);
        let pk_hex: String = sk
            .verifying_key()
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let payload = serde_json::to_vec(claims).unwrap();
        let msg_b64 = URL_SAFE_NO_PAD.encode(payload);
        let sig = sk.sign(msg_b64.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_bytes());
        (format!("{KEY_PREFIX}{msg_b64}.{sig_b64}"), pk_hex)
    }

    fn claims(exp: i64) -> LicenseClaims {
        LicenseClaims {
            v: 1,
            id: "lic_123".into(),
            plan: "teams".into(),
            seats: 5,
            email: "kh@gets.no".into(),
            iat: 1_000,
            exp,
        }
    }

    #[test]
    fn round_trip_valid() {
        let seed = [7u8; 32];
        let (key, pk_hex) = mint(&seed, &claims(2_000_000_000));
        let got = verify_with(&key, &pk_hex, 1_700_000_000).unwrap();
        assert_eq!(got.email, "kh@gets.no");
        assert!(got.is_teams() && got.is_pro());
        assert_eq!(got.seats, 5);
    }

    #[test]
    fn rejects_expired() {
        let seed = [7u8; 32];
        let (key, pk_hex) = mint(&seed, &claims(1_500_000_000));
        assert_eq!(
            verify_with(&key, &pk_hex, 1_700_000_000),
            Err(LicenseError::Expired)
        );
    }

    #[test]
    fn rejects_tampered_payload() {
        let seed = [7u8; 32];
        let (key, pk_hex) = mint(&seed, &claims(2_000_000_000));
        // Flip the plan by re-encoding a different payload but keeping the sig.
        let body = key.strip_prefix(KEY_PREFIX).unwrap();
        let (_msg, sig) = body.split_once('.').unwrap();
        let forged_payload = serde_json::to_vec(&claims(2_000_000_000)).unwrap();
        let forged_msg = URL_SAFE_NO_PAD.encode({
            let mut p = forged_payload;
            p.extend_from_slice(b" "); // any change
            p
        });
        let forged = format!("{KEY_PREFIX}{forged_msg}.{sig}");
        assert_eq!(
            verify_with(&forged, &pk_hex, 1_700_000_000),
            Err(LicenseError::BadSignature)
        );
    }

    #[test]
    fn rejects_wrong_key_prefix() {
        assert_eq!(verify_with("nope", &"00".repeat(32), 0), {
            // wrong public key length is fine; prefix check happens after key parse
            Err(LicenseError::Malformed)
        });
    }
}
