//! Real-world media, imported, edited and delivered through engine commands, with ffmpeg /
//! ffprobe (external test oracles) making the files and judging the exports:
//!
//! - a portrait, variable-frame-rate phone clip with a flash and a beep at the same instant: the
//!   export stays portrait and as long, and picture and sound stay in sync;
//! - an iPhone-style HDR clip (HEVC Main 10, HLG, QuickTime): the export decodes and has a picture;
//! - a screen recording (VP9 + Opus WebM at 1366×768): size and audio survive;
//! - a long-GOP H.264 clip with B-frames cut in the middle: every exported frame is the source
//!   frame it should be;
//! - a download cut short: the export still completes.

use std::path::{Path, PathBuf};
use std::process::Command;

use filmcraft_engine::Session;
use serde_json::{Value, json};

fn out_dir(name: &str) -> PathBuf {
    let d = filmcraft_testkit::fixtures_dir("engine").join(format!("real-media {name} {}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ffmpeg(ff: &Path, args: &[&str]) -> Vec<u8> {
    let o = Command::new(ff).args(["-v", "error", "-y"]).args(args).output().unwrap();
    assert!(o.status.success(), "ffmpeg {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, p.clone()).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

/// Import one file and make a sequence from it (its settings follow the clip).
fn session_with(file: &Path) -> Session {
    let mut s = Session::default();
    let r = run(&mut s, "file.import", json!({"paths": [file]}));
    assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
    let item = r["items"][0].as_u64().unwrap_or_else(|| panic!("imported: {r}"));
    run(&mut s, "file.newSequenceFromClip", json!({"items": [item]}));
    s
}

fn export(s: &mut Session, path: &Path, extra: Value) -> Value {
    let mut p = json!({"path": path, "format": "h264", "wait": true});
    if let (Some(o), Some(e)) = (p.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    run(s, "file.exportMedia", p)
}

fn probe(fp: &Path, file: &Path) -> Value {
    let o = Command::new(fp)
        .args(["-v", "error", "-show_entries", "stream=codec_type,codec_name,width,height,color_transfer:format=duration", "-of", "json"])
        .arg(file)
        .output()
        .unwrap();
    serde_json::from_slice(&o.stdout).unwrap()
}

fn stream<'a>(p: &'a Value, kind: &str) -> &'a Value {
    p["streams"].as_array().and_then(|a| a.iter().find(|s| s["codec_type"] == kind)).unwrap_or(&Value::Null)
}

fn duration(p: &Value) -> f64 {
    p["format"]["duration"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0.0)
}

/// Presentation times (s, relative to the first) of the video frames.
fn frame_times(fp: &Path, file: &Path) -> Vec<f64> {
    let o = Command::new(fp).args(["-v", "error", "-select_streams", "v:0", "-show_entries", "frame=pts_time", "-of", "csv=p=0"]).arg(file).output().unwrap();
    let t: Vec<f64> = String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().trim_end_matches(',').parse().ok()).collect();
    let t0 = t.first().copied().unwrap_or(0.0);
    t.into_iter().map(|x| x - t0).collect()
}

/// Every frame as 8-bit grey at `w`×`h` (display orientation, every coded frame once).
fn grey_frames(ff: &Path, file: &Path, w: usize, h: usize) -> Vec<Vec<u8>> {
    let raw =
        ffmpeg(ff, &["-i", file.to_str().unwrap(), "-fps_mode", "passthrough", "-vf", &format!("scale={w}:{h}"), "-f", "rawvideo", "-pix_fmt", "gray", "-"]);
    raw.chunks_exact(w * h).map(<[u8]>::to_vec).collect()
}

fn mean(f: &[u8]) -> f64 {
    f.iter().map(|&v| v as f64).sum::<f64>() / f.len().max(1) as f64
}

/// Time of the first frame brighter than mid grey (the flash).
fn flash_time(ff: &Path, fp: &Path, file: &Path) -> f64 {
    let frames = grey_frames(ff, file, 16, 16);
    let times = frame_times(fp, file);
    let i = frames.iter().position(|f| mean(f) > 128.0).unwrap_or_else(|| panic!("{}: no flash", file.display()));
    times[i]
}

/// Time of the first loud audio sample (the beep), from the start of the file's timeline.
fn beep_time(ff: &Path, file: &Path) -> f64 {
    let pcm = ffmpeg(ff, &["-i", file.to_str().unwrap(), "-vn", "-ac", "1", "-ar", "48000", "-f", "s16le", "-"]);
    let i = pcm.as_chunks::<2>().0.iter().position(|s| i16::from_le_bytes(*s).unsigned_abs() > 8000).unwrap_or_else(|| panic!("{}: no beep", file.display()));
    i as f64 / 48_000.0
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse = a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum::<f64>() / a.len().max(1) as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

/// ffmpeg decodes the whole file without an error.
fn decodes_cleanly(ff: &Path, file: &Path) {
    let o = Command::new(ff).args(["-v", "error", "-i"]).arg(file).args(["-f", "null", "-"]).output().unwrap();
    assert!(o.status.success() && o.stderr.is_empty(), "{}: ffmpeg decode errors: {}", file.display(), String::from_utf8_lossy(&o.stderr));
}

#[test]
fn portrait_vfr_phone_clip_keeps_orientation_length_and_sync() {
    let (ff, fp) = filmcraft_testkit::require_oracles!();
    let d = out_dir("phone");
    // 4 s, every 7th frame held 20 ms longer (variable frame rate), B-frames, AAC; a white flash
    // and a 1 kHz beep both at 2.0 s of the file's timeline
    let base = d.join("landscape.mp4");
    ffmpeg(
        &ff,
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=0x303030:s=320x176:r=30,setpts='PTS+trunc(N/7)*0.02/TB',drawbox=x=0:y=0:w=iw:h=ih:color=white:t=fill:enable='between(t,2,2.06)'",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc='0.8*sin(2*PI*1000*t)*between(t,2,2.06)':s=48000",
            "-t",
            "4",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-bf",
            "2",
            "-fps_mode",
            "vfr",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            base.to_str().unwrap(),
        ],
    );
    // as a phone held upright stores it: landscape pixels with a display rotation
    let clip = d.join("portrait 縦.mp4");
    ffmpeg(&ff, &["-display_rotation", "90", "-i", base.to_str().unwrap(), "-c", "copy", clip.to_str().unwrap()]);
    let gaps: Vec<f64> = frame_times(&fp, &clip).windows(2).map(|w| w[1] - w[0]).collect();
    assert!(gaps.iter().any(|g| (g - 1.0 / 30.0).abs() > 0.01), "the clip really is variable frame rate");

    let mut s = session_with(&clip);
    let out = d.join("delivery.mp4");
    let r = export(&mut s, &out, json!({}));
    assert!(r["result"]["frames"].as_u64().unwrap_or(0) > 0, "{r}");

    let p = probe(&fp, &out);
    let v = stream(&p, "video");
    assert_eq!((v["width"].as_u64(), v["height"].as_u64()), (Some(176), Some(320)), "portrait export: {p}");
    assert_eq!(stream(&p, "audio")["codec_name"], "aac", "{p}");
    let (src_len, out_len) = (duration(&probe(&fp, &clip)), duration(&p));
    assert!((out_len - src_len).abs() < 0.1, "length {out_len} s, source {src_len} s");
    decodes_cleanly(&ff, &out);

    // picture and sound: where the source has them, and together
    let (src_flash, src_beep) = (flash_time(&ff, &fp, &clip), beep_time(&ff, &clip));
    let (out_flash, out_beep) = (flash_time(&ff, &fp, &out), beep_time(&ff, &out));
    eprintln!("flash {src_flash:.3} → {out_flash:.3} s, beep {src_beep:.3} → {out_beep:.3} s");
    assert!((out_flash - src_flash).abs() <= 1.0 / 30.0 + 0.005, "flash moved: {src_flash} → {out_flash}");
    assert!((out_beep - src_beep).abs() <= 0.01, "beep moved: {src_beep} → {out_beep}");
    assert!((out_flash - out_beep).abs() <= 1.0 / 30.0 + 0.01, "out of sync: flash {out_flash}, beep {out_beep}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn iphone_style_hdr_hevc_mov_exports() {
    let (ff, fp) = filmcraft_testkit::require_oracles!();
    let encoders = Command::new(&ff).args(["-hide_banner", "-encoders"]).output().unwrap();
    if !String::from_utf8_lossy(&encoders.stdout).contains("libx265") {
        eprintln!("this ffmpeg has no libx265: skipped");
        return;
    }
    let d = out_dir("hdr");
    let clip = d.join("IMG_0001.MOV");
    ffmpeg(
        &ff,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "2",
            "-c:v",
            "libx265",
            "-x265-params",
            "log-level=error:colorprim=bt2020:transfer=arib-std-b67:colormatrix=bt2020nc",
            "-pix_fmt",
            "yuv420p10le",
            "-tag:v",
            "hvc1",
            "-color_primaries",
            "bt2020",
            "-color_trc",
            "arib-std-b67",
            "-colorspace",
            "bt2020nc",
            "-c:a",
            "aac",
            clip.to_str().unwrap(),
        ],
    );
    let mut s = session_with(&clip);
    // a sequence from an HDR clip works in its HDR space (as Premiere does)
    let q = run(&mut s, "sequence.inspect", json!({}));
    assert_eq!(q["settings"]["color"]["working"], "Rec2100Hlg", "{}", q["settings"]);
    for (name, extra, transfer) in [("as the sequence.mp4", json!({}), "arib-std-b67"), ("SDR.mp4", json!({"settings": {"sdr": true}}), "bt709")] {
        let out = d.join(name);
        let r = export(&mut s, &out, extra);
        assert!(r["result"]["frames"].as_u64().is_some_and(|f| f == 60), "2 s at 30 fps: {r}");
        let p = probe(&fp, &out);
        assert!((duration(&p) - 2.0).abs() < 0.1, "{name}: {p}");
        decodes_cleanly(&ff, &out);
        // a picture, not black or flat
        let frames = grey_frames(&ff, &out, 64, 36);
        let m = mean(&frames[30]);
        let var = frames[30].iter().map(|&v| (v as f64 - m).powi(2)).sum::<f64>() / frames[30].len() as f64;
        eprintln!("{name}: transfer {}, mean luma {m:.1}, variance {var:.0}", stream(&p, "video")["color_transfer"]);
        assert!((20.0..235.0).contains(&m) && var > 100.0, "{name}: mean {m}, variance {var}");
        assert_eq!(stream(&p, "video")["color_transfer"], transfer, "{name}: {p}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn screen_recording_keeps_its_size_and_sound() {
    let (ff, fp) = filmcraft_testkit::require_oracles!();
    let d = out_dir("screen");
    let clip = d.join("画面収録 1366x768.webm");
    ffmpeg(
        &ff,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1366x768:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "1.2",
            "-c:v",
            "libvpx-vp9",
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "libopus",
            clip.to_str().unwrap(),
        ],
    );
    let mut s = session_with(&clip);
    let out = d.join("screen.mp4");
    export(&mut s, &out, json!({}));
    let p = probe(&fp, &out);
    let v = stream(&p, "video");
    assert_eq!((v["width"].as_u64(), v["height"].as_u64()), (Some(1366), Some(768)), "{p}");
    assert_eq!(stream(&p, "audio")["codec_name"], "aac", "{p}");
    assert!((duration(&p) - 1.2).abs() < 0.1, "{p}");
    decodes_cleanly(&ff, &out);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn long_gop_cut_exports_the_right_frames() {
    let (ff, fp) = filmcraft_testkit::require_oracles!();
    let d = out_dir("long-gop");
    // one keyframe in 300 frames, three B-frames: the cut at 3 s is deep inside a GOP
    let clip = d.join("long gop.mp4");
    ffmpeg(
        &ff,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x176:rate=30",
            "-t",
            "6",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-g",
            "300",
            "-bf",
            "3",
            "-crf",
            "12",
            clip.to_str().unwrap(),
        ],
    );
    let mut s = session_with(&clip);
    run(&mut s, "timeline.razor", json!({"seconds": 3.0}));
    let q = run(&mut s, "sequence.inspect", json!({}));
    let first = q["video"][0]["items"][0]["clip"].as_u64().unwrap_or_else(|| panic!("clips: {q}"));
    run(&mut s, "edit.rippleDelete", json!({"clips": [first]}));
    let out = d.join("from 3 s.mp4");
    let r = export(&mut s, &out, json!({"settings": {"bitrateKbps": 20000}}));
    assert_eq!(r["result"]["frames"].as_u64(), Some(90), "3 s left at 30 fps: {r}");
    let _ = fp;
    let src = grey_frames(&ff, &clip, 320, 176);
    let got = grey_frames(&ff, &out, 320, 176);
    assert_eq!(got.len(), 90);
    for k in [0usize, 1, 7, 29, 60, 89] {
        let right = psnr(&got[k], &src[90 + k]);
        let before = psnr(&got[k], &src[89 + k]);
        // (the last frame has no successor)
        let after = src.get(91 + k).map_or(0.0, |n| psnr(&got[k], n));
        assert!(right > 32.0 && right > before + 3.0 && right > after + 3.0, "frame {k}: {right:.1} dB vs neighbours {before:.1} / {after:.1}");
    }
    decodes_cleanly(&ff, &out);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn download_cut_short_still_exports() {
    let (ff, fp) = filmcraft_testkit::require_oracles!();
    let d = out_dir("cut-short");
    let full = d.join("full.mp4");
    ffmpeg(
        &ff,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x176:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-g",
            "15",
            "-c:a",
            "aac",
            "-movflags",
            "+faststart",
            full.to_str().unwrap(),
        ],
    );
    let data = std::fs::read(&full).unwrap();
    let clip = d.join("download cut short.mp4");
    std::fs::write(&clip, &data[..data.len() * 6 / 10]).unwrap();
    let mut s = session_with(&clip);
    let out = d.join("export.mp4");
    let r = export(&mut s, &out, json!({}));
    assert!(r["result"]["frames"].as_u64().is_some_and(|f| f >= 30), "{r}");
    let p = probe(&fp, &out);
    assert!(duration(&p) > 1.0, "{p}");
    decodes_cleanly(&ff, &out);
    // the part that was downloaded has its picture
    let frames = grey_frames(&ff, &out, 64, 36);
    let src = grey_frames(&ff, &full, 64, 36);
    assert!(psnr(&frames[5], &src[5]) > 25.0, "frame 5 is the clip's");
    let _ = std::fs::remove_dir_all(&d);
}
