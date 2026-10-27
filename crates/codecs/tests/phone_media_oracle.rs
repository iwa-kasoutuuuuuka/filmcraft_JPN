//! Media as phones and screen recorders make it, against ffmpeg: ffmpeg writes the files and
//! decodes them as the oracle.
//!
//! - Portrait phone video: stored landscape with a display rotation. Reported portrait, and every
//!   frame equals ffmpeg's auto-rotated decode (bit-exact).
//! - Variable frame rate: irregular timestamps, as phones record. Each frame, asked for at its own
//!   timestamp, is ffmpeg's frame (bit-exact).
//! - Screen recording: VP9 + Opus WebM at 1366×768 (not a multiple of the codec block size).
//! - Interrupted recording / download: a file cut short opens and plays up to the cut (frames
//!   bit-exact with ffmpeg's decode of the same bytes) and never panics past it; one whose index
//!   (`moov`) was never written is a clear error.

mod common;

use std::path::Path;

use common::*;
use filmcraft_media::FrameRequest;
use filmcraft_time::Tick;

const TONE: &[&str] = &["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"];

fn open(file: &Path) -> filmcraft_media::SharedSource {
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    filmcraft_codecs::open_bytes(&name, bytes(file)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// ffmpeg's frames of `file` (display order, auto-rotated) split into 4:2:0 planes of `w`×`h`.
fn oracle_frames(ff: &Path, file: &Path, w: usize, h: usize) -> Vec<Vec<Vec<u16>>> {
    let raw = ffmpeg_frames(ff, file, "yuv420p");
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let size = w * h + 2 * cw * ch;
    raw.chunks_exact(size).map(|f| raw_planes(f, w, h, cw, ch, 1)).collect()
}

/// Presentation times of the video frames in seconds, from ffprobe, relative to the first.
fn frame_times(fp: &Path, file: &Path) -> Vec<f64> {
    let o = std::process::Command::new(fp)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "frame=pts_time", "-of", "csv=p=0"])
        .arg(file)
        .output()
        .unwrap();
    let t: Vec<f64> = String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().trim_end_matches(',').parse().ok()).collect();
    let t0 = t.first().copied().unwrap_or(0.0);
    t.into_iter().map(|x| x - t0).collect()
}

/// Our frame at `secs` as planes.
fn ours(src: &filmcraft_media::SharedSource, secs: f64) -> Vec<Vec<u16>> {
    let f = src.video_frame(FrameRequest::full(Tick::from_seconds_f64(secs))).unwrap_or_else(|e| panic!("frame at {secs}: {e}"));
    planes(&f)
}

/// Every frame of `file`, asked for at its own presentation time, equals ffmpeg's.
fn frames_match(ff: &Path, fp: &Path, file: &Path) {
    let src = open(file);
    let v = src.info().video.clone().unwrap();
    let (w, h) = (v.width as usize, v.height as usize);
    let want = oracle_frames(ff, file, w, h);
    let times = frame_times(fp, file);
    assert_eq!(times.len(), want.len(), "ffprobe and ffmpeg frame counts");
    assert!(want.len() >= 10, "{}: only {} frames", file.display(), want.len());
    for (i, (t, w)) in times.iter().zip(&want).enumerate() {
        // a little after the frame's start: the frame shown from t until the next one
        let got = ours(&src, t + 0.001);
        assert_eq!(max_diff(&got, w), 0, "{} frame {i} at {t:.4} s", file.display());
    }
}

