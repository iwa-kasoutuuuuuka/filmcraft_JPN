//! Hardware (OS) video encoders: the Settings ▸ Export ▸ Hardware encoding switch, and the
//! counters `perf.stats` reports.
//!
//! `filmcraft-platform` registers the OS encoders ([`crate::register_encoder`]) in front of ours.
//! Their factories decline (fall through to our encoders) while the switch is off, for settings
//! they do not take, and when the OS has no hardware encoder; an export whose hardware encoder
//! fails on its first frames carries on with our encoder ([`crate::VideoEncoder::is_hardware`]).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(true);
static SESSIONS: AtomicU64 = AtomicU64::new(0);
static FALLBACKS: AtomicU64 = AtomicU64::new(0);

/// Settings ▸ Export ▸ Hardware encoding (on by default). Exports already running keep their
/// encoder.
pub fn set_hardware_encoding(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether new exports may use hardware encoders.
pub fn hardware_encoding() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Hardware encoding counters (they only grow).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HwEncodeStats {
    /// Hardware encoders created.
    pub sessions: u64,
    /// Exports whose hardware encoder failed on the first frames and that finished with ours.
    pub fallbacks: u64,
}

/// The counters so far.
pub fn hw_encode_stats() -> HwEncodeStats {
    HwEncodeStats { sessions: SESSIONS.load(Ordering::Relaxed), fallbacks: FALLBACKS.load(Ordering::Relaxed) }
}

/// A hardware encoder was created.
pub fn note_hw_encode_session() {
    SESSIONS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_hw_encode_fallback() {
    FALLBACKS.fetch_add(1, Ordering::Relaxed);
}
