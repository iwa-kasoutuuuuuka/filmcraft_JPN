//! AVI files made by ffmpeg (skipped without it; `FILMCRAFT_REQUIRE_ORACLES=1` makes that a
//! failure), checked against ffmpeg's decode of the same file: bit-exact where the decoder is
//! (H.264, uncompressed video, PCM), within decoder tolerance where it differs (JPEG, MP3 / MP2 /
//! AC-3 / AAC). Plus hand-made hostile files that must fail cleanly.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use filmcraft_media::{FrameRequest, SharedSource};
use filmcraft_time::Tick;

const PIC: [&str; 4] = ["-f", "lavfi", "-i", "testsrc2=s=160x120:r=25:d=2"];
const TONE: [&str; 4] = ["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:d=2"];

/// `target/fixtures/codecs-avi/<name>`, made by ffmpeg with `args` when missing (`None` without
/// ffmpeg or when it fails, e.g. an encoder it was built without).
fn fixture_path(name: &str, args: &[&str]) -> Option<PathBuf> {
    let ffmpeg = filmcraft_testkit::ffmpeg_or_skip("codecs avi")?;
    let out = filmcraft_testkit::fixtures_dir("codecs-avi").join(name);
    let made = filmcraft_testkit::fixtures::generate(&out, |tmp: &Path| {
        Command::new(&ffmpeg).args(["-v", "error", "-y"]).args(args).arg(tmp).status().is_ok_and(|s| s.success())
    });
    if made.is_none() {
        eprintln!("SKIPPED (codecs avi): ffmpeg could not make {name} (encoder missing?)");
    }
    made
}

/// ffmpeg's decode of `src` as raw frames in `pix_fmt` (the oracle).
fn reference_frames(src: &Path, pix_fmt: &str) -> Vec<u8> {
    let ffmpeg = filmcraft_testkit::ffmpeg().unwrap();
    let out = Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(src)
        .args(["-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", pix_fmt, "-"])
        .output()
        .unwrap();
    assert!(out.status.success(), "ffmpeg decode of {}", src.display());
    out.stdout
}

/// ffmpeg's decode of the first audio stream as mono f32 at its own rate.
fn reference_audio(path: &Path) -> Vec<f32> {
    let ffmpeg = filmcraft_testkit::ffmpeg().unwrap();
    let out = Command::new(ffmpeg).args(["-v", "error", "-i"]).arg(path).args(["-map", "0:a:0", "-f", "f32le", "-c:a", "pcm_f32le", "-"]).output().unwrap();
    out.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
}

/// Planes of a decoded YUV frame (ffmpeg rawvideo layout).
fn yuv_bytes(f: &filmcraft_frame::VideoFrame) -> Vec<u8> {
    match &f.data {
        filmcraft_frame::PixelData::Yuv8 { planes, .. } => planes.iter().flat_map(|p| p.iter().copied()).collect(),
        _ => panic!("not an 8-bit YUV frame: {}", f.format_label()),
    }
}

fn avi(name: &str, video: &[&str], audio: &[&str]) -> Option<(PathBuf, SharedSource)> {
    let mut args: Vec<&str> = PIC.to_vec();
    args.extend_from_slice(&TONE);
    args.extend_from_slice(video);
    args.extend_from_slice(audio);
    let path = fixture_path(name, &args)?;
    let bytes: Arc<[u8]> = std::fs::read(&path).ok()?.into();
    Some((path.clone(), crate::open_bytes(name, bytes).unwrap_or_else(|e| panic!("{name}: {e}"))))
}

/// The first `n` samples of channel 0 as FilmCraft decodes them.
fn ours(src: &SharedSource, n: usize) -> Vec<f32> {
    src.audio(0, n, 48_000).unwrap().channels[0].clone()
}

fn check_info(name: &str, src: &SharedSource, vcodec: &str, acodec: &str) {
    let info = src.info();
    assert_eq!(info.container, "AVI", "{name}");
    let v = info.video.as_ref().unwrap();
    assert_eq!((v.width, v.height, v.codec.as_str()), (160, 120, vcodec), "{name}");
    assert_eq!(v.frame_rate.as_f64(), 25.0, "{name}");
    let a = info.audio.as_ref().unwrap();
    assert_eq!((a.sample_rate, a.channels, a.codec.as_str()), (48_000, 1, acodec), "{name}");
    let secs = info.duration.seconds();
    assert!((secs - 2.0).abs() < 0.1, "{name}: duration {secs}");
}

