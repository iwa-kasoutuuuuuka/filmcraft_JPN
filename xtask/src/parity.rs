//! `cargo xtask parity`: a measured parity report against Premiere 26.
//!
//! What it measures, from data in the repository:
//! - **Presence**: every item of Premiere's Video Effects (and Legacy) bin, Audio Effects and
//!   Video Transitions bin, looked up in our effect registry.
//! - **GPU**: which present video effects run on the GPU effect stage (`filmcraft_render::gpufx`).
//! - **Fidelity**: exact / approximate / stub from `xtask/parity/fidelity.txt`; anything not listed
//!   there is "unclassified", so the report never claims more than someone has written down.
//! - **Commands** (`--commands`): how many engine commands exist, sit in a menu, have a shortcut;
//!   with a Premiere menu listing (`plan/premiere/menus.txt` or `--menus PATH`, one
//!   `File > New > Project…` path per line) also which of Premiere's menu items we have.
//!
//! Writes `target/parity/parity.{md,json}` and prints the Markdown. `--check` fails on a missing
//! Premiere item or a broken fidelity file (run by `cargo xtask ci`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use filmcraft_project::effect::{PREMIERE_26_LEGACY_VIDEO_EFFECTS, PREMIERE_26_VIDEO_EFFECTS, PREMIERE_AUDIO_EFFECTS, find_effect_by_name};
use filmcraft_project::vtransition::{PREMIERE_26_TRANSITION_COUNTS, video_transition_tree};
use filmcraft_project::{EffectKind, find_effect};
use filmcraft_render::gpufx::GPU_EFFECTS;
use serde_json::{Value, json};

const FIDELITY_FILE: &str = "xtask/parity/fidelity.txt";
const MENUS_FILE: &str = "plan/premiere/menus.txt";
const LEVELS: &[&str] = &["exact", "approximate", "stub"];

/// One Premiere item and what we have for it.
struct Item {
    area: &'static str,
    folder: String,
    name: String,
    id: Option<&'static str>,
    /// `None` where the GPU column does not apply (audio, transitions).
    gpu: Option<bool>,
}

struct Fidelity {
    level: String,
    reason: String,
}

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let check = args.contains(&"--check");
    let with_commands = args.contains(&"--commands") || args.contains(&"--menus");
    let menus_arg = args.iter().position(|a| *a == "--menus").and_then(|i| args.get(i + 1)).map(|p| root.join(p));
    if let Some(bad) = args.iter().find(|a| a.starts_with("--") && !["--check", "--commands", "--menus"].contains(a)) {
        return Err(format!("parity: unknown option {bad} (usage: cargo xtask parity [--check] [--commands] [--menus PATH])"));
    }

    let mut errors = Vec::new();
    let fidelity = read_fidelity(&root.join(FIDELITY_FILE), &mut errors);
    for id in GPU_EFFECTS {
        if find_effect(id).is_none() {
            errors.push(format!("gpufx::GPU_EFFECTS: unknown effect id `{id}`"));
        }
    }
    let items = collect_items(&mut errors);

    let commands = if with_commands {
        let menus = menus_arg.unwrap_or_else(|| root.join(MENUS_FILE));
        Some(commands_report(root, &menus)?)
    } else {
        None
    };

    let md = markdown(&items, &fidelity, commands.as_ref(), &errors);
    let report = json_report(&items, &fidelity, commands.as_ref(), &errors);
    let dir = root.join("target").join("parity");
    std::fs::create_dir_all(&dir).map_err(|e| format!("parity: {}: {e}", dir.display()))?;
    std::fs::write(dir.join("parity.md"), &md).map_err(|e| format!("parity: write parity.md: {e}"))?;
    let pretty = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("parity.json"), pretty).map_err(|e| format!("parity: write parity.json: {e}"))?;
    println!("{md}");
    println!("written: {}", dir.join("parity.{md,json}").display());

    if check && !errors.is_empty() {
        return Err(format!("parity --check: {} problem(s)\n  {}", errors.len(), errors.join("\n  ")));
    }
    Ok(())
}

