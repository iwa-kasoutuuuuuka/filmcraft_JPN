//! Frame requests from a small rayon global pool must not deadlock (CI runners have 4 cores).
//!
//! An export renders a batch of frames on rayon's global pool. A worker that needs a decoded frame
//! hands the request to a thread of its own and waits for it. If that thread's decoder then runs
//! its parallel work on the global pool, every worker is waiting and nobody runs it: a deadlock that
//! only shows with fewer threads than frames in a batch. This binary pins the global pool to two
//! threads, so it is its own test binary (the global pool can be built once per process).

use std::sync::Arc;

use filmcraft_codecs::gop::{GopCache, VideoSamples};
use filmcraft_codecs::{DecodedFrame, VideoDecoder};
use filmcraft_frame::VideoFrame;
use rayon::prelude::*;

/// An intra-only stream of `n` one-byte samples; sample `i` decodes to a 1×1 picture of value `i`.
struct Samples {
    n: usize,
}

/// Decodes with nested parallel work on whatever pool it is called from, as ProRes / DNx do.
struct Dec;

impl VideoDecoder for Dec {
    fn decode(&mut self, sample: &[u8], pts: i64) -> filmcraft_codecs::Result<Vec<DecodedFrame>> {
        let sum: u64 = (0..64u64).into_par_iter().map(|a| (0..64u64).into_par_iter().map(|b| (a * b) % 7).sum::<u64>()).sum();
        std::hint::black_box(sum);
        let v = sample.first().copied().unwrap_or(0);
        Ok(vec![DecodedFrame { pts, frame: VideoFrame::rgba8(1, 1, vec![v, 0, 0, 255]), draft: false }])
    }
    fn flush(&mut self) -> Vec<DecodedFrame> {
        Vec::new()
    }
    fn reset(&mut self) {}
    fn name(&self) -> &str {
        "test"
    }
    fn intra_only(&self) -> bool {
        true
    }
}

impl VideoSamples for Samples {
    fn count(&self) -> usize {
        self.n
    }
    fn pts(&self, i: usize) -> i64 {
        i as i64 * 1000
    }
    fn sync_before(&self, i: usize) -> usize {
        i
    }
    fn sample_at(&self, t: i64) -> Option<usize> {
        let i = usize::try_from(t / 1000).ok()?;
        (i < self.n).then_some(i)
    }
    fn read(&self, i: usize) -> filmcraft_codecs::Result<Vec<u8>> {
        Ok(vec![i as u8])
    }
    fn make_decoder(&self) -> filmcraft_codecs::Result<Box<dyn VideoDecoder>> {
        Ok(Box::new(Dec))
    }
}

fn value_of(f: &VideoFrame) -> u8 {
    match &f.data {
        filmcraft_frame::PixelData::Rgba8(px) => px.first().copied().unwrap_or(255),
        _ => 255,
    }
}

#[test]
fn intra_frames_from_a_two_thread_global_pool_do_not_deadlock() {
    rayon::ThreadPoolBuilder::new().num_threads(2).build_global().expect("first use of the global pool");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let s = Samples { n: 64 };
        let c = Arc::new(GopCache::new(None));
        for round in 0..4usize {
            (0..64usize).into_par_iter().for_each(|i| {
                let i = (i * 37 + round * 11) % 64;
                let f = c.frame(&s, i as i64 * 1000).expect("frame");
                assert_eq!(value_of(&f), i as u8);
            });
        }
        let _ = tx.send(());
    });
    assert!(rx.recv_timeout(std::time::Duration::from_secs(60)).is_ok(), "frame requests deadlocked on a two-thread global pool");
}
