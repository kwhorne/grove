//! Verify a license key against the baked-in public key.
//! Usage: `cargo run -p grove-license --example verify -- "GROVE-…"`

fn main() {
    let key = std::env::args().nth(1).expect("pass a license key");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    match grove_license::verify(&key, now) {
        Ok(c) => {
            println!("✓ VALID license");
            println!("  plan   : {}", c.plan);
            println!("  seats  : {}", c.seats);
            println!("  email  : {}", c.email);
            println!("  expires: {} (unix)", c.exp);
            println!("  teams? : {}", c.is_teams());
        }
        Err(e) => {
            println!("✗ INVALID: {e}");
            std::process::exit(1);
        }
    }
}