/// Parse the fidelity file; problems go to `errors` (the report still runs).
fn read_fidelity(path: &Path, errors: &mut Vec<String>) -> BTreeMap<String, Fidelity> {
    let mut map = BTreeMap::new();
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            errors.push(format!("{FIDELITY_FILE}: {e}"));
            return map;
        }
    };
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(id), Some(level)) = (parts.next(), parts.next()) else {
            errors.push(format!("{FIDELITY_FILE}:{}: expected `<id> <level> <reason>`", n + 1));
            continue;
        };
        let reason = parts.collect::<Vec<_>>().join(" ");
        if !LEVELS.contains(&level) {
            errors.push(format!("{FIDELITY_FILE}:{}: unknown level `{level}` (one of {})", n + 1, LEVELS.join(", ")));
        }
        if find_effect(id).is_none() {
            errors.push(format!("{FIDELITY_FILE}:{}: unknown effect id `{id}`", n + 1));
        }
        if reason.is_empty() {
            errors.push(format!("{FIDELITY_FILE}:{}: `{id}` needs a reason", n + 1));
        }
        if map.insert(id.to_string(), Fidelity { level: level.to_string(), reason }).is_some() {
            errors.push(format!("{FIDELITY_FILE}:{}: `{id}` listed twice", n + 1));
        }
    }
    map
}

fn video_item(area: &'static str, folder: &str, name: &str) -> Item {
    let def = find_effect_by_name(name).filter(|d| d.kind == EffectKind::Video);
    Item { area, folder: folder.to_string(), name: name.to_string(), id: def.map(|d| d.id), gpu: Some(def.is_some_and(|d| GPU_EFFECTS.contains(&d.id))) }
}

fn collect_items(errors: &mut Vec<String>) -> Vec<Item> {
    let mut items = Vec::new();
    for (folder, names) in PREMIERE_26_VIDEO_EFFECTS {
        for name in *names {
            items.push(video_item("Video effects", folder, name));
        }
    }
    for name in PREMIERE_26_LEGACY_VIDEO_EFFECTS {
        items.push(video_item("Legacy video effects", "Legacy", name));
    }
    for id in PREMIERE_AUDIO_EFFECTS {
        let def = find_effect(id).filter(|d| d.kind == EffectKind::Audio);
        items.push(Item {
            area: "Audio effects",
            folder: def.and_then(|d| d.category.get(1)).map_or_else(String::new, |f| (*f).to_string()),
            name: def.map_or_else(|| (*id).to_string(), |d| d.name.to_string()),
            id: def.map(|d| d.id),
            gpu: None,
        });
    }
    // Transitions: Premiere's list is per-folder counts, so each folder's items are ours, padded
    // with "missing" placeholders up to Premiere's count.
    let tree = video_transition_tree();
    for (folder, want) in PREMIERE_26_TRANSITION_COUNTS {
        let ours = tree.iter().find(|(f, _)| f == folder).map(|(_, v)| v.as_slice()).unwrap_or_default();
        if ours.len() > *want {
            errors.push(format!("transitions: {folder} has {} items, Premiere has {want}", ours.len()));
        }
        for i in 0..*want {
            let id = ours.get(i).copied();
            items.push(Item {
                area: "Video transitions",
                folder: (*folder).to_string(),
                name: id.and_then(find_effect).map_or_else(|| "(missing)".to_string(), |d| d.name.to_string()),
                id,
                gpu: None,
            });
        }
    }
    for it in items.iter().filter(|it| it.id.is_none()) {
        errors.push(format!("missing: {} / {} / {}", it.area, it.folder, it.name));
    }
    items
}

#[derive(Default)]
struct Tally {
    total: usize,
    present: usize,
    gpu: Option<usize>,
    /// Present items the GPU column applies to (video effects), the denominator for `gpu`.
    gpu_base: usize,
    levels: BTreeMap<String, usize>,
}

fn tally<'a>(items: impl Iterator<Item = &'a Item>, fidelity: &BTreeMap<String, Fidelity>) -> Tally {
    let mut t = Tally::default();
    for it in items {
        t.total += 1;
        if let Some(g) = it.gpu {
            *t.gpu.get_or_insert(0) += usize::from(g && it.id.is_some());
            t.gpu_base += usize::from(it.id.is_some());
        }
        let Some(id) = it.id else { continue };
        t.present += 1;
        let level = fidelity.get(id).map_or("unclassified", |f| f.level.as_str());
        *t.levels.entry(level.to_string()).or_default() += 1;
    }
    t
}