fn rms_and_diff(reference: &[f32], ours: &[f32], range: std::ops::Range<usize>) -> (f32, f32) {
    let n = range.len() as f32;
    let rms = (range.clone().map(|i| reference[i].powi(2)).sum::<f32>() / n).sqrt();
    let diff = (range.map(|i| (reference[i] - ours[i]).powi(2)).sum::<f32>() / n).sqrt();
    (rms, diff)
}

#[test]
fn h264_frames_are_bit_exact_with_random_access_and_mp3_plays() {
    let Some((path, src)) =
        avi("avi_h264_mp3.avi", &["-c:v", "libx264", "-bf", "2", "-g", "12", "-pix_fmt", "yuv420p"], &["-c:a", "libmp3lame", "-b:a", "128k"])
    else {
        return;
    };
    check_info("h264", &src, "H.264", "MP3");
    let reference = reference_frames(&path, "yuv420p");
    let frame = reference.len() / 50;
    let rate = src.info().frame_rate();
    for k in [37, 3, 24, 49, 0, 13, 12, 11, 30, 31, 32] {
        let f = src.video_frame(FrameRequest::full(rate.tick_of(k as i64))).unwrap();
        assert!(yuv_bytes(&f) == reference[k * frame..(k + 1) * frame], "H.264 frame {k} differs from ffmpeg");
    }
    // MP3 (a different decoder): the tone, in time with ffmpeg's decode
    let r = reference_audio(&path);
    let a = ours(&src, 48_000);
    let (rms_ref, rms_diff) = rms_and_diff(&r, &a, 4_800..43_200);
    // ffmpeg's test tone is at 1/8 of full scale
    assert!(rms_ref > 0.05 && rms_diff < rms_ref * 0.05, "MP3: tone {rms_ref}, difference {rms_diff}");
    // random access into the middle matches sequential decoding
    let mid = src.audio(24_000, 4_800, 48_000).unwrap().channels[0].clone();
    let worst = mid.iter().zip(&a[24_000..28_800]).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max);
    assert!(worst < 0.01, "MP3 seek: {worst}");
}

#[test]
fn mjpeg_and_pcm() {
    let Some((path, src)) = avi("avi_mjpeg_pcm.avi", &["-c:v", "mjpeg", "-q:v", "3", "-pix_fmt", "yuvj420p"], &["-c:a", "pcm_s16le"]) else { return };
    check_info("mjpeg", &src, "Motion JPEG", "PCM 16-bit");
    let reference = reference_frames(&path, "rgba");
    let frame = reference.len() / 50;
    let rate = src.info().frame_rate();
    for k in [10, 0, 49] {
        let f = src.video_frame(FrameRequest::full(rate.tick_of(k as i64))).unwrap().to_rgba8();
        let r = &reference[k * frame..(k + 1) * frame];
        let mean = f.iter().zip(r).map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs()).sum::<u32>() as f64 / f.len() as f64;
        // another JPEG decoder and chroma upsampling than ffmpeg's: close, not identical
        assert!(mean < 6.0, "JPEG frame {k}: mean difference {mean}");
        // and not a black or flat picture
        let luma: Vec<u8> = f.as_chunks::<4>().0.iter().map(|p| p[1]).collect();
        let (lo, hi) = (luma.iter().min().unwrap(), luma.iter().max().unwrap());
        assert!(hi - lo > 100, "JPEG frame {k} is flat: {lo}..{hi}");
    }
    // PCM: exactly ffmpeg's samples
    let r = reference_audio(&path);
    assert_eq!(ours(&src, 48_000), r[..48_000].to_vec());
}

#[test]
fn uncompressed_video_is_bit_exact() {
    for (name, pix, codec, reference_fmt) in [
        ("avi_bgr24.avi", "bgr24", "Uncompressed RGB 24-bit", "rgba"),
        ("avi_yuyv.avi", "yuyv422", "Uncompressed YUV 4:2:2", "yuv422p"),
        ("avi_i420.avi", "yuv420p", "Uncompressed YUV 4:2:0", "yuv420p"),
        ("avi_nv12.avi", "nv12", "Uncompressed YUV 4:2:0", "yuv420p"),
    ] {
        let Some((path, src)) = avi(name, &["-c:v", "rawvideo", "-pix_fmt", pix], &["-c:a", "pcm_s16le"]) else { return };
        check_info(name, &src, codec, "PCM 16-bit");
        let reference = reference_frames(&path, reference_fmt);
        let frame = reference.len() / 50;
        let rate = src.info().frame_rate();
        for k in [7, 0, 49] {
            let f = src.video_frame(FrameRequest::full(rate.tick_of(k as i64))).unwrap();
            let got = if reference_fmt == "rgba" { f.to_rgba8() } else { yuv_bytes(&f) };
            assert!(got == reference[k * frame..(k + 1) * frame], "{name}: frame {k} differs from ffmpeg");
        }
    }
}

