//! Automation registry: every interactive element registers a stable id and its screen rect each
//! frame, so agents can click/drag by id (`ui.click {id:"timeline.track.V1.lock"}`) and
//! `ui.inspect` can report the full on-screen widget tree.

use egui::Rect;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Element {
    pub id: String,
    pub label: String,
    pub rect: [f32; 4],
}

#[derive(Default)]
pub struct Registry {
    /// This frame's elements, while it is being laid out.
    pub elements: Vec<Element>,
    /// The last complete frame's elements (for lookups from control handlers between frames).
    pub previous: Vec<Element>,
    /// The frame before that: an element whose rect differs from it is still settling (a window's
    /// sizing pass, an anchored dialog being centred, a scroll or zoom animation).
    older: Vec<Element>,
    /// Complete frames so far (control requests waiting for an element count frames with it).
    frames: u64,
}

/// Where an element is, as far as synthetic input aimed at it is concerned.
pub enum Lookup<'a> {
    /// Not in the last frame (not open yet, scrolled away, or no such id).
    Missing,
    /// In the last frame but somewhere else (or not at all) the frame before.
    Moving(&'a Element),
    /// At the same place in the last two frames.
    Settled(&'a Element),
}

impl Registry {
    /// Register an element. The label is recorded in English whatever the UI language is
    /// (`crate::i18n::english_of`), so agents see the same labels as the ids they go with.
    pub fn add(&mut self, id: &str, rect: Rect, label: &str) {
        // nothing to reach (scrolled or clipped out of view)
        if !(rect.width() > 0.0 && rect.height() > 0.0) {
            return;
        }
        let label = crate::i18n::english_of(label).into_owned();
        self.elements.push(Element { id: id.to_string(), label, rect: [rect.min.x, rect.min.y, rect.width(), rect.height()] });
    }
    pub fn begin_frame(&mut self) {
        self.elements.clear();
    }
    /// The frame is laid out: its elements become [`Self::previous`].
    pub fn end_frame(&mut self) {
        self.older = std::mem::replace(&mut self.previous, std::mem::take(&mut self.elements));
        self.frames = self.frames.saturating_add(1);
    }
    /// Complete frames so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }
    pub fn find(&self, id: &str) -> Option<&Element> {
        self.previous.iter().chain(self.elements.iter()).find(|e| e.id == id)
    }
    /// [`Self::find`] in the last complete frame, and whether it has settled there.
    pub fn locate(&self, id: &str) -> Lookup<'_> {
        let Some(e) = self.previous.iter().find(|e| e.id == id) else { return Lookup::Missing };
        let same = |o: &Element| o.rect.iter().zip(e.rect.iter()).all(|(a, b)| (a - b).abs() < 0.5);
        if self.older.iter().find(|o| o.id == id).is_some_and(same) { Lookup::Settled(e) } else { Lookup::Moving(e) }
    }
    /// Elements whose id starts with a prefix.
    pub fn query(&self, prefix: &str) -> Vec<&Element> {
        self.previous.iter().filter(|e| e.id.starts_with(prefix)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32) -> Rect {
        Rect::from_min_size(egui::pos2(x, 10.0), egui::vec2(20.0, 10.0))
    }

    #[test]
    fn an_element_settles_once_it_stays_put_for_two_frames() {
        let mut r = Registry::default();
        assert!(matches!(r.locate("a"), Lookup::Missing));
        r.begin_frame();
        r.add("a", rect(0.0), "A");
        r.end_frame();
        assert!(matches!(r.locate("a"), Lookup::Moving(_)), "first seen");
        r.begin_frame();
        r.add("a", rect(100.0), "A");
        r.end_frame();
        assert!(matches!(r.locate("a"), Lookup::Moving(_)), "moved (a sizing pass, then centred)");
        r.begin_frame();
        r.add("a", rect(100.0), "A");
        r.end_frame();
        assert!(matches!(r.locate("a"), Lookup::Settled(e) if e.rect[0] == 100.0));
        assert_eq!(r.frames(), 3);
        r.begin_frame();
        r.end_frame();
        assert!(matches!(r.locate("a"), Lookup::Missing), "closed");
    }
}
