//! Derive the public key (to bake into grove-license) from the signing seed,
//! and mint a sample license key so we can prove the round trip against the
//! elyra-web signer.
//!
//! Usage: `cargo run -p grove-license --example keygen -- <seed_hex_64>`

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};

fn hex_decode(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn main() {
    let seed_hex = std::env::args()
        .nth(1)
        .or_else(|| std::fs::read_to_string("/tmp/grove-license-seed.txt").ok())
        .map(|s| s.trim().to_string())
        .expect("pass the 64-char seed hex as an argument");

    let seed: [u8; 32] = hex_decode(&seed_hex).try_into().expect("seed must be 32 bytes");
    let sk = SigningKey::from_bytes(&seed);
    let pk = sk.verifying_key();
    let pk_hex: String = pk.to_bytes().iter().map(|b| format!("{b:02x}")).collect();

    println!("PUBLIC_KEY_HEX = {pk_hex}");

    // A sample license, minted exactly the way elyra-web will.
    let payload = serde_json::json!({
        "v": 1,
        "id": "lic_sample",
        "plan": "teams",
        "seats": 5,
        "email": "kh@gets.no",
        "iat": 1_700_000_000,
        "exp": 4_100_000_000i64
    });
    let msg_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    let sig = sk.sign(msg_b64.as_bytes());
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_bytes());
    println!("SAMPLE_KEY = GROVE-{msg_b64}.{sig_b64}");
}