#[test]
fn mp2_ac3_and_aac_audio() {
    for (name, codec, label) in [("avi_mp2.avi", "mp2", "MPEG Audio"), ("avi_ac3.avi", "ac3", "AC-3"), ("avi_aac.avi", "aac", "AAC")] {
        let Some((path, src)) = avi(name, &["-c:v", "mjpeg"], &["-c:a", codec, "-b:a", "192k"]) else { return };
        assert_eq!(src.info().audio.as_ref().unwrap().codec, label, "{name}");
        let r = reference_audio(&path);
        let a = ours(&src, 48_000);
        let (rms_ref, rms_diff) = rms_and_diff(&r, &a, 4_800..43_200);
        assert!(rms_ref > 0.05 && rms_diff < rms_ref * 0.05, "{name}: tone {rms_ref}, difference {rms_diff}");
    }
}

/// Dropped frames are stored as empty chunks: their slot shows the picture before.
#[test]
fn drop_frames_hold_the_previous_picture() {
    let args = [
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=160x120:r=25:d=2",
        "-vf",
        "select='not(eq(n\\,10))'",
        "-fps_mode",
        "passthrough",
        "-c:v",
        "rawvideo",
        "-pix_fmt",
        "bgr24",
    ];
    let Some(path) = fixture_path("avi_drop.avi", &args) else { return };
    let src = crate::open_bytes("avi_drop.avi", std::fs::read(&path).unwrap().into()).unwrap();
    let rate = src.info().frame_rate();
    let at = |k: i64| src.video_frame(FrameRequest::full(rate.tick_of(k))).unwrap().to_rgba8();
    assert!(at(10) == at(9), "the dropped slot holds frame 9");
    assert!(at(11) != at(9));
}

/// DivX / Xvid has no decoder yet: the file opens, the codec is named, the picture reports it, and
/// the sound plays.
#[test]
fn mpeg4_part2_is_named_and_its_audio_plays() {
    let Some((_, src)) = avi("avi_xvid.avi", &["-c:v", "mpeg4", "-vtag", "XVID"], &["-c:a", "pcm_s16le"]) else { return };
    assert_eq!(src.info().video.as_ref().unwrap().codec, "MPEG-4 Part 2 (DivX / Xvid)");
    let e = src.video_frame(FrameRequest::full(Tick::ZERO)).unwrap_err().to_string();
    assert!(e.contains("MPEG-4 Part 2"), "{e}");
    assert!(ours(&src, 4_800).iter().any(|s| s.abs() > 0.1));
}

/// A Motion JPEG frame without Huffman tables (the AVI1 convention) decodes with the standard ones:
/// exactly as the same frame with them.
#[test]
fn jpeg_without_huffman_tables_uses_the_standard_ones() {
    let Some(path) = fixture_path(
        "default_huffman.jpg",
        &["-f", "lavfi", "-i", "testsrc2=s=160x120", "-frames:v", "1", "-c:v", "mjpeg", "-huffman", "default", "-q:v", "3"],
    ) else {
        return;
    };
    let jpeg = std::fs::read(&path).unwrap();
    // remove every DHT segment
    let mut stripped = jpeg[..2].to_vec();
    let mut at = 2;
    while at + 4 <= jpeg.len() && jpeg[at] == 0xFF && jpeg[at + 1] != 0xDA {
        let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        if jpeg[at + 1] != 0xC4 {
            stripped.extend_from_slice(&jpeg[at..at + 2 + len]);
        }
        at += 2 + len;
    }
    stripped.extend_from_slice(&jpeg[at..]);
    assert!(stripped.len() < jpeg.len() && !stripped.windows(2).any(|w| w == [0xFF, 0xC4]));
    use crate::video::VideoDecoder;
    let mut d = crate::video::MjpegDecoder;
    let a = d.decode(&jpeg, 0).unwrap().remove(0).frame.to_rgba8();
    let b = d.decode(&stripped, 0).unwrap().remove(0).frame.to_rgba8();
    assert!(a == b);
    // the inserted segment is 420 bytes: 4 tables of 12, 12, 162, 162 codes
    let with = crate::video::with_default_huffman(&stripped);
    assert_eq!(with.len(), stripped.len() + 420);
}

