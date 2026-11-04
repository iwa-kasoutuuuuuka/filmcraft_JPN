//! Animated charts: a data table (categories × steps) shown as a bar chart race, bars, columns,
//! lines, areas or a pie / donut, animated over the clip. A chart lives on a graphic clip
//! ([`crate::graphic_design::GraphicMeta::chart`]) and is drawn behind its layers by the renderer.
//!
//! Timing: the first [`ChartSpec::intro_seconds`] grow the chart in from zero; the steps then play
//! evenly across the clip, and the last [`ChartSpec::hold_seconds`] hold the final step. Between
//! two steps values move linearly and bar ranks glide (eased), so a race reorders smoothly.
//!
//! Every number here comes from a project file or a command and is hostile: non-finite values
//! count as zero, the table is capped ([`MAX_CATEGORIES`], [`MAX_STEPS`]) and lengths that do not
//! match are padded with zeros when read.

use serde::{Deserialize, Serialize};

use crate::ItemId;

/// Most categories (rows) a chart keeps.
pub const MAX_CATEGORIES: usize = 200;
/// Most steps (columns, e.g. years) a chart keeps.
pub const MAX_STEPS: usize = 1000;
/// Longest text kept for a title, category or step label (in characters).
pub const MAX_LABEL_CHARS: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChartKind {
    /// Horizontal bars sorted by value, reordering as the steps play.
    #[default]
    BarRace,
    /// Horizontal bars in table order.
    Bar,
    /// Vertical bars in table order.
    Column,
    /// One line per category across the steps, drawn from left to right.
    Line,
    /// [`ChartKind::Line`] with the area under each line filled.
    Area,
    Pie,
    Donut,
}

impl ChartKind {
    pub const ALL: [ChartKind; 7] = [ChartKind::BarRace, ChartKind::Bar, ChartKind::Column, ChartKind::Line, ChartKind::Area, ChartKind::Pie, ChartKind::Donut];

    /// The id used by commands (`"barRace"`, `"pie"`, …).
    pub fn id(self) -> &'static str {
        match self {
            ChartKind::BarRace => "barRace",
            ChartKind::Bar => "bar",
            ChartKind::Column => "column",
            ChartKind::Line => "line",
            ChartKind::Area => "area",
            ChartKind::Pie => "pie",
            ChartKind::Donut => "donut",
        }
    }

    pub fn from_id(id: &str) -> Option<ChartKind> {
        ChartKind::ALL.into_iter().find(|k| k.id().eq_ignore_ascii_case(id))
    }

    /// English display name.
    pub fn name(self) -> &'static str {
        match self {
            ChartKind::BarRace => "Bar Chart Race",
            ChartKind::Bar => "Bar Chart",
            ChartKind::Column => "Column Chart",
            ChartKind::Line => "Line Chart",
            ChartKind::Area => "Area Chart",
            ChartKind::Pie => "Pie Chart",
            ChartKind::Donut => "Donut Chart",
        }
    }

    /// Whether the steps run along the x axis (lines, areas) rather than through time.
    pub fn steps_on_axis(self) -> bool {
        matches!(self, ChartKind::Line | ChartKind::Area)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChartTheme {
    #[default]
    Dark,
    Light,
}

/// Built-in colour palettes (our own picks), as sRGB bytes.
pub const PALETTES: &[(&str, &[[u8; 3]])] = &[
    (
        "Vivid",
        &[
            [239, 128, 90],
            [133, 92, 235],
            [232, 122, 122],
            [222, 88, 150],
            [137, 160, 178],
            [94, 186, 98],
            [204, 204, 88],
            [222, 168, 74],
            [94, 128, 245],
            [36, 178, 212],
        ],
    ),
    ("Ocean", &[[0, 119, 182], [0, 150, 199], [0, 180, 216], [72, 202, 228], [144, 224, 239], [3, 4, 94], [2, 62, 138], [173, 232, 244]]),
    ("Sunset", &[[255, 89, 94], [255, 146, 76], [255, 202, 58], [138, 201, 38], [25, 130, 196], [106, 76, 147], [244, 132, 95], [242, 92, 84]]),
    ("Forest", &[[45, 106, 79], [64, 145, 108], [82, 183, 136], [116, 198, 157], [149, 213, 178], [27, 67, 50], [183, 228, 199], [8, 28, 21]]),
    ("Pastel", &[[255, 173, 173], [255, 214, 165], [253, 255, 182], [202, 255, 191], [155, 246, 255], [160, 196, 255], [189, 178, 255], [255, 198, 255]]),
    ("Mono", &[[230, 230, 230], [200, 200, 200], [170, 170, 170], [140, 140, 140], [110, 110, 110], [80, 80, 80]]),
];

/// Longest file path kept for a category image (in characters).
pub const MAX_PATH_CHARS: usize = 4096;

/// A category's picture (a flag, a logo): a project media item whose first frame is drawn beside
/// the category's bar, column or legend entry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChartImage {
    /// The project item holding the picture (a still, or a movie's first frame).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<ItemId>,
    /// The file the picture came from. Commands import it into the project (and set `item`)
    /// when `item` is missing; the renderer only reads `item`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
}