fn areas(items: &[Item]) -> Vec<&'static str> {
    let mut seen = Vec::new();
    for it in items {
        if !seen.contains(&it.area) {
            seen.push(it.area);
        }
    }
    seen
}

fn pct(n: usize, d: usize) -> String {
    if d == 0 { "–".into() } else { format!("{:.0}%", n as f64 * 100.0 / d as f64) }
}

fn markdown(items: &[Item], fidelity: &BTreeMap<String, Fidelity>, commands: Option<&Value>, errors: &[String]) -> String {
    let mut s = String::from("# Parity with Premiere 26\n\n");
    s.push_str("Presence is looked up in the effect registry; GPU is the GPU effect stage; fidelity comes from\n");
    s.push_str(&format!("`{FIDELITY_FILE}` (anything not listed there is unclassified).\n\n"));
    s.push_str("| Area | Premiere | Present | GPU | Exact | Approximate | Stub | Unclassified |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---:|---:|\n");
    let lv = |t: &Tally, k: &str| t.levels.get(k).copied().unwrap_or(0);
    let row = |s: &mut String, name: &str, t: &Tally| {
        let gpu = t.gpu.map_or_else(|| "–".to_string(), |g| format!("{g} ({})", pct(g, t.gpu_base)));
        s.push_str(&format!(
            "| {name} | {} | {} ({}) | {gpu} | {} | {} | {} | {} |\n",
            t.total,
            t.present,
            pct(t.present, t.total),
            lv(t, "exact"),
            lv(t, "approximate"),
            lv(t, "stub"),
            lv(t, "unclassified"),
        ));
    };
    for area in areas(items) {
        row(&mut s, area, &tally(items.iter().filter(|it| it.area == area), fidelity));
    }
    row(&mut s, "**All**", &tally(items.iter(), fidelity));

    if let Some(c) = commands {
        s.push_str("\n## Commands\n\n");
        s.push_str(&format!("{} commands; {} in a menu, {} with a default shortcut.\n", c["total"], c["in_menu"], c["with_shortcut"]));
        match c.get("menus") {
            Some(m) if !m.is_null() => {
                s.push_str(&format!("\nPremiere menu items: {} of {} found ({}).\n", m["found"], m["total"], m["percent"].as_str().unwrap_or("–")));
                if let Some(missing) = m["missing"].as_array().filter(|v| !v.is_empty()) {
                    s.push_str("\nMissing (first 40):\n\n");
                    for p in missing.iter().take(40).filter_map(Value::as_str) {
                        s.push_str(&format!("- {p}\n"));
                    }
                }
            }
            _ => s.push_str(&format!("\nNo Premiere menu listing (`{MENUS_FILE}` or `--menus PATH`), so menus are not compared.\n")),
        }
    }

    let missing: Vec<&Item> = items.iter().filter(|it| it.id.is_none()).collect();
    if !missing.is_empty() {
        s.push_str("\n## Missing\n\n");
        for it in missing {
            s.push_str(&format!("- {} / {} / {}\n", it.area, it.folder, it.name));
        }
    }
    let marked: Vec<(&String, &Fidelity)> = fidelity.iter().filter(|(_, f)| f.level != "exact").collect();
    if !marked.is_empty() {
        s.push_str("\n## Known approximations and stubs\n\n");
        for (id, f) in marked {
            s.push_str(&format!("- `{id}` ({}): {}\n", f.level, f.reason));
        }
    }
    if !errors.is_empty() {
        s.push_str("\n## Problems\n\n");
        for e in errors {
            s.push_str(&format!("- {e}\n"));
        }
    }
    s
}

fn json_report(items: &[Item], fidelity: &BTreeMap<String, Fidelity>, commands: Option<&Value>, errors: &[String]) -> Value {
    let summary: Vec<Value> = areas(items)
        .into_iter()
        .map(|area| {
            let t = tally(items.iter().filter(|it| it.area == area), fidelity);
            json!({"area": area, "premiere": t.total, "present": t.present, "gpu": t.gpu, "fidelity": t.levels})
        })
        .collect();
    let list: Vec<Value> = items
        .iter()
        .map(|it| {
            let f = it.id.and_then(|id| fidelity.get(id));
            json!({
                "area": it.area, "folder": it.folder, "name": it.name, "id": it.id, "present": it.id.is_some(),
                "gpu": it.gpu, "fidelity": f.map_or("unclassified", |f| f.level.as_str()), "reason": f.map(|f| f.reason.as_str()),
            })
        })
        .collect();
    json!({"premiere": "26", "summary": summary, "items": list, "commands": commands, "problems": errors})
}

