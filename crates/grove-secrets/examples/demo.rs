//! End-to-end demo of zero-knowledge team secrets — two members, one shared
//! store, no backend. Run with: `cargo run -p grove-secrets --example demo`

use grove_secrets::{FileStore, Identity, SecretsClient};

fn main() {
    let root = std::env::temp_dir().join("grove-secrets-demo");
    let _ = std::fs::remove_dir_all(&root);

    // Two teammates generate identities locally (private keys never leave).
    let alice = Identity::generate();
    let bob = Identity::generate();
    let alice_pub = alice.public();
    let bob_pub = bob.public();

    println!("👩 Alice pubkey: {}", alice_pub.as_str());
    println!("🧔 Bob   pubkey: {}\n", bob_pub.as_str());

    // Both point at the SAME store (simulates the shared, zero-knowledge backend).
    let alice_client = SecretsClient::new(FileStore::new(&root), alice);
    let bob_client = SecretsClient::new(FileStore::new(&root), bob);

    // Alice creates the project with both members and sets secrets.
    alice_client
        .init_project("billing", &[alice_pub.clone(), bob_pub.clone()])
        .unwrap();
    alice_client
        .set("billing", "APP_KEY", "base64:9f2a…")
        .unwrap();
    alice_client
        .set("billing", "DB_PASSWORD", "s3cr3t-value")
        .unwrap();
    println!("👩 Alice set APP_KEY + DB_PASSWORD, encrypted to [alice, bob]\n");

    // What the server actually stores: ciphertext only.
    let blob = std::fs::read(root.join("billing/env.age")).unwrap();
    let head: String = String::from_utf8_lossy(&blob).chars().take(48).collect();
    println!("🗄  On-disk blob ({} bytes) starts: {:?}", blob.len(), head);
    println!(
        "   contains \"s3cr3t-value\"? {}  ← zero-knowledge\n",
        String::from_utf8_lossy(&blob).contains("s3cr3t-value")
    );

    // Bob clones the repo and pulls — decrypts with his own key.
    let env = bob_client.pull("billing").unwrap();
    println!("🧔 Bob pulled + decrypted his .env:");
    for line in env.to_dotenv().lines() {
        println!("     {line}");
    }
    println!();

    // Bob leaves the team. Alice removes him and re-encrypts.
    alice_client.remove_member("billing", &bob_pub).unwrap();
    println!("👩 Alice removed Bob and re-encrypted to [alice]\n");

    match bob_client.pull("billing") {
        Err(e) => println!("🧔 Bob tries to pull again → 🔒 {e}"),
        Ok(_) => println!("🧔 Bob could still read (BUG!)"),
    }
    let still = alice_client.pull("billing").unwrap();
    println!(
        "👩 Alice can still read DB_PASSWORD = {:?}",
        still.get("DB_PASSWORD").unwrap()
    );

    let _ = std::fs::remove_dir_all(&root);
}