/// H.264 with B-frames copied from an MP4 whose time base is 1/600 (ffmpeg keeps it): 600 frame
/// slots a second, a picture in every 24th at 25 fps, empty drop chunks between. The pictures stay
/// at their times in presentation order, and the stream reports the rate they play at.
#[test]
fn copied_h264_keeps_its_timing_with_a_fine_time_base() {
    let mp4 =
        ["-f", "lavfi", "-i", "testsrc2=s=160x120:r=25:d=2", "-c:v", "libx264", "-bf", "2", "-g", "12", "-pix_fmt", "yuv420p", "-video_track_timescale", "600"];
    let Some(src_mp4) = fixture_path("avi_copy_source.mp4", &mp4) else { return };
    let src_path = src_mp4.to_string_lossy().into_owned();
    let Some(path) = fixture_path("avi_copy_600.avi", &["-i", &src_path, "-c:v", "copy", "-bsf:v", "h264_mp4toannexb", "-f", "avi"]) else { return };
    let src = crate::open_bytes("avi_copy_600.avi", std::fs::read(&path).unwrap().into()).unwrap();
    let v = src.info().video.clone().unwrap();
    assert_eq!(v.frame_rate.as_f64(), 25.0, "the rate the pictures play at, not the 600 slots");
    let reference = reference_frames(&path, "yuv420p");
    let frame = reference.len() / 50;
    for k in [37usize, 3, 24, 49, 0, 13, 12, 11, 30] {
        let at = Tick::from_rational(k as i64, 1, 25);
        let f = src.video_frame(FrameRequest::full(at)).unwrap();
        assert!(yuv_bytes(&f) == reference[k * frame..(k + 1) * frame], "picture {k} at {:.2} s differs from ffmpeg", k as f64 / 25.0);
    }
    // between pictures, the one before holds
    let mid = src.video_frame(FrameRequest::full(Tick::from_rational(1, 1, 50))).unwrap();
    assert!(yuv_bytes(&mid) == reference[0..frame]);
}

/// A streaming reader (the Media Browser probe) opens the same file through the reader registry.
#[test]
fn reader_opener_streams_avi() {
    let Some((path, _)) = avi("avi_mjpeg_pcm.avi", &["-c:v", "mjpeg", "-q:v", "3", "-pix_fmt", "yuvj420p"], &["-c:a", "pcm_s16le"]) else { return };
    let bytes: Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let reader: filmcraft_media::SharedReader = Arc::new(filmcraft_media::reader::MemReader(bytes.clone()));
    let head = &bytes[..bytes.len().min(64 << 10)];
    let src = crate::reader_openers().iter().find_map(|o| o("x.avi", head, &reader)).expect("an AVI reader opener").unwrap();
    assert_eq!(src.info().container, "AVI");
    assert!(src.video_frame(FrameRequest::full(Tick::ZERO)).is_ok());
}

fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(data);
    if data.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn list(ltype: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut d = ltype.to_vec();
    d.extend_from_slice(body);
    chunk(b"LIST", &d)
}