/// Normalise a menu path for comparison: lowercase, ` > ` separators, no trailing ellipsis.
fn norm_path(p: &str) -> String {
    p.split('>').map(|seg| seg.trim().trim_end_matches('…').trim_end_matches("...").trim().to_lowercase()).collect::<Vec<_>>().join(" > ")
}

/// Engine commands from `filmcraft-cli commands --json`, and the Premiere menu comparison when a
/// listing exists.
fn commands_report(root: &Path, menus: &Path) -> Result<Value, String> {
    let out = Command::new(env!("CARGO"))
        .current_dir(root)
        .args(["run", "--quiet", "--release", "-p", "filmcraft-cli", "--", "commands", "--json"])
        .output()
        .map_err(|e| format!("parity: run filmcraft-cli: {e}"))?;
    if !out.status.success() {
        return Err(format!("parity: filmcraft-cli commands failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    let list: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("parity: commands JSON: {e}"))?;
    let list = list.as_array().cloned().unwrap_or_default();
    let menu_of = |c: &Value| -> Option<String> {
        let menu: Vec<&str> = c["menu"].as_array()?.iter().filter_map(Value::as_str).collect();
        if menu.is_empty() {
            return None;
        }
        Some(norm_path(&format!("{} > {}", menu.join(" > "), c["label"].as_str().unwrap_or_default())))
    };
    let ours: BTreeSet<String> = list.iter().filter_map(menu_of).collect();
    let with_shortcut = list.iter().filter(|c| c["defaultShortcut"].as_str().is_some_and(|s| !s.is_empty())).count();

    let menus_report = match std::fs::read_to_string(menus) {
        Ok(text) => {
            let wanted: BTreeSet<String> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(norm_path).collect();
            let missing: Vec<&String> = wanted.iter().filter(|p| !ours.contains(*p)).collect();
            let found = wanted.len().saturating_sub(missing.len());
            json!({"source": menus.display().to_string(), "total": wanted.len(), "found": found, "percent": pct(found, wanted.len()), "missing": missing})
        }
        Err(_) => Value::Null,
    };
    Ok(json!({"total": list.len(), "in_menu": ours.len(), "with_shortcut": with_shortcut, "menus": menus_report}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_paths_compare_without_case_or_ellipsis() {
        assert_eq!(norm_path("File > New > Project…"), "file > new > project");
        assert_eq!(norm_path("file>new>Project..."), "file > new > project");
    }

    #[test]
    fn the_checked_in_fidelity_file_is_valid_and_every_premiere_item_is_present() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().map(Path::to_path_buf).unwrap_or_default();
        let mut errors = Vec::new();
        let fidelity = read_fidelity(&root.join(FIDELITY_FILE), &mut errors);
        let items = collect_items(&mut errors);
        assert!(errors.is_empty(), "{errors:#?}");
        assert!(fidelity.values().all(|f| LEVELS.contains(&f.level.as_str())));
        let t = tally(items.iter().filter(|it| it.area == "Video effects"), &fidelity);
        assert_eq!((t.total, t.present), (93, 93));
        assert!(t.gpu.unwrap_or(0) > 0);
    }

    #[test]
    fn a_broken_fidelity_file_is_reported() {
        let dir = std::env::temp_dir().join(format!("filmcraft-parity-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("fidelity.txt");
        std::fs::write(&p, "gaussian_blur approximate x\ngaussian_blur exact y\nno_such_effect stub z\ncrop sorta w\nlevels exact\n").unwrap();
        let mut errors = Vec::new();
        read_fidelity(&p, &mut errors);
        let all = errors.join("\n");
        assert!(all.contains("listed twice"), "{all}");
        assert!(all.contains("unknown effect id `no_such_effect`"), "{all}");
        assert!(all.contains("unknown level `sorta`"), "{all}");
        assert!(all.contains("needs a reason"), "{all}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
