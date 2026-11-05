//! Generate Narration, driven headless through the control channel against a fake VOICEVOX
//! engine (a tiny HTTP server on 127.0.0.1): the dialog shows the engine running and its voices, a
//! pasted script becomes lines, Generate renders in the background and places clips + captions,
//! and the credits appear; an engine that is not running shows the "start the app" message.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

fn wav(seconds: f64) -> Vec<u8> {
    let frames = (seconds * 24_000.0) as usize;
    let data = frames * 2;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&((36 + data) as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&24_000u32.to_le_bytes());
    v.extend_from_slice(&48_000u32.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data as u32).to_le_bytes());
    v.resize(44 + data, 0);
    v
}

fn serve(mut c: TcpStream) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        match c.read(&mut tmp) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let len: usize = head
        .lines()
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim().parse().unwrap_or(0)))
        .unwrap_or(0);
    while buf.len() < head_end + len {
        match c.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    let target = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_string();
    let path = target.split('?').next().unwrap_or("/");
    let (ctype, out): (&str, Vec<u8>) = match path {
        "/version" => ("application/json", b"\"0.20.0\"".to_vec()),
        "/speakers" => (
            "application/json",
            json!([
                {"name": "ずんだもん", "speaker_uuid": "uuid-zunda", "styles": [{"name": "ノーマル", "id": 3}]},
                {"name": "四国めたん", "speaker_uuid": "uuid-metan", "styles": [{"name": "ノーマル", "id": 2}]}
            ])
            .to_string()
            .into_bytes(),
        ),
        "/speaker_info" => ("application/json", json!({"policy": "terms text", "portrait": ""}).to_string().into_bytes()),
        "/audio_query" => ("application/json", json!({"accent_phrases": [], "speedScale": 1.0}).to_string().into_bytes()),
        "/synthesis" => ("audio/wav", wav(0.5)),
        _ => ("application/json", b"{}".to_vec()),
    };
    let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len()).as_bytes());
    let _ = c.write_all(&out);
}

fn fake_engine() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            std::thread::spawn(move || serve(c));
        }
    });
    url
}

fn closed_url() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = l.local_addr().unwrap();
    drop(l);
    format!("http://{a}")
}

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
}

impl Driver {
    fn demo(voicevox: &str, aivis: &str) -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        session.prefs.narration.voicevox_url = voicevox.to_string();
        session.prefs.narration.aivisspeech_url = aivis.to_string();
        session.prefs.narration.engine = "voicevox".into();
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.frames(4);
        d
    }
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }
    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method} {params}");
    }
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params.clone());
        assert_eq!(r["ok"], true, "{method} {params}: {r}");
        r["result"].clone()
    }
    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id }));
        self.frames(2);
    }
    fn dialog(&mut self) -> Value {
        self.ok("ui.inspect", json!({}))["ui"]["narration"].clone()
    }
}

#[test]
fn generate_narration_from_a_pasted_script() {
    let url = fake_engine();
    let mut d = Driver::demo(&url, &closed_url());
    let r = d.ok("ui.menu.invoke", json!({"id": "tts.synthesize"}));
    assert_eq!(r["dialog"], "narration");
    d.frames(3);
    let n = d.dialog();
    assert_eq!(n["engine"], "voicevox");
    let vv = n["engines"].as_array().unwrap().iter().find(|e| e["id"] == "voicevox").unwrap().clone();
    assert_eq!(vv["running"], true);
    assert_eq!(n["voices"].as_array().unwrap().len(), 2);
    assert_eq!(n["lines"][0]["voice"], "ずんだもん/ノーマル", "an empty line gets the first voice");

    // paste a two-voice script
    d.ok("ui.set", json!({"narration": {"script": "ずんだもん: こんにちは\n四国めたん: よろしくね"}}));
    d.frames(2);
    // a script in the box opens its section; Add Lines replaces the empty first line
    d.click("narration.addScript");
    let lines = d.dialog()["lines"].clone();
    assert_eq!(lines.as_array().unwrap().len(), 2, "{lines}");
    assert_eq!((lines[0]["voice"].as_str(), lines[1]["voice"].as_str()), (Some("ずんだもん/ノーマル"), Some("四国めたん/ノーマル")));
    assert_eq!(lines[1]["text"], "よろしくね");
    d.frames(2);
    d.click("narration.generate");
    for _ in 0..300 {
        d.frames(1);
        if d.dialog()["busy"] == false {
            break;
        }
    }
    let n = d.dialog();
    assert_eq!(n["error"], "", "{n}");
    assert_eq!(n["credits"], json!(["VOICEVOX:ずんだもん", "VOICEVOX:四国めたん"]));
    // two clips and two captions in the sequence, one undo step
    let seq = d.ok("engine.execute", json!({"command": "history.list", "params": {}}));
    assert_eq!(seq["undo"].as_array().unwrap().last().unwrap(), "Synthesize Narration");
    d.click("narration.copyCredits");
    d.click("narration.cancel");
    assert!(d.dialog().is_null());
}

#[test]
fn an_engine_that_is_not_running_says_so() {
    let down = closed_url();
    let mut d = Driver::demo(&down, &closed_url());
    d.ok("ui.menu.invoke", json!({"id": "tts.synthesize"}));
    d.frames(3);
    let n = d.dialog();
    assert!(n["error"].as_str().unwrap().contains("not running") || n["error"].as_str().unwrap().contains("起動していません"), "{n}");
    let ids = d.ok("ui.elements", json!({"prefix": "narration."})).to_string();
    assert!(ids.contains("narration.getEngine") && ids.contains("narration.refresh"), "{ids}");
    // Generate with no text says what to do instead of failing silently
    d.click("narration.generate");
    assert!(!d.dialog()["error"].as_str().unwrap().is_empty());
}
