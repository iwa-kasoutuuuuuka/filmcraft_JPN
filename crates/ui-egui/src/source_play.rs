//! Source monitor playback: the clip open in the Source monitor plays on its own playhead (video
//! and, at normal speed, its audio), independently of the sequence. Space / J / K / L and the
//! transport's Play button drive it while the Source monitor has focus.

use filmcraft_time::Tick;

use crate::FilmcraftApp;

/// A running Source monitor playback.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SourcePlay {
    anchor_tick: Tick,
    /// Wall-clock time (egui seconds) of `anchor_tick`; negative until the first frame.
    anchor_time: f64,
    speed: f64,
    /// The audio device plays the clip and is the clock.
    audio: bool,
}

impl FilmcraftApp {
    /// Whether the Source monitor is playing.
    pub fn source_playing(&self) -> bool {
        self.source_play.is_some()
    }

    /// Play the Source monitor clip at `speed` (negative: backwards) from its playhead; stops
    /// sequence playback.
    pub(crate) fn play_source(&mut self, speed: f64) {
        let Some(item) = self.session.state.source_item else { return };
        let Some(v) = filmcraft_engine::clip_ops::source_view(&self.session, item) else { return };
        if self.playback.playing {
            self.stop();
        }
        self.stop_source();
        let last = (v.end - v.rate.frame_duration()).max(v.start);
        let mut from = self.session.state.source_playhead.clamp(v.start, last);
        // from the end, play again from the start (and backwards from the start, from the end)
        if speed > 0.0 && from >= last {
            from = v.start;
        } else if speed < 0.0 && from <= v.start {
            from = last;
        }
        self.session.state.source_playhead = from;
        let audio = (speed - 1.0).abs() < 1e-9 && self.start_source_audio(v.media, from);
        self.source_play = Some(SourcePlay { anchor_tick: from, anchor_time: -1.0, speed, audio });
    }

    pub(crate) fn stop_source(&mut self) {
        if let Some(p) = self.source_play.take()
            && p.audio
            && let Some(a) = self.audio.as_mut()
        {
            a.stop();
        }
    }

    pub(crate) fn toggle_source_play(&mut self) {
        if self.source_play.is_some() { self.stop_source() } else { self.play_source(1.0) }
    }

    /// J / L: shuttle faster in one direction (1×, 2×, 4×, 8×).
    pub(crate) fn shuttle_source(&mut self, forward: bool) {
        let cur = self.source_play.map(|p| p.speed).unwrap_or(0.0);
        let speed = if forward == (cur > 0.0) && cur != 0.0 {
            (cur * 2.0).clamp(-8.0, 8.0)
        } else if forward {
            1.0
        } else {
            -1.0
        };
        self.play_source(speed);
    }

    /// The clip's audio from `from` (media time) on the output device; false when it cannot play.
    fn start_source_audio(&mut self, media: filmcraft_engine::project::ItemId, from: Tick) -> bool {
        let services = self.session.services.clone();
        let Some(src) = self.session.media.source_for(&self.session.project, media, &*services) else { return false };
        if src.info().audio.is_none() {
            return false;
        }
        let map = [self.session.prefs.audio_hardware.map_left, self.session.prefs.audio_hardware.map_right];
        let Some(a) = self.audio.as_mut() else { return false };
        let sr = a.sample_rate();
        let mut cursor = from.to_units_floor(i64::from(sr));
        let fill = Box::new(move |buf: &mut [f32], ch: usize| {
            let n = buf.len() / ch.max(1);
            match src.audio(cursor, n, sr) {
                Ok(b) if !b.channels.is_empty() => {
                    let right = b.channels.len().min(2) - 1;
                    filmcraft_engine::settings::map_output(&b.channels[0], &b.channels[right], buf, ch, map);
                }
                _ => buf.fill(0.0),
            }
            cursor += n as i64;
        });
        a.start(fill).is_ok()
    }

    /// Move the Source playhead with the clock; stops at the clip's ends.
    pub(crate) fn advance_source_playback(&mut self, ctx: &egui::Context) {
        let Some(mut p) = self.source_play else { return };
        let Some(v) = self.session.state.source_item.and_then(|i| filmcraft_engine::clip_ops::source_view(&self.session, i)) else {
            self.stop_source();
            return;
        };
        let now = ctx.input(|i| i.time);
        if p.anchor_time < 0.0 {
            p.anchor_time = now;
        }
        let elapsed = match self.audio.as_ref().filter(|_| p.audio).and_then(|a| a.played_frames().map(|f| (f, a.sample_rate()))) {
            Some((f, sr)) if sr > 0 => f as f64 / f64::from(sr),
            _ => now - p.anchor_time,
        };
        let last = (v.end - v.rate.frame_duration()).max(v.start);
        let t = p.anchor_tick + Tick::from_seconds_f64(elapsed * p.speed);
        self.source_play = Some(p);
        if t >= last || t <= v.start && p.speed < 0.0 {
            self.session.state.source_playhead = t.clamp(v.start, last);
            self.stop_source();
        } else {
            self.session.state.source_playhead = v.rate.tick_of(v.rate.frame_at(t.max(v.start)));
        }
        ctx.request_repaint();
    }
}
