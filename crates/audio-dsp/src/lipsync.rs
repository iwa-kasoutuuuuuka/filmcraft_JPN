//! Lip sync from the loudness of speech: when a character's mouth is closed, half open or open.
//!
//! No model and no phonemes: the level envelope of the voice, measured against the speaker's own
//! loud passages, decides the mouth, with hysteresis so it does not flutter and a minimum time per
//! shape so it does not flicker. That is what a hand-timed two- or three-picture mouth does, and
//! it is deterministic, so the same recording always gives the same mouth. (Mouth shapes per vowel
//! need the reading of the text aligned to the audio; this is the level below that.)
//!
//! Pure functions on plain slices; times are seconds.

use crate::ducking::{FLOOR_DB, envelope_db};

/// Longest signal analysed, seconds (an hour).
pub const MAX_SECONDS: f64 = 3_600.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// Envelope step, seconds.
    pub hop_s: f32,
    /// Envelope window, seconds.
    pub window_s: f32,
    /// Level (dB below the voice's loud level) from which the mouth is fully open.
    pub open_db: f32,
    /// Level (dB below the loud level) from which the mouth is half open.
    pub half_db: f32,
    /// A shape ends only this many dB below the level that started it.
    pub hysteresis_db: f32,
    /// Shortest time a mouth shape is held, seconds.
    pub min_s: f32,
    /// Absolute level (dBFS) below which the mouth is closed whatever the loud level is.
    pub floor_db: f32,
    /// Three shapes (closed, half, open) instead of two (closed, open).
    pub three_level: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { hop_s: 0.01, window_s: 0.03, open_db: -9.0, half_db: -18.0, hysteresis_db: 3.0, min_s: 0.05, floor_db: -50.0, three_level: false }
    }
}

/// A stretch of one mouth shape: 0 closed, 1 half open (or open, with two shapes), 2 open.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouthSegment {
    pub start: f64,
    pub end: f64,
    pub level: u8,
}

/// The loud level of the voice: the 95th percentile of the envelope above the floor.
fn loud_level(env: &[f32], floor_db: f32) -> Option<f32> {
    let mut v: Vec<f32> = env.iter().copied().filter(|e| *e > floor_db).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let k = ((v.len() as f64 * 0.95) as usize).min(v.len() - 1);
    v.get(k).copied()
}

