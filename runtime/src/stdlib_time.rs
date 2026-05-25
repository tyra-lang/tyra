//! Time stdlib backing (§17.3.x). v0.1 scalar time operations.
//!
//! Exposes `tyra_time_*` intrinsics consumed by `stdlib/time.tyra`.
//!
//! Scope (v0.1):
//!   now_unix()          -> Int   (seconds since Unix epoch, UTC)
//!   monotonic_millis()  -> Int   (monotonic milliseconds, arbitrary origin)

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// `__time_now_unix() -> Int` — seconds since Unix epoch (UTC).
#[unsafe(no_mangle)]
pub extern "C" fn tyra_time_now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

static MONOTONIC_ORIGIN: OnceLock<Instant> = OnceLock::new();

/// `__time_monotonic_millis() -> Int` — monotonic milliseconds since first call.
#[unsafe(no_mangle)]
pub extern "C" fn tyra_time_monotonic_millis() -> i64 {
    let origin = MONOTONIC_ORIGIN.get_or_init(Instant::now);
    origin.elapsed().as_millis() as i64
}