impl ChartImage {
    pub fn of_item(item: ItemId) -> ChartImage {
        ChartImage { item: Some(item), path: String::new() }
    }
}

/// How category pictures are cropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageShape {
    #[default]
    Circle,
    /// A square with rounded corners.
    Rounded,
}

impl ImageShape {
    fn is_default(&self) -> bool {
        *self == ImageShape::Circle
    }
}

/// An animated chart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChartSpec {
    pub kind: ChartKind,
    /// Header text (empty: no header).
    pub title: String,
    /// Row names (countries, products, …).
    pub categories: Vec<String>,
    /// Column names (years, months, …), in playing order.
    pub steps: Vec<String>,
    /// `values[category][step]`.
    pub values: Vec<Vec<f64>>,
    pub theme: ChartTheme,
    /// Category colours, repeated when there are more categories (empty: the first palette).
    pub palette: Vec<[u8; 3]>,
    /// Font family (empty: the default interface font).
    pub font: String,
    /// Draw each bar's / slice's value.
    pub show_values: bool,
    /// Draw the current step (e.g. the year) large in the corner.
    pub show_step: bool,
    pub show_grid: bool,
    /// Names and colours beside line, area and pie charts.
    pub show_legend: bool,
    /// A rounded card behind the chart.
    pub background: bool,
    /// Bars shown at once in a race or bar chart.
    pub max_bars: u32,
    /// Digits after the decimal point in value labels.
    pub decimals: u32,
    /// Seconds the chart takes to grow in at the clip start.
    pub intro_seconds: f64,
    /// Seconds the last step holds at the clip end.
    pub hold_seconds: f64,
    /// A picture per category (aligned with `categories`; missing entries have none).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Option<ChartImage>>,
    /// How the pictures are cropped.
    #[serde(skip_serializing_if = "ImageShape::is_default")]
    pub image_shape: ImageShape,
}

impl Default for ChartSpec {
    fn default() -> Self {
        ChartSpec {
            kind: ChartKind::BarRace,
            title: String::new(),
            categories: Vec::new(),
            steps: Vec::new(),
            values: Vec::new(),
            theme: ChartTheme::Dark,
            palette: Vec::new(),
            font: String::new(),
            show_values: true,
            show_step: true,
            show_grid: true,
            show_legend: true,
            background: true,
            max_bars: 10,
            decimals: 0,
            intro_seconds: 1.0,
            hold_seconds: 1.0,
            images: Vec::new(),
            image_shape: ImageShape::Circle,
        }
    }
}

/// Cut `s` to [`MAX_LABEL_CHARS`] characters.
fn clip_label(s: &str) -> String {
    s.chars().take(MAX_LABEL_CHARS).collect()
}

