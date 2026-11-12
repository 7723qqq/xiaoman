//! Task ids double as trace run ids, mirroring `app/src/main/kotlin/dev/xiaoman/app/RunIds.kt`.
//!
//! The debug bridge [`RunIds::prime`]s the next id so `xiaoman run --run-id r1` finds its trace
//! under `traces/r1/`; otherwise ids look like `app-20261031-120000-a1b2`, stamped in local time
//! the way Kotlin's `SimpleDateFormat("yyyyMMdd-HHmmss")` does.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{FixedOffset, Local, TimeZone};

const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const SUFFIX_LEN: usize = 4;
/// `RunIds.VALID` allows 1 to 80 characters: ids become folder names under `traces/`.
pub const MAX_ID_LEN: usize = 80;

type Clock = Box<dyn Fn() -> i64 + Send + Sync>;
type Random = Box<dyn FnMut(usize) -> usize + Send>;

/// Where the `yyyyMMdd-HHmmss` stamp comes from: the machine's zone, or a fixed offset in tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Local,
    Fixed(i32),
}

pub struct RunIds {
    clock: Clock,
    random: Random,
    zone: Zone,
    primed: Option<String>,
}

impl RunIds {
    /// The production generator: wall clock, a small built-in PRNG and the local zone.
    pub fn new() -> Self {
        let mut state = seed() | 1;
        Self {
            clock: Box::new(xiaoman_api::now_millis),
            random: Box::new(move |bound| random_index(&mut state, bound)),
            zone: Zone::Local,
            primed: None,
        }
    }

    /// Kotlin's `RunIds(clock, random)` plus the zone offset in minutes east of UTC, so tests can
    /// pin every input.
    pub fn with(
        clock: impl Fn() -> i64 + Send + Sync + 'static,
        random: impl FnMut(usize) -> usize + Send + 'static,
        offset_minutes: i32,
    ) -> Self {
        Self {
            clock: Box::new(clock),
            random: Box::new(random),
            zone: Zone::Fixed(offset_minutes),
            primed: None,
        }
    }

    /// The next [`RunIds::next_id`] call returns `id`.
    pub fn prime(&mut self, id: impl Into<String>) {
        self.primed = Some(id.into());
    }

    /// `RunIds.next` in Kotlin; named for what it returns so it cannot be mistaken for
    /// `Iterator::next`.
    pub fn next_id(&mut self) -> String {
        match self.primed.take() {
            Some(id) => id,
            None => self.generate(),
        }
    }

    pub fn generate(&mut self) -> String {
        let stamp = stamp((self.clock)(), self.zone);
        let mut suffix = String::with_capacity(SUFFIX_LEN);
        for _ in 0..SUFFIX_LEN {
            let index = (self.random)(ALPHABET.len()) % ALPHABET.len();
            suffix.push(ALPHABET[index] as char);
        }
        format!("app-{stamp}-{suffix}")
    }

    /// Same rule as the CLI: ids are folder names under `traces/`.
    pub fn is_valid(id: &str) -> bool {
        let length = id.chars().count();
        (1..=MAX_ID_LEN).contains(&length)
            && id
                .chars()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, '.' | '_' | '-'))
            && id != "."
            && id != ".."
    }
}

impl Default for RunIds {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for RunIds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunIds")
            .field("primed", &self.primed)
            .field("zone", &self.zone)
            .finish_non_exhaustive()
    }
}

/// `yyyyMMdd-HHmmss` in the zone `offset_minutes` east of UTC.
pub fn format_stamp(millis: i64, offset_minutes: i32) -> String {
    stamp(millis, Zone::Fixed(offset_minutes))
}

fn stamp(millis: i64, zone: Zone) -> String {
    let formatted = match zone {
        Zone::Local => Local
            .timestamp_millis_opt(millis)
            .single()
            .map(|time| time.format("%Y%m%d-%H%M%S").to_string()),
        Zone::Fixed(minutes) => FixedOffset::east_opt(minutes * 60)
            .and_then(|offset| offset.timestamp_millis_opt(millis).single())
            .map(|time| time.format("%Y%m%d-%H%M%S").to_string()),
    };
    // Only an unrepresentable instant (outside chrono's year range) falls back to the raw value.
    formatted.unwrap_or_else(|| millis.to_string())
}

/// `Random.nextInt(ALPHABET.length)` per suffix position; xorshift64 keeps this dependency-free.
fn random_index(state: &mut u64, bound: usize) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state % bound as u64) as usize
}

fn seed() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or_default();
    let id = u64::from(std::process::id());
    nanos
        ^ (id << 32)
        ^ COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
}
