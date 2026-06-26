//! RSA keypair generation for ActivityPub Actor `publicKey` documents (ADR-0010 W2).
//!
//! Pure CPU + RNG — no database, no I/O. The caller (the gateway composition root) persists the
//! generated pair into `citizen_actor_key` lazily, only when a citizen flips `is_public = true`.
//! The returned strings are PKCS#8 (private) and SubjectPublicKeyInfo (public) PEM, which is
//! what the federation Actor document and HTTP-Signature verifiers both expect.

use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use rsa::{RsaPrivateKey, RsaPublicKey};

/// RSA modulus size for ActivityPub Actor keys. 2048 is the Mastodon / Pleroma de-facto floor;
/// anything smaller is rejected by mainstream verifiers, larger is unnecessary at this layer.
pub const ACTOR_KEY_BITS: usize = 2048;

/// A freshly-generated keypair, ready to persist. The private PEM is a credential — store it in
/// a column the federation surface never reads from on the request hot path.
#[derive(Debug, Clone)]
pub struct GeneratedKeypair {
    /// PKCS#8 PEM of the private key. NEVER leave the gateway boundary.
    pub private_pem: String,
    /// SubjectPublicKeyInfo PEM of the public key. Embedded in the Actor document.
    pub public_pem: String,
}

/// Generate a fresh RSA-2048 keypair. CPU-bound (≈100ms on modern x86); the caller should
/// invoke from a blocking task if the request hot path matters.
///
/// # Errors
/// Returns the underlying `rsa::Error` if key generation fails (effectively never; the only
/// realistic source is an exhausted OS RNG, which means the process has bigger problems).
pub fn generate_actor_keypair() -> Result<GeneratedKeypair, rsa::Error> {
    let mut rng = rand::rngs::OsRng;
    let private = RsaPrivateKey::new(&mut rng, ACTOR_KEY_BITS)?;
    let public = RsaPublicKey::from(&private);
    let private_pem = private
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|_| rsa::Error::Internal)?
        .to_string();
    let public_pem = public
        .to_public_key_pem(LineEnding::LF)
        .map_err(|_| rsa::Error::Internal)?;
    Ok(GeneratedKeypair {
        private_pem,
        public_pem,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_is_well_formed_pem() {
        let kp = generate_actor_keypair().expect("rsa gen");
        assert!(kp.private_pem.contains("BEGIN PRIVATE KEY"));
        assert!(kp.private_pem.contains("END PRIVATE KEY"));
        assert!(kp.public_pem.contains("BEGIN PUBLIC KEY"));
        assert!(kp.public_pem.contains("END PUBLIC KEY"));
    }

    #[test]
    fn pair_is_distinct_each_call() {
        let a = generate_actor_keypair().unwrap();
        let b = generate_actor_keypair().unwrap();
        assert_ne!(a.private_pem, b.private_pem);
        assert_ne!(a.public_pem, b.public_pem);
    }
}