fn finite(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

impl ChartSpec {
    /// A sample chart of `kind`, so a new chart shows something to edit.
    pub fn sample(kind: ChartKind) -> ChartSpec {
        let categories: Vec<String> = ["Tokyo", "Osaka", "Nagoya", "Fukuoka", "Sapporo", "Sendai"].map(String::from).to_vec();
        let steps: Vec<String> = (2019..=2024).map(|y| y.to_string()).collect();
        let base = [92.0, 71.0, 55.0, 38.0, 30.0, 24.0];
        let growth = [1.04, 1.07, 1.02, 1.12, 1.09, 1.15];
        let values = base.iter().zip(growth).map(|(&b, g)| (0..steps.len()).map(|i| (b * f64::powi(g, i as i32) * 10.0).round() / 10.0).collect()).collect();
        ChartSpec { kind, title: "Visitors (10,000s)".into(), categories, steps, values, decimals: 1, ..ChartSpec::default() }
    }

    /// The table cut to the caps, labels cut to [`MAX_LABEL_CHARS`], every row as long as
    /// `steps` (padded with zeros), and settings in range.
    pub fn sanitized(&self) -> ChartSpec {
        let mut c = self.clone();
        c.title = clip_label(&c.title);
        c.categories.truncate(MAX_CATEGORIES);
        c.categories.iter_mut().for_each(|s| *s = clip_label(s));
        c.steps.truncate(MAX_STEPS);
        c.steps.iter_mut().for_each(|s| *s = clip_label(s));
        if c.steps.is_empty() {
            c.steps.push(String::new());
        }
        let n = c.steps.len();
        c.values.resize(c.categories.len(), Vec::new());
        c.values.truncate(c.categories.len());
        for row in &mut c.values {
            row.resize(n, 0.0);
            row.iter_mut().for_each(|v| *v = finite(*v));
        }
        c.palette.truncate(64);
        c.font = clip_label(&c.font);
        c.max_bars = c.max_bars.clamp(1, 50);
        c.decimals = c.decimals.min(6);
        c.intro_seconds = finite(c.intro_seconds).clamp(0.0, 60.0);
        c.hold_seconds = finite(c.hold_seconds).clamp(0.0, 60.0);
        c.images.resize(c.categories.len(), None);
        for img in c.images.iter_mut() {
            if let Some(i) = img {
                i.path = i.path.chars().take(MAX_PATH_CHARS).collect();
                if i.item.is_none() && i.path.trim().is_empty() {
                    *img = None;
                }
            }
        }
        if c.images.iter().all(Option::is_none) {
            c.images.clear();
        }
        c
    }

    /// The picture of category `i`, if it has one.
    pub fn image(&self, i: usize) -> Option<&ChartImage> {
        self.images.get(i).and_then(Option::as_ref)
    }

    /// Whether any category has a picture.
    pub fn has_images(&self) -> bool {
        self.images.iter().any(Option::is_some)
    }

    /// Set (or with `None` clear) the picture of category `i` (ignored past the last category).
    pub fn set_image(&mut self, i: usize, image: Option<ChartImage>) {
        if i >= self.categories.len() {
            return;
        }
        if self.images.len() <= i {
            self.images.resize(i + 1, None);
        }
        if let Some(slot) = self.images.get_mut(i) {
            *slot = image;
        }
    }

    /// Remove category `i` with its values and picture.
    pub fn remove_category(&mut self, i: usize) {
        if i >= self.categories.len() {
            return;
        }
        self.categories.remove(i);
        if i < self.values.len() {
            self.values.remove(i);
        }
        if i < self.images.len() {
            self.images.remove(i);
        }
    }

    /// Move category `from` to place `to` (its values and picture move with it).
    pub fn move_category(&mut self, from: usize, to: usize) {
        let n = self.categories.len();
        if from >= n || to >= n || from == to {
            return;
        }
        self.values.resize(n, Vec::new());
        self.images.resize(n, None);
        let c = self.categories.remove(from);
        self.categories.insert(to, c);
        let v = self.values.remove(from);
        self.values.insert(to, v);
        let im = self.images.remove(from);
        self.images.insert(to, im);
    }

    /// Replace the table (e.g. a pasted one), keeping each picture with the category of the same
    /// name.
    pub fn set_table(&mut self, categories: Vec<String>, steps: Vec<String>, values: Vec<Vec<f64>>) {
        let old: Vec<(String, ChartImage)> =
            self.categories.iter().zip(&self.images).filter_map(|(c, i)| i.clone().map(|i| (c.trim().to_string(), i))).collect();
        self.images = categories.iter().map(|c| old.iter().find(|(n, _)| n == c.trim()).map(|(_, i)| i.clone())).collect();
        self.categories = categories;
        self.steps = steps;
        self.values = values;
        if !self.has_images() {
            self.images.clear();
        }
    }

    /// The value of `category` at `step` (zero when missing or not finite).
    pub fn value(&self, category: usize, step: usize) -> f64 {
        self.values.get(category).and_then(|r| r.get(step)).copied().map_or(0.0, finite)
    }

    /// Colour of category `i` (sRGB bytes).
    pub fn color(&self, i: usize) -> [u8; 3] {
        let pal: &[[u8; 3]] = if self.palette.is_empty() { PALETTES.first().map_or(&[[128, 128, 128]], |p| p.1) } else { &self.palette };
        pal.get(i % pal.len().max(1)).copied().unwrap_or([128, 128, 128])
    }

    /// Parse a table: the first row is a corner cell then the step names; each further row is a
    /// category name then its values. Cells are separated by tabs (a spreadsheet paste),
    /// semicolons or commas, whichever the first row uses most; `"…"` quotes a cell. Values may
    /// use thousands separators (`1,234` inside quotes or with tabs) and a trailing `%`; a cell
    /// that is not a number counts as zero. Errors when there is no data row.
    pub fn parse_table(text: &str) -> Result<(Vec<String>, Vec<String>, Vec<Vec<f64>>), String> {
        let mut lines = text.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.trim().is_empty());
        let head = lines.next().ok_or("the table is empty")?;
        let sep = ['\t', ';', ',']
            .into_iter()
            .max_by_key(|&c| (head.matches(c).count(), c == '\t', c == ';'))
            .filter(|&c| head.contains(c))
            .ok_or("the first row needs the step names after a corner cell, separated by tabs or commas")?;
        let steps: Vec<String> = split_row(head, sep).into_iter().skip(1).take(MAX_STEPS).map(|s| clip_label(s.trim())).collect();
        if steps.is_empty() {
            return Err("the first row has no step names".into());
        }
        let mut categories = Vec::new();
        let mut values = Vec::new();
        for line in lines.take(MAX_CATEGORIES) {
            let cells = split_row(line, sep);
            let Some((name, rest)) = cells.split_first() else { continue };
            categories.push(clip_label(name.trim()));
            values.push((0..steps.len()).map(|i| rest.get(i).map_or(0.0, |c| parse_number(c))).collect());
        }
        if categories.is_empty() {
            return Err("the table has no data rows".into());
        }
        Ok((categories, steps, values))
    }

    /// The table as tab-separated text ([`ChartSpec::parse_table`] reads it back).
    pub fn table_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&std::iter::once(String::new()).chain(self.steps.iter().map(|s| quote(s))).collect::<Vec<_>>().join("\t"));
        for (i, c) in self.categories.iter().enumerate() {
            out.push('\n');
            out.push_str(&quote(c));
            for j in 0..self.steps.len() {
                out.push('\t');
                out.push_str(&format_plain(self.value(i, j)));
            }
        }
        out
    }
}

