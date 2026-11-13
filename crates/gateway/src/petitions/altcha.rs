//! # ALTCHA proof-of-work: the "are you a person?" check for public signing.
//!
//! Self-hosted. No third party sees the visitor. The format follows the ALTCHA v1
//! protocol (<https://altcha.org/docs/server-integration>), so the official widget also
//! works with this server:
//!
//! 1. The server picks a random `salt` and a secret `number` in `0..=max_number`.
//! 2. It sends `challenge = hex(SHA-256(salt + number))` and
//!    `signature = hex(HMAC-SHA-256(key, challenge))`. The salt carries `?expires=<unix>`.
//! 3. The browser tries numbers until the hash matches, and sends back a base64 JSON
//!    payload with `algorithm`, `challenge`, `number`, `salt` and `signature`.
//! 4. The server checks the HMAC, the hash and the expiry. The caller must also record
//!    the signature as spent (see `altcha_spent`), so one solution counts once.
//!
//! The key comes from `ALTCHA_HMAC_KEY`. Without it, public signing is off.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use rand::{Rng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The only algorithm this server issues and accepts.
pub(crate) const ALGORITHM: &str = "SHA-256";
/// Upper bound of the secret number. About one second of work on a phone.
pub(crate) const DEFAULT_MAX_NUMBER: u64 = 100_000;
/// How long a challenge stays valid.
pub(crate) const CHALLENGE_TTL_SECS: i64 = 600;

type HmacSha256 = Hmac<Sha256>;

/// What the browser receives (`GET /petitions/altcha`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Challenge {
    pub algorithm: String,
    pub challenge: String,
    pub maxnumber: u64,
    pub salt: String,
    pub signature: String,
}

/// What the browser sends back, after base64 decoding.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Solution {
    pub algorithm: String,
    pub challenge: String,
    pub number: u64,
    pub salt: String,
    pub signature: String,
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn hash_hex(salt: &str, number: u64) -> String {
    to_hex(&Sha256::digest(format!("{salt}{number}").as_bytes()))
}

fn sign(key: &[u8], challenge: &str) -> Option<String> {
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(challenge.as_bytes());
    Some(to_hex(&mac.finalize().into_bytes()))
}

/// Make a challenge with a given secret number. Split out so tests are deterministic.
pub(crate) fn challenge_with(
    key: &[u8],
    salt_random: &str,
    number: u64,
    max_number: u64,
    now_unix: i64,
) -> Option<Challenge> {
    let salt = format!("{salt_random}?expires={}", now_unix + CHALLENGE_TTL_SECS);
    let challenge = hash_hex(&salt, number);
    let signature = sign(key, &challenge)?;
    Some(Challenge {
        algorithm: ALGORITHM.to_owned(),
        challenge,
        maxnumber: max_number,
        salt,
        signature,
    })
}

/// Make a fresh random challenge.
pub(crate) fn new_challenge(key: &[u8], now_unix: i64) -> Option<Challenge> {
    let mut raw = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let number = rand::rngs::OsRng.gen_range(0..=DEFAULT_MAX_NUMBER);
    challenge_with(key, &to_hex(&raw), number, DEFAULT_MAX_NUMBER, now_unix)
}

/// Read the `expires` value from a salt like `abc?expires=1700000000`.
fn salt_expiry(salt: &str) -> Option<i64> {
    let (_, query) = salt.split_once('?')?;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("expires="))
        .and_then(|v| v.parse().ok())
}

