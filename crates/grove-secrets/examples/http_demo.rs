//! Two-member end-to-end secret sync against a running grove-teams backend.
//!
//! Usage:
//!   cargo run -p grove-secrets --features http --example http_demo -- <base_url> <license_key>

use grove_secrets::{HttpStore, Identity, SecretsClient};

fn main() {
    let base = std::env::args().nth(1).expect("base url");
    let token = std::env::args().nth(2).expect("license key");

    // Two teammates share the team license (token) but hold their own identities.
    let alice = Identity::generate();
    let bob = Identity::generate();
    let alice_pub = alice.public();
    let bob_pub = bob.public();

    let alice_client = SecretsClient::new(HttpStore::new(&base, &token), alice);
    let bob_client = SecretsClient::new(HttpStore::new(&base, &token), bob);

    println!("👩 Alice initialises the project with both members + sets a secret");
    alice_client
        .init_project("billing", &[alice_pub.clone(), bob_pub.clone()])
        .unwrap();
    alice_client
        .set("billing", "DB_PASSWORD", "s3cr3t-over-http")
        .unwrap();

    println!("🧔 Bob pulls from the backend and decrypts:");
    let env = bob_client.pull("billing").unwrap();
    for line in env.to_dotenv().lines() {
        println!("     {line}");
    }

    println!("👩 Alice removes Bob and re-encrypts");
    alice_client.remove_member("billing", &bob_pub).unwrap();

    match bob_client.pull("billing") {
        Err(e) => println!("🧔 Bob pulls again → 🔒 {e}"),
        Ok(env) => println!("🧔 Bob still read: {:?} (BUG!)", env.get("DB_PASSWORD")),
    }
    println!(
        "👩 Alice still reads DB_PASSWORD = {:?}",
        alice_client
            .pull("billing")
            .unwrap()
            .get("DB_PASSWORD")
            .unwrap()
    );
}