fn quote(s: &str) -> String {
    if s.contains(['\t', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

/// A number without a trailing `.0` (`12`, `12.5`).
fn format_plain(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
}

/// Split one row on `sep`, honouring `"…"` quotes (`""` is a quote inside them).
fn split_row(line: &str, sep: char) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' && cur.trim().is_empty() {
            cur.clear();
            quoted = true;
        } else if c == sep {
            cells.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    cells.push(cur);
    cells
}

/// A cell as a number: spaces, thousands separators (`,`, `_`, `'`), a trailing `%` and full-width
/// digits are allowed; anything else is zero.
pub fn parse_number(cell: &str) -> f64 {
    let s: String = cell
        .trim()
        .chars()
        .map(|c| match c {
            '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c),
            '．' => '.',
            '－' | '−' => '-',
            _ => c,
        })
        .filter(|c| !matches!(c, ',' | '_' | '\'' | ' ' | '%' | '\u{a0}'))
        .collect();
    s.parse::<f64>().ok().map_or(0.0, finite)
}

/// Thousands-separated value with `decimals` digits (`300,165,618`, `12.5`).
pub fn format_value(v: f64, decimals: u32) -> String {
    let v = finite(v);
    let d = decimals.min(6) as usize;
    let s = format!("{:.*}", d, v.abs());
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s.as_str(), None),
    };
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    if v < 0.0 && out.chars().any(|c| c.is_ascii_digit() && c != '0') { format!("-{out}") } else { out }
}