/// Decode the base64 payload the widget sends.
pub(crate) fn decode(payload: &str) -> Option<Solution> {
    if payload.len() > 2048 {
        return None;
    }
    let raw = STANDARD.decode(payload.trim()).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Check a solution. On success, returns the expiry (unix seconds), which the caller
/// stores with the signature in `altcha_spent`.
pub(crate) fn verify(key: &[u8], solution: &Solution, now_unix: i64) -> Option<i64> {
    // The challenge is the key of the replay guard, so it must have ONE spelling: the
    // 64 lower-case hex digits this server issued.
    if solution.algorithm != ALGORITHM
        || solution.challenge.len() != 64
        || from_hex(&solution.challenge).is_none()
    {
        return None;
    }
    let expires = salt_expiry(&solution.salt)?;
    if expires < now_unix {
        return None;
    }
    // Constant-time compare of the HMAC through the Mac API.
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(solution.challenge.as_bytes());
    let given = from_hex(&solution.signature)?;
    mac.verify_slice(&given).ok()?;
    (hash_hex(&solution.salt, solution.number) == solution.challenge).then_some(expires)
}

/// Strict decode: lower-case hex digits only. `u8::from_str_radix` alone also takes
/// upper case and a leading `+`, which would let one solution appear in many spellings.
fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"test-key";
    const NOW: i64 = 1_800_000_000;

    fn solve(c: &Challenge) -> Solution {
        let number = (0..=c.maxnumber)
            .find(|n| hash_hex(&c.salt, *n) == c.challenge)
            .unwrap();
        Solution {
            algorithm: c.algorithm.clone(),
            challenge: c.challenge.clone(),
            number,
            salt: c.salt.clone(),
            signature: c.signature.clone(),
        }
    }

    #[test]
    fn a_solved_challenge_verifies() {
        let c = challenge_with(KEY, "aa", 4242, 10_000, NOW).unwrap();
        let s = solve(&c);
        assert_eq!(s.number, 4242);
        assert_eq!(verify(KEY, &s, NOW), Some(NOW + CHALLENGE_TTL_SECS));
    }

    #[test]
    fn a_random_challenge_round_trips_through_base64() {
        let c = new_challenge(KEY, NOW).unwrap();
        assert!(c.salt.contains("?expires="));
        let payload = STANDARD.encode(serde_json::to_vec(&solve(&c)).unwrap());
        let decoded = decode(&payload).unwrap();
        assert!(verify(KEY, &decoded, NOW).is_some());
    }

    #[test]
    fn a_wrong_number_fails() {
        let c = challenge_with(KEY, "aa", 7, 100, NOW).unwrap();
        let mut s = solve(&c);
        s.number = 8;
        assert!(verify(KEY, &s, NOW).is_none());
    }

    #[test]
    fn another_key_fails() {
        let c = challenge_with(KEY, "aa", 7, 100, NOW).unwrap();
        assert!(verify(b"other", &solve(&c), NOW).is_none());
    }

    #[test]
    fn an_expired_challenge_fails() {
        let c = challenge_with(KEY, "aa", 7, 100, NOW).unwrap();
        assert!(verify(KEY, &solve(&c), NOW + CHALLENGE_TTL_SECS + 1).is_none());
    }

    #[test]
    fn a_changed_salt_fails() {
        let c = challenge_with(KEY, "aa", 7, 100, NOW).unwrap();
        let mut s = solve(&c);
        s.salt = format!("bb?expires={}", NOW + 99_999);
        assert!(verify(KEY, &s, NOW).is_none());
    }

    #[test]
    fn other_algorithms_and_garbage_fail() {
        let c = challenge_with(KEY, "aa", 7, 100, NOW).unwrap();
        let mut s = solve(&c);
        s.algorithm = "SHA-1".to_owned();
        assert!(verify(KEY, &s, NOW).is_none());
        assert!(decode("not base64 !!").is_none());
        assert!(decode(&"A".repeat(5000)).is_none());
        let mut bad_sig = solve(&c);
        bad_sig.signature = "zz".to_owned();
        assert!(verify(KEY, &bad_sig, NOW).is_none());
    }

    #[test]
    fn only_the_canonical_spelling_verifies() {
        let c = challenge_with(KEY, "aa", 7, 100, NOW).unwrap();
        let mut upper = solve(&c);
        upper.signature = upper.signature.to_uppercase();
        assert!(verify(KEY, &upper, NOW).is_none());
        let mut plus = solve(&c);
        plus.signature = format!("+{}", &plus.signature[1..]);
        assert!(verify(KEY, &plus, NOW).is_none());
        let mut chal = solve(&c);
        chal.challenge = chal.challenge.to_uppercase();
        assert!(verify(KEY, &chal, NOW).is_none());
    }
}
