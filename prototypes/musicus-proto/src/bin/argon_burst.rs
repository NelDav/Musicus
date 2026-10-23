//! Assumption: password hashing, not file serving, is the expensive part of a
//! burst of logins (e.g. the whole orchestra logging in at the start of a
//! rehearsal) and must be bounded by a semaphore on a small server.

use argon2::{Argon2, PasswordHasher, PasswordVerifier, password_hash::phc::PasswordHash};
use std::time::Instant;

fn main() {
    // OWASP minimum for Argon2id: m = 19 MiB, t = 2, p = 1 (the crate's default).
    let argon = Argon2::default();
    let hash = argon.hash_password(b"correct horse battery staple").unwrap().to_string();
    let parsed = PasswordHash::new(&hash).unwrap();
    println!("hash: {hash}");

    let n = 20;
    let t = Instant::now();
    for _ in 0..n {
        argon.verify_password(b"correct horse battery staple", &parsed).unwrap();
    }
    let per = t.elapsed() / n;
    println!("verify on one core: {per:?} per login, 19 MiB RAM per concurrent verify");
    for logins in [60u32, 120] {
        for cores in [1u32, 4] {
            println!(
                "{logins} logins at once, {cores} core(s): last user waits {:.1?}; unbounded parallelism would need {} MiB",
                per * logins / cores,
                logins * 19
            );
        }
    }
}
