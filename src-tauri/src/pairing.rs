use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::auth;

const PIN_TTL: Duration = Duration::from_secs(60);

struct Challenge {
    code: String,
    expires_at: Instant,
}

pub struct PairingStore {
    inner: Mutex<Option<Challenge>>,
}

impl Default for PairingStore {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }
}

impl PairingStore {
    pub fn issue(&self) -> String {
        let code = generate_pin();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(Challenge {
            code: code.clone(),
            expires_at: Instant::now() + PIN_TTL,
        });
        code
    }

    pub fn verify_and_consume(&self, code: &str) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(challenge) = guard.as_ref() else {
            return false;
        };
        if Instant::now() > challenge.expires_at {
            *guard = None;
            return false;
        }
        let ok = auth::tokens_match(&challenge.code, code.trim());
        if ok {
            *guard = None;
        }
        ok
    }

    pub fn active(&self) -> Option<(String, u64)> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let challenge = guard.as_ref()?;
        let remaining = challenge
            .expires_at
            .saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        Some((challenge.code.clone(), remaining.as_secs().max(1)))
    }
}

fn generate_pin() -> String {
    let mut bytes = [0u8; 4];
    let _ = getrandom::getrandom(&mut bytes);
    let n = u32::from_le_bytes(bytes) % 1_000_000;
    format!("{n:06}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_is_six_digits() {
        let store = PairingStore::default();
        let code = store.issue();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn wrong_pin_rejected() {
        let store = PairingStore::default();
        let _ = store.issue();
        assert!(!store.verify_and_consume("000000"));
        assert!(store.active().is_some());
    }

    #[test]
    fn correct_pin_consumed_once() {
        let store = PairingStore::default();
        let code = store.issue();
        assert!(store.verify_and_consume(&code));
        assert!(!store.verify_and_consume(&code));
        assert!(store.active().is_none());
    }
}
