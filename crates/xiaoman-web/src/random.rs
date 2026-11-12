//! Unguessable identifiers for sessions and for approval / calendar entries.

use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;

/// Fills `dest` from the system random source, falling back to a time-seeded mix when the
/// platform refuses (the fallback keeps the server running rather than panicking).
pub fn fill(dest: &mut [u8]) {
    if getrandom::fill(dest).is_ok() {
        return;
    }
    static COUNTER: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
    let mut state = xiaoman_api::now_millis() as u64
        ^ COUNTER.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::SeqCst);
    for byte in dest.iter_mut() {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        *byte = (state.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as u8;
    }
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    fill(&mut bytes);
    bytes
}

/// A 16-byte session token in URL-safe base64 without padding: 22 characters.
pub fn session_id() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes::<16>())
}

/// A random RFC 4122 version 4 UUID, as the Kotlin server used for approval ids and `UID:`.
pub fn uuid() -> String {
    let mut bytes = random_bytes::<16>();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_are_url_safe_and_distinct() {
        let first = session_id();
        let second = session_id();
        assert_eq!(first.len(), 22);
        assert!(
            first
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_ne!(first, second);
    }

    #[test]
    fn uuids_have_the_version_4_shape() {
        let value = uuid();
        assert_eq!(value.len(), 36);
        assert_eq!(value.as_bytes()[14], b'4');
        assert!(matches!(value.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
    }
}
