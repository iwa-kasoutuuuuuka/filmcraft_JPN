//! A real Filmora project, when one is at hand: `FILMCRAFT_FILMORA_SAMPLE=<path to a .wfp>` imports
//! it through `file.import` (as the app does) and checks the result; with
//! `FILMCRAFT_FILMORA_REFERENCE=<the video Filmora exported from it>` and ffmpeg, a few frames are
//! rendered and written next to the reference frames into `FILMCRAFT_FILMORA_OUT` (default: the
//! temporary folder) for a look. Nothing is written next to the project, and nothing is deleted.
//! Skipped without the variables (no Filmora projects are committed: they hold users' media paths).

use std::path::PathBuf;
use std::process::Command;

use filmcraft_engine::Session;
use serde_json::json;

#[test]
fn a_real_filmora_project_imports() {
    let Some(sample) = std::env::var_os("FILMCRAFT_FILMORA_SAMPLE").map(PathBuf::from) else {
        eprintln!("SKIPPED: set FILMCRAFT_FILMORA_SAMPLE to a .wfp");
        return;
    };
    let mut s = Session::default();
    let r = s.execute("file.import", json!({"paths": [sample]})).unwrap();
    println!("import: {}", serde_json::to_string_pretty(&r).unwrap_or_default());
    let errors = r["errors"].as_array().cloned().unwrap_or_default();
    assert!(errors.is_empty(), "{r}");
    let seq = s.active_sequence().expect("the project's timeline is open");
    let (w, h, rate) = (seq.settings.width, seq.settings.height, seq.settings.frame_rate);
    println!("sequence {w}x{h} @ {} fps, {:.2} s", rate.as_f64(), seq.duration().0 as f64 / filmcraft_time::TICKS_PER_SECOND as f64);
    for (i, t) in seq.video_tracks.iter().enumerate() {
        println!("  V{}: {} clips", i + 1, t.items.len());
    }
    for (i, t) in seq.audio_tracks.iter().enumerate() {
        println!("  A{}: {} clips", i + 1, t.items.len());
    }
    let clips: usize = seq.all_tracks().map(|t| t.items.len()).sum();
    assert!(clips > 0, "the timeline has clips");

    let (Some(reference), Some(ff)) = (std::env::var_os("FILMCRAFT_FILMORA_REFERENCE").map(PathBuf::from), filmcraft_testkit::ffmpeg()) else { return };
    let out = std::env::var_os("FILMCRAFT_FILMORA_OUT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("filmora-check");
    std::fs::create_dir_all(&out).unwrap();
    let times: Vec<f64> = std::env::var("FILMCRAFT_FILMORA_TIMES")
        .ok()
        .map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![1.0, 20.0, 60.0, 130.0, 200.0, 270.0]);
    for t in times {
        s.set_playhead(rate.snap(filmcraft_time::Tick::from_seconds_f64(t)));
        if let Some(q) = s.active_sequence() {
            let at = s.playhead();
            for (i, tr) in q.video_tracks.iter().enumerate() {
                for it in tr.items.iter().filter(|it| it.start <= at && at < it.end()) {
                    println!("  {t:.1} s  V{}: {}", i + 1, it.name);
                }
            }
        }
        let Some(img) = s.render_program(1.0) else { continue };
        let ours = img.over_black_rgba8();
        let theirs = Command::new(&ff)
            .args(["-v", "error", "-ss", &format!("{t}"), "-i"])
            .arg(&reference)
            .args(["-frames:v", "1", "-vf", &format!("scale={}:{}", img.w, img.h), "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .output()
            .map(|o| o.stdout)
            .unwrap_or_default();
        // side by side: ours | Filmora's
        let (iw, ih) = (img.w as u32, img.h as u32);
        let mut both = image::RgbaImage::new(iw * 2, ih);
        for y in 0..ih {
            for x in 0..iw {
                let k = ((y * iw + x) * 4) as usize;
                let px = |b: &[u8]| image::Rgba([b.get(k).copied().unwrap_or(0), b.get(k + 1).copied().unwrap_or(0), b.get(k + 2).copied().unwrap_or(0), 255]);
                both.put_pixel(x, y, px(&ours));
                both.put_pixel(x + iw, y, px(&theirs));
            }
        }
        let path = out.join(format!("t{t:06.1}.png"));
        both.save(&path).unwrap();
        println!("frame {t:.1} s: {}", path.display());
    }
}