/// Mouth segments for mono `samples` at `rate` Hz: contiguous, covering the whole signal, each
/// at least `min_s` long (except a signal shorter than that). Empty input, a zero rate or a signal
/// longer than [`MAX_SECONDS`] gives no segments.
pub fn mouth_segments(samples: &[f32], rate: u32, o: &Options) -> Vec<MouthSegment> {
    if samples.is_empty() || rate == 0 || samples.len() as f64 / rate as f64 > MAX_SECONDS {
        return Vec::new();
    }
    // the envelope steps by a whole number of samples: use that step for every time below, or
    // the times drift (22 050 Hz and 0.01 s is 220.5 samples, rounded to 221)
    let hop_n = (o.hop_s.clamp(0.002, 0.1) as f64 * rate as f64).round().max(1.0);
    let hop = hop_n / rate as f64;
    let clean: Vec<f32> = samples.iter().map(|s| if s.is_finite() { *s } else { 0.0 }).collect();
    let env = envelope_db(&[&clean], rate as f32, hop as f32, o.window_s.clamp(0.005, 0.2));
    let total = samples.len() as f64 / rate as f64;
    let Some(loud) = loud_level(&env, o.floor_db) else {
        return vec![MouthSegment { start: 0.0, end: total, level: 0 }];
    };
    let open_at = loud + o.open_db.min(0.0);
    let half_at = (loud + o.half_db.min(0.0)).max(o.floor_db);
    let hyst = o.hysteresis_db.max(0.0);
    let top: u8 = if o.three_level { 2 } else { 1 };

    // per-frame level with hysteresis
    let mut levels: Vec<u8> = Vec::with_capacity(env.len());
    let mut cur: u8 = 0;
    for &e in &env {
        let up = if e >= open_at && o.three_level {
            2
        } else if e >= half_at {
            if o.three_level { 1 } else { top }
        } else {
            0
        };
        if up > cur {
            cur = up;
        } else if up < cur {
            // stay in the shape until the level is `hyst` below the shape's own threshold
            let stay_at = if cur >= 2 { open_at } else { half_at } - hyst;
            if e < stay_at {
                cur = up;
            }
        }
        levels.push(cur);
    }

    // runs of one shape (clamped to the signal: at tiny rates the last frames lie beyond it)
    let mut runs: Vec<MouthSegment> = Vec::new();
    for (k, &l) in levels.iter().enumerate() {
        let (start, end) = ((k as f64 * hop).min(total), ((k + 1) as f64 * hop).min(total));
        if end <= start {
            continue;
        }
        match runs.last_mut() {
            Some(r) if r.level == l => r.end = end,
            _ => runs.push(MouthSegment { start, end, level: l }),
        }
    }
    if let Some(last) = runs.last_mut() {
        last.end = total;
    }
    // one pass: a run shorter than the minimum is absorbed by the shape before it (which then
    // goes on, and joins the next run if that is the same shape); a short first run joins the next
    let min = o.min_s.max(0.0) as f64;
    let mut segs: Vec<MouthSegment> = Vec::with_capacity(runs.len());
    for r in runs {
        match segs.last_mut() {
            Some(last) if last.level == r.level => last.end = r.end,
            Some(last) if r.end - r.start < min - 1e-9 => last.end = r.end,
            _ => segs.push(r),
        }
    }
    if segs.len() > 1 && segs.first().is_some_and(|f| f.end - f.start < min - 1e-9) {
        segs.remove(0);
        if let Some(f) = segs.first_mut() {
            f.start = 0.0;
        }
    }
    debug_assert!(env.iter().all(|e| *e >= FLOOR_DB));
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    /// `(seconds, amplitude)` pieces of a 220 Hz tone (amplitude 0 = silence).
    fn tone(pieces: &[(f64, f32)]) -> Vec<f32> {
        let mut v = Vec::new();
        for (secs, amp) in pieces {
            let n = (secs * RATE as f64) as usize;
            let t0 = v.len();
            v.extend((0..n).map(|k| amp * ((t0 + k) as f32 * 220.0 * std::f32::consts::TAU / RATE as f32).sin()));
        }
        v
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.04
    }

    #[test]
    fn silence_and_nonsense_give_a_closed_mouth_or_nothing() {
        let o = Options::default();
        let s = mouth_segments(&vec![0.0; RATE as usize], RATE, &o);
        assert_eq!(s, vec![MouthSegment { start: 0.0, end: 1.0, level: 0 }]);
        assert!(mouth_segments(&[], RATE, &o).is_empty());
        assert!(mouth_segments(&[0.1; 100], 0, &o).is_empty());
        // hiss far below the floor never opens the mouth
        let quiet = tone(&[(1.0, 0.0005)]);
        assert!(mouth_segments(&quiet, RATE, &o).iter().all(|s| s.level == 0));
        let bad = vec![f32::NAN, f32::INFINITY, -f32::INFINITY, 0.0];
        assert_eq!(mouth_segments(&bad, RATE, &o).len(), 1);
    }

    #[test]
    fn speech_bursts_open_and_close_the_mouth_at_the_right_times() {
        let x = tone(&[(0.3, 0.0), (0.4, 0.5), (0.3, 0.0), (0.5, 0.5), (0.3, 0.0)]);
        let s = mouth_segments(&x, RATE, &Options::default());
        let levels: Vec<u8> = s.iter().map(|g| g.level).collect();
        assert_eq!(levels, vec![0, 1, 0, 1, 0], "{s:?}");
        assert!(near(s[1].start, 0.3) && near(s[1].end, 0.7), "{s:?}");
        assert!(near(s[3].start, 1.0) && near(s[3].end, 1.5), "{s:?}");
        // contiguous and covering
        assert_eq!(s[0].start, 0.0);
        assert!(near(s.last().unwrap().end, 1.8));
        assert!(s.windows(2).all(|w| w[0].end == w[1].start));
    }

    #[test]
    fn a_click_shorter_than_the_minimum_is_absorbed() {
        // a 10 ms click is about 40 ms wide after the 30 ms smoothing window: under the 50 ms minimum
        let x = tone(&[(0.3, 0.0), (0.01, 0.5), (0.3, 0.0), (0.4, 0.5)]);
        let s = mouth_segments(&x, RATE, &Options::default());
        let levels: Vec<u8> = s.iter().map(|g| g.level).collect();
        assert_eq!(levels, vec![0, 1], "{s:?}");
        assert!(near(s[1].start, 0.61), "{s:?}");
    }

    #[test]
    fn three_shapes_follow_the_loudness() {
        let x = tone(&[(0.3, 0.0), (0.4, 0.12), (0.4, 0.5), (0.3, 0.0)]);
        let o = Options { three_level: true, ..Options::default() };
        let levels: Vec<u8> = mouth_segments(&x, RATE, &o).iter().map(|g| g.level).collect();
        assert_eq!(levels, vec![0, 1, 2, 0]);
        // the same recording with two shapes: open for both loud parts
        let levels: Vec<u8> = mouth_segments(&x, RATE, &Options::default()).iter().map(|g| g.level).collect();
        assert_eq!(levels, vec![0, 1, 0]);
    }

    #[test]
    fn a_wobbling_level_does_not_flutter() {
        // loudness swinging ±2 dB around the threshold of the half-open shape
        let mut pieces = vec![(0.2, 0.0)];
        for k in 0..20 {
            pieces.push((0.05, if k % 2 == 0 { 0.5 } else { 0.4 }));
        }
        pieces.push((0.2, 0.0));
        let s = mouth_segments(&tone(&pieces), RATE, &Options::default());
        assert_eq!(s.iter().map(|g| g.level).collect::<Vec<_>>(), vec![0, 1, 0], "{s:?}");
    }

    #[test]
    fn times_do_not_drift_when_the_hop_is_not_a_whole_number_of_samples() {
        // 22 050 Hz and a 0.01 s hop is 220.5 samples; a burst at 40-42 s of 50 s
        let rate = 22_050u32;
        let n = |secs: f64| (secs * rate as f64) as usize;
        let mut x = vec![0.0f32; n(40.0)];
        x.extend((0..n(2.0)).map(|k| 0.5 * (k as f32 * 220.0 * std::f32::consts::TAU / rate as f32).sin()));
        x.extend(vec![0.0f32; n(8.0)]);
        let s = mouth_segments(&x, rate, &Options::default());
        assert_eq!(s.iter().map(|g| g.level).collect::<Vec<_>>(), vec![0, 1, 0], "{s:?}");
        assert!((s[1].start - 40.0).abs() < 0.03 && (s[1].end - 42.0).abs() < 0.03, "{s:?}");
        assert!((s[2].end - 50.0).abs() < 1e-6);
        // a rate so low that frames lie beyond the end still gives well-formed segments
        let s =
            mouth_segments(&[0.5, -0.5, 0.5, -0.5, 0.5, -0.5, 0.5, -0.5, 0.5, -0.5, 0.5, -0.5, 0.5, -0.5], 14, &Options { hop_s: 0.1, ..Options::default() });
        assert!(s.iter().all(|g| g.end > g.start) && s.windows(2).all(|w| w[0].end == w[1].start), "{s:?}");
    }

    #[test]
    fn many_short_bursts_are_merged_in_linear_time() {
        // ten minutes of 40 ms on / 40 ms off at 8 kHz: every burst is below the minimum
        let rate = 8_000u32;
        let burst = (0.04 * rate as f64) as usize;
        let mut x = Vec::new();
        for k in 0..15_000 {
            x.extend((0..burst).map(|i| if k % 2 == 0 { 0.5 * ((i as f32) * 0.2).sin() } else { 0.0 }));
        }
        let t0 = std::time::Instant::now();
        let s = mouth_segments(&x, rate, &Options::default());
        assert!(t0.elapsed() < std::time::Duration::from_secs(10), "{:?}", t0.elapsed());
        assert!(s.windows(2).all(|w| w[0].end == w[1].start && w[0].level != w[1].level));
        assert!(s.iter().all(|g| g.end - g.start >= 0.05 - 1e-6 || s.len() == 1), "{}", s.len());
    }

    #[test]
    fn deterministic_and_bounded() {
        let x = tone(&[(0.3, 0.0), (0.4, 0.5)]);
        assert_eq!(mouth_segments(&x, RATE, &Options::default()), mouth_segments(&x, RATE, &Options::default()));
        let long = vec![0.0f32; (MAX_SECONDS as usize + 1) * 100];
        assert!(mouth_segments(&long, 100, &Options::default()).is_empty());
    }
}