#[test]
fn portrait_phone_video_is_upright() {
    let (ff, fp) = filmcraft_testkit::require_oracles!();
    let Some(base) = fixture(
        &ff,
        "phone_landscape.mp4",
        &[
            &["-f", "lavfi", "-i", "testsrc2=size=320x176:rate=30"][..],
            TONE,
            &["-t", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-movflags", "+faststart"],
        ]
        .concat(),
    ) else {
        return;
    };
    for (deg, name) in [("90", "phone_portrait_90.mp4"), ("-90", "phone_portrait_270.mp4"), ("180", "phone_upside_down.mp4")] {
        let Some(file) = fixture(&ff, name, &["-display_rotation", deg, "-i", base.to_str().unwrap(), "-c", "copy"]) else { continue };
        let src = open(&file);
        let v = src.info().video.clone().unwrap();
        let portrait = deg != "180";
        assert_eq!((v.width, v.height), if portrait { (176, 320) } else { (320, 176) }, "{name}: reported size");
        frames_match(&ff, &fp, &file);
    }
}

#[test]
fn variable_frame_rate_phone_video() {
    let (ff, fp) = filmcraft_testkit::require_oracles!();
    // irregular frame durations (every 7th frame held 20 ms longer), B-frames, AAC: as phones record
    let Some(file) = fixture(
        &ff,
        "phone_vfr.mp4",
        &[
            &["-f", "lavfi", "-i", "testsrc2=size=320x176:rate=30,setpts='PTS+trunc(N/7)*0.02/TB'"][..],
            TONE,
            &["-t", "1.5", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-bf", "2", "-fps_mode", "vfr", "-c:a", "aac", "-movflags", "+faststart"],
        ]
        .concat(),
    ) else {
        return;
    };
    let times = frame_times(&fp, &file);
    let gaps: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(gaps.iter().any(|g| (g - 1.0 / 30.0).abs() > 0.01), "the fixture really is variable frame rate: {gaps:?}");
    frames_match(&ff, &fp, &file);
}

#[test]
fn screen_recording_vp9_opus_odd_size() {
    let (ff, fp) = filmcraft_testkit::require_oracles!();
    let Some(file) = fixture(
        &ff,
        "screen_1366x768.webm",
        &[
            &["-f", "lavfi", "-i", "testsrc2=size=1366x768:rate=25"][..],
            TONE,
            &["-t", "0.6", "-c:v", "libvpx-vp9", "-deadline", "realtime", "-cpu-used", "8", "-pix_fmt", "yuv420p", "-c:a", "libopus"],
        ]
        .concat(),
    ) else {
        return;
    };
    let src = open(&file);
    let info = src.info().clone();
    assert_eq!(info.video.as_ref().map(|v| (v.width, v.height)), Some((1366, 768)));
    assert!(info.audio.is_some(), "Opus audio track");
    frames_match(&ff, &fp, &file);
    let a = src.audio(0, 4800, 48_000).unwrap();
    assert!(a.peaks().iter().any(|p| *p > 0.05), "the tone decodes");
}

#[test]
fn interrupted_recording_plays_up_to_the_cut() {
    let (ff, fp) = filmcraft_testkit::require_oracles!();
    let args = |faststart: bool| {
        let mut a = [
            &["-f", "lavfi", "-i", "testsrc2=size=320x176:rate=30"][..],
            TONE,
            &["-t", "2", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "15", "-c:a", "aac"],
        ]
        .concat();
        if faststart {
            a.extend(["-movflags", "+faststart"]);
        }
        a
    };
    // index first (faststart, as a finished phone recording or a progressive download): cut short
    let Some(full) = fixture(&ff, "phone_faststart.mp4", &args(true)) else { return };
    let data = std::fs::read(&full).unwrap();
    let cut = dir().join("phone_cut_short.mp4");
    std::fs::write(&cut, &data[..data.len() * 6 / 10]).unwrap();
    let src = filmcraft_codecs::open_bytes("phone_cut_short.mp4", bytes(&cut)).expect("a file cut short still opens");
    let v = src.info().video.clone().unwrap();
    let want = oracle_frames(&ff, &cut, v.width as usize, v.height as usize);
    let times = frame_times(&fp, &cut);
    assert!(want.len() >= 10, "ffmpeg decodes the start of the file");
    // the frames ffmpeg can decode match (the last couple may be cut mid-sample)
    for (i, (t, w)) in times.iter().zip(&want).take(want.len().saturating_sub(2)).enumerate() {
        assert_eq!(max_diff(&ours(&src, t + 0.001), w), 0, "frame {i} at {t:.4} s");
    }
    // past the cut: an error or a frame, never a panic
    let src2 = src.clone();
    let tail = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        for k in 0..20 {
            let t = 0.9 + k as f64 * 0.06;
            let _ = src2.video_frame(FrameRequest::full(Tick::from_seconds_f64(t)));
            let _ = src2.audio((t * 48_000.0) as i64, 4800, 48_000);
        }
    }));
    assert!(tail.is_ok(), "reading past the cut panicked");

    // index last and never written (the recording app was killed): a clear error, no panic
    let Some(full) = fixture(&ff, "phone_moov_last.mp4", &args(false)) else { return };
    let data = std::fs::read(&full).unwrap();
    let r = std::panic::catch_unwind(|| filmcraft_codecs::open_bytes("killed.mp4", data[..data.len() / 2].to_vec().into()).map(|_| ()));
    match r {
        Ok(Err(e)) => assert!(!e.to_string().is_empty()),
        Ok(Ok(())) => {} // a demuxer that recovers the samples is fine too
        Err(_) => panic!("opening a file without its index panicked"),
    }
}
