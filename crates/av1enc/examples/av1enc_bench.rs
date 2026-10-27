//! Development tool: encode synthetic moving content and report the speed.
//!
//! `cargo run --release -p filmcraft-av1enc --example av1enc_bench -- [W H FRAMES KBPS SPEED]`
//! (default 1920 1080 48 8000 10).

use filmcraft_av1enc::{Encoder, EncoderConfig, RateControl, YuvFrame};
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let arg = |i: usize, d: usize| a.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let (w, h, n, kbps, speed) = (arg(1, 1920), arg(2, 1080), arg(3, 48), arg(4, 8000), arg(5, 10));
    let mut cfg = EncoderConfig::new(w as u32, h as u32, 24, 1);
    cfg.rate = RateControl::Bitrate { kbps: kbps as u32 };
    cfg.speed = speed.min(10) as u8;
    let mut enc = match Encoder::new(&cfg) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let t0 = Instant::now();
    let mut bytes = 0usize;
    let mut packets = 0usize;
    for i in 0..n {
        let y: Vec<u8> = (0..w * h).map(|k| ((k % w + k / w + i * 4) % 220 + 16) as u8).collect();
        let u = vec![(110 + i % 20) as u8; cw * ch];
        let v = vec![140u8; cw * ch];
        match enc.encode(&YuvFrame { y: &y, u: &u, v: &v, y_stride: w, uv_stride: cw }) {
            Ok(ps) => {
                packets += ps.len();
                bytes += ps.iter().map(|p| p.data.len()).sum::<usize>();
            }
            Err(e) => {
                eprintln!("{e}");
                return;
            }
        }
    }
    if let Ok(ps) = enc.flush() {
        packets += ps.len();
        bytes += ps.iter().map(|p| p.data.len()).sum::<usize>();
    }
    let s = t0.elapsed().as_secs_f64();
    println!("{w}x{h} speed {speed}: {packets} frames in {s:.2}s = {:.2} fps, {:.0} kbps", packets as f64 / s, bytes as f64 * 8.0 / 1000.0 / (n as f64 / 24.0));
}