/// A tiny hand-made AVI: one BGR24 video stream (`w` x `h`, `frames` chunks of `frame_bytes`) and
/// one PCM stream with the given header fields.
fn tiny(w: i32, h: i32, frames: usize, frame_bytes: usize, rate: u32, scale: u32, sample_rate: u32, channels: u16, block: u16) -> Vec<u8> {
    let mut strh_v = vec![0u8; 56];
    strh_v[..4].copy_from_slice(b"vids");
    strh_v[20..24].copy_from_slice(&scale.to_le_bytes());
    strh_v[24..28].copy_from_slice(&rate.to_le_bytes());
    let mut bih = vec![0u8; 40];
    bih[4..8].copy_from_slice(&w.to_le_bytes());
    bih[8..12].copy_from_slice(&h.to_le_bytes());
    bih[14..16].copy_from_slice(&24u16.to_le_bytes());
    let mut v = chunk(b"strh", &strh_v);
    v.extend(chunk(b"strf", &bih));
    let mut strh_a = vec![0u8; 56];
    strh_a[..4].copy_from_slice(b"auds");
    strh_a[20..24].copy_from_slice(&1u32.to_le_bytes());
    strh_a[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    let mut wfx = vec![0u8; 18];
    wfx[..2].copy_from_slice(&1u16.to_le_bytes());
    wfx[2..4].copy_from_slice(&channels.to_le_bytes());
    wfx[4..8].copy_from_slice(&sample_rate.to_le_bytes());
    wfx[12..14].copy_from_slice(&block.to_le_bytes());
    wfx[14..16].copy_from_slice(&16u16.to_le_bytes());
    let mut a = chunk(b"strh", &strh_a);
    a.extend(chunk(b"strf", &wfx));
    let mut hdrl = chunk(b"avih", &[0; 56]);
    hdrl.extend(list(b"strl", &v));
    hdrl.extend(list(b"strl", &a));
    let mut movi = Vec::new();
    for k in 0..frames {
        movi.extend(chunk(b"00db", &vec![(k * 40) as u8; frame_bytes]));
        movi.extend(chunk(b"01wb", &[0x10; 64]));
    }
    let mut riff = b"AVI ".to_vec();
    riff.extend(list(b"hdrl", &hdrl));
    riff.extend(list(b"movi", &movi));
    chunk(b"RIFF", &riff)
}

/// Hostile and damaged hand-made files open or fail with an error, never a panic: zero rates,
/// absurd dimensions, frames shorter than the picture, zero and huge sample rates and channel
/// counts, truncation, hostile reads.
#[test]
fn hostile_avi_files_fail_cleanly() {
    let ok = tiny(4, 2, 3, 4 * 3 * 2, 25, 1, 48_000, 1, 2);
    let src = crate::open_bytes("ok.avi", ok.clone().into()).unwrap();
    assert_eq!(src.info().container, "AVI");
    assert_eq!(src.info().video.as_ref().unwrap().codec, "Uncompressed RGB 24-bit");
    let f = src.video_frame(FrameRequest::full(Tick::ZERO)).unwrap();
    assert_eq!((f.width, f.height), (4, 2));
    assert_eq!(src.audio(0, 10, 48_000).unwrap().channels[0].len(), 10);
    // absurd reads are errors, not allocations
    assert!(src.audio(0, usize::MAX / 2, 48_000).is_err());
    assert!(src.audio(i64::MIN, 10, 0).is_err());
    let _ = src.audio(i64::MAX, 10, 48_000);
    let _ = src.audio(i64::MIN, 10, 1);
    for (name, file) in [
        ("zero rate", tiny(4, 2, 3, 24, 0, 0, 48_000, 1, 2)),
        ("huge picture", tiny(1 << 30, -(1 << 30), 3, 24, 25, 1, 48_000, 1, 2)),
        ("short frames", tiny(64, 64, 3, 10, 25, 1, 48_000, 1, 2)),
        ("no audio rate", tiny(4, 2, 3, 24, 25, 1, 0, 0, 0)),
        ("huge audio", tiny(4, 2, 3, 24, 25, 1, u32::MAX, u16::MAX, u16::MAX)),
        ("negative height", tiny(4, i32::MIN, 3, 24, 25, 1, 48_000, 1, 2)),
    ] {
        let Ok(src) = crate::open_bytes("hostile.avi", file.into()) else { continue };
        for t in [0, 1, 2, 100] {
            let _ = src.video_frame(FrameRequest::full(Tick::from_rational(t, 1, 25)));
        }
        let _ = src.audio(0, 1000, 48_000);
        let _ = src.info().duration;
        eprintln!("{name}: opened");
    }
    // every truncation and many single-byte mutations
    for cut in 0..ok.len() {
        if let Ok(src) = crate::open_bytes("cut.avi", ok[..cut].to_vec().into()) {
            let _ = src.video_frame(FrameRequest::full(Tick::from_rational(2, 1, 25)));
            let _ = src.audio(0, 100, 48_000);
        }
    }
    for at in (0..ok.len()).step_by(3) {
        for v in [0x00u8, 0xFF, 0x80] {
            let mut b = ok.clone();
            b[at] = v;
            if let Ok(src) = crate::open_bytes("mut.avi", b.into()) {
                let _ = src.video_frame(FrameRequest::full(Tick::ZERO));
                let _ = src.audio(0, 100, 44_100);
            }
        }
    }
}