/// A "nice" axis step (1, 2 or 5 × 10ⁿ) giving about `ticks` divisions up to `max`.
pub fn nice_step(max: f64, ticks: u32) -> f64 {
    let max = finite(max).abs();
    if max <= 0.0 {
        return 1.0;
    }
    let raw = max / f64::from(ticks.max(1));
    let mag = 10f64.powf(raw.log10().floor());
    let n = raw / mag;
    let k = if n <= 1.0 {
        1.0
    } else if n <= 2.0 {
        2.0
    } else if n <= 5.0 {
        5.0
    } else {
        10.0
    };
    k * mag
}

fn smoothstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn ease_out_cubic(x: f64) -> f64 {
    let x = 1.0 - x.clamp(0.0, 1.0);
    1.0 - x * x * x
}

/// A chart at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartState {
    /// Position in the steps: 0 at the first, `steps - 1` at the last (fractional between).
    pub step_pos: f64,
    /// The step shown as the big label (the nearest one).
    pub step: usize,
    /// Intro growth, 0 → 1.
    pub grow: f64,
    /// Each category's value now (interpolated between steps, times [`ChartState::grow`]).
    pub values: Vec<f64>,
    /// Each category's place in a race (0 = top), gliding between steps.
    pub ranks: Vec<f64>,
}

/// Ranks of the categories at `step`: largest value first, ties in table order.
fn ranks_at(spec: &ChartSpec, step: usize) -> Vec<f64> {
    let n = spec.categories.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| spec.value(b, step).total_cmp(&spec.value(a, step)).then(a.cmp(&b)));
    let mut r = vec![0.0; n];
    for (place, &c) in order.iter().enumerate() {
        if let Some(x) = r.get_mut(c) {
            *x = place as f64;
        }
    }
    r
}

/// The chart at `u` seconds into a clip `d` seconds long.
pub fn state_at(spec: &ChartSpec, u: f64, d: f64) -> ChartState {
    let (u, d) = (finite(u).max(0.0), finite(d).max(0.0));
    let n = spec.steps.len().max(1);
    let intro = finite(spec.intro_seconds).clamp(0.0, 60.0).min(d);
    let hold = finite(spec.hold_seconds).clamp(0.0, 60.0).min((d - intro).max(0.0));
    let grow = if intro > 0.0 { ease_out_cubic(u / intro) } else { 1.0 };
    let span = d - intro - hold;
    let frac = if span > 1e-9 {
        ((u - intro) / span).clamp(0.0, 1.0)
    } else if u >= intro {
        1.0
    } else {
        0.0
    };
    let step_pos = frac * (n - 1) as f64;
    let i0 = (step_pos.floor() as usize).min(n - 1);
    let i1 = (i0 + 1).min(n - 1);
    let f = step_pos - i0 as f64;
    let values: Vec<f64> = (0..spec.categories.len()).map(|c| (spec.value(c, i0) * (1.0 - f) + spec.value(c, i1) * f) * grow).collect();
    let ranks = if spec.kind == ChartKind::BarRace {
        let (r0, r1) = (ranks_at(spec, i0), ranks_at(spec, i1));
        let e = smoothstep(f);
        r0.iter().zip(&r1).map(|(a, b)| a + (b - a) * e).collect()
    } else {
        (0..spec.categories.len()).map(|i| i as f64).collect()
    };
    ChartState { step_pos, step: (step_pos.round() as usize).min(n - 1), grow, values, ranks }
}

#[cfg(test)]
mod tests;
