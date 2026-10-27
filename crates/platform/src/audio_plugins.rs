//! Audio plug-in hosting: finding VST3 plug-ins on disk and registering the VST3 host
//! ([`register_audio_plugins`]) with the plug-in catalogue of `filmcraft_audio_dsp::plugin`.
//!
//! This module has no `unsafe`: it is the scanner's file-system logic (folders, bundles, which
//! binary to load), the class-id text form and the state-blob container, all tested on every
//! platform. The FFI (loading the plug-in, its COM interfaces) is `vst3host` (Windows only for
//! now; docs/adr/0002-audio-plugin-hosting.md).

use std::path::{Path, PathBuf};

/// What [`register_audio_plugins`] made available.
pub use crate::Availability;

/// Register this system's audio plug-in hosts (VST3 on Windows) with the plug-in catalogue. Call
/// once at startup; repeated calls are harmless. Nothing is loaded until a scan
/// (`audioPlugins.scan`, or a project that uses a plug-in).
pub fn register_audio_plugins() -> Availability {
    #[cfg(windows)]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| filmcraft_audio_dsp::plugin::register_host(std::sync::Arc::new(crate::vst3host::Vst3Host::standard())));
        Availability::Available("VST3")
    }
    #[cfg(not(windows))]
    {
        Availability::Unavailable("no audio plug-in host for this system yet")
    }
}

/// Extra VST3 folders, separated like `PATH` (tests, portable setups).
pub const VST3_PATH_ENV: &str = "FILMCRAFT_VST3_PATH";

/// The folders searched for VST3 plug-ins: `$FILMCRAFT_VST3_PATH` first, then the system's
/// standard locations (Windows: `%CommonProgramFiles%\VST3` and
/// `%LOCALAPPDATA%\Programs\Common\VST3`; macOS: `/Library/Audio/Plug-Ins/VST3` and the user's;
/// Linux: `~/.vst3`, `/usr/lib/vst3`, `/usr/local/lib/vst3`). Folders that do not exist are left out.
pub fn vst3_folders() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let mut v: Vec<PathBuf> = std::env::var_os(VST3_PATH_ENV).map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if cfg!(windows) {
        v.extend(env("CommonProgramFiles").map(|d| d.join("VST3")));
        v.extend(env("LOCALAPPDATA").map(|d| d.join("Programs").join("Common").join("VST3")));
    } else if cfg!(target_os = "macos") {
        v.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        v.extend(env("HOME").map(|d| d.join("Library/Audio/Plug-Ins/VST3")));
    } else {
        v.extend(env("HOME").map(|d| d.join(".vst3")));
        v.push(PathBuf::from("/usr/lib/vst3"));
        v.push(PathBuf::from("/usr/local/lib/vst3"));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for d in v {
        if d.is_dir() && !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// Deepest folder nesting searched below a VST3 folder (vendor sub-folders).
const MAX_DEPTH: usize = 6;
/// Most directory entries looked at per folder tree (a huge folder cannot stall a scan forever).
const MAX_ENTRIES: usize = 50_000;
/// Most plug-ins taken from one scan.
pub const MAX_BUNDLES: usize = 2_000;

fn is_vst3_name(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("vst3"))
}

/// The plug-ins below `folder`: `.vst3` bundles (directories, not searched further) and
/// single-file `.vst3` modules, sorted. Unreadable folders are skipped.
pub fn find_vst3_bundles(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut seen = 0usize;
    let mut stack = vec![(folder.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES || found.len() >= MAX_BUNDLES {
                found.sort();
                return found;
            }
            let p = e.path();
            // symlinks are followed for the entry itself but never descended into (no cycles)
            let Ok(ft) = e.file_type() else { continue };
            if is_vst3_name(&p) {
                if p.is_dir() || p.is_file() {
                    found.push(p);
                }
            } else if ft.is_dir() && depth < MAX_DEPTH {
                stack.push((p, depth + 1));
            }
        }
    }
    found.sort();
    found
}

/// The `Contents/<arch>` folder names a bundle may hold this process's binary in.
fn arch_dirs() -> &'static [&'static str] {
    if cfg!(windows) {
        if cfg!(target_arch = "x86_64") {
            &["x86_64-win"]
        } else if cfg!(target_arch = "aarch64") {
            &["arm64-win", "arm64x-win", "arm64ec-win"]
        } else {
            &["x86-win"]
        }
    } else if cfg!(target_os = "macos") {
        &["MacOS"]
    } else if cfg!(target_arch = "aarch64") {
        &["aarch64-linux"]
    } else {
        &["x86_64-linux"]
    }
}

/// The loadable module of a plug-in: a single-file module is itself; a bundle
/// `X.vst3/Contents/<arch>/X.vst3` (Windows), `X.vst3/Contents/MacOS/X` (macOS) or
/// `X.vst3/Contents/<arch>-linux/X.so` (Linux), or else the one module in that folder.
pub fn vst3_binary(bundle: &Path) -> Option<PathBuf> {
    if bundle.is_file() {
        return Some(bundle.to_path_buf());
    }
    let stem = bundle.file_stem()?.to_os_string();
    let contents = bundle.join("Contents");
    for arch in arch_dirs() {
        let dir = contents.join(arch);
        let mut name = stem.clone();
        if cfg!(windows) {
            name.push(".vst3");
        } else if !cfg!(target_os = "macos") {
            name.push(".so");
        }
        let exact = dir.join(&name);
        if exact.is_file() {
            return Some(exact);
        }
        // a binary renamed inside its bundle: the only module there
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mods: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && (if cfg!(windows) {
                        is_vst3_name(p)
                    } else if cfg!(target_os = "macos") {
                        true
                    } else {
                        p.extension().is_some_and(|e| e == "so")
                    })
            })
            .take(2)
            .collect();
        if let [one] = mods.as_slice() {
            return Some(one.clone());
        }
    }
    None
}

/// The text form of a VST3 class id: 32 uppercase hex digits, the same on every OS (the SDK's
/// `FUID::toString`). On Windows class ids are stored in COM `GUID` byte order (`com_layout`),
/// elsewhere as four big-endian 32-bit words.
pub fn uid_string(tuid: [u8; 16], com_layout: bool) -> String {
    let b = tuid;
    let words: [u32; 4] = if com_layout {
        let d1 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let d2 = u16::from_le_bytes([b[4], b[5]]) as u32;
        let d3 = u16::from_le_bytes([b[6], b[7]]) as u32;
        [d1, (d2 << 16) | d3, u32::from_be_bytes([b[8], b[9], b[10], b[11]]), u32::from_be_bytes([b[12], b[13], b[14], b[15]])]
    } else {
        [
            u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            u32::from_be_bytes([b[4], b[5], b[6], b[7]]),
            u32::from_be_bytes([b[8], b[9], b[10], b[11]]),
            u32::from_be_bytes([b[12], b[13], b[14], b[15]]),
        ]
    };
    words.iter().map(|w| format!("{w:08X}")).collect()
}

/// Magic of the host state container (component state + controller state).
const STATE_MAGIC: &[u8; 8] = b"FCVST3S1";
/// Largest state accepted from a project or a plug-in (each part).
pub const MAX_STATE_PART: usize = 32 << 20;

/// Pack a plug-in's component (processor) and controller states into one blob.
pub fn encode_state(component: &[u8], controller: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(16 + component.len() + controller.len());
    v.extend_from_slice(STATE_MAGIC);
    v.extend_from_slice(&(component.len().min(u32::MAX as usize) as u32).to_le_bytes());
    v.extend_from_slice(component);
    v.extend_from_slice(&(controller.len().min(u32::MAX as usize) as u32).to_le_bytes());
    v.extend_from_slice(controller);
    v
}

/// Unpack [`encode_state`]; `None` for anything malformed (truncated, wrong magic, oversized).
pub fn decode_state(blob: &[u8]) -> Option<(&[u8], &[u8])> {
    let rest = blob.strip_prefix(STATE_MAGIC.as_slice())?;
    let (component, rest) = take_part(rest)?;
    let (controller, rest) = take_part(rest)?;
    rest.is_empty().then_some((component, controller))
}

fn take_part(b: &[u8]) -> Option<(&[u8], &[u8])> {
    let len = u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize;
    if len > MAX_STATE_PART {
        return None;
    }
    let end = 4usize.checked_add(len)?;
    Some((b.get(4..end)?, b.get(end..)?))
}

/// A NUL-terminated byte string from a fixed-size C buffer (lossy UTF-8; no NUL = whole buffer).
// Used by the Windows-only VST3 host (`vst3host`); unit-tested on every platform.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn c_string(buf: &[u8]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(buf.get(..end).unwrap_or_default()).trim().to_string()
}

/// A NUL-terminated UTF-16 string from a fixed-size buffer.
// Used by the Windows-only VST3 host (`vst3host`); unit-tested on every platform.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn wide_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(buf.get(..end).unwrap_or_default()).trim().to_string()
}

/// Whether a VST3 class with these sub-categories is an effect (not an instrument): `Fx…`, or
/// no sub-categories at all; an `Instrument` without `Fx` is left out.
// Used by the Windows-only VST3 host (`vst3host`); unit-tested on every platform.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn is_effect_category(sub_categories: &str) -> bool {
    let parts: Vec<&str> = sub_categories.split('|').map(str::trim).collect();
    parts.iter().any(|p| p.eq_ignore_ascii_case("Fx")) || !parts.iter().any(|p| p.eq_ignore_ascii_case("Instrument"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("filmcraft-vst3-scan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn uid_text_matches_the_sdk_on_both_layouts() {
        // uid(0x6E332252, 0x54224A00, 0xAA69301A, 0xF318797D) stored in COM GUID order (Windows)…
        let com = [0x52, 0x22, 0x33, 0x6E, 0x22, 0x54, 0x00, 0x4A, 0xAA, 0x69, 0x30, 0x1A, 0xF3, 0x18, 0x79, 0x7D];
        assert_eq!(uid_string(com, true), "6E33225254224A00AA69301AF318797D");
        // …and as big-endian words (macOS / Linux)
        let be = [0x6E, 0x33, 0x22, 0x52, 0x54, 0x22, 0x4A, 0x00, 0xAA, 0x69, 0x30, 0x1A, 0xF3, 0x18, 0x79, 0x7D];
        assert_eq!(uid_string(be, false), "6E33225254224A00AA69301AF318797D");
    }

    #[test]
    fn state_blobs_round_trip_and_reject_damage() {
        let b = encode_state(b"abc", b"");
        assert_eq!(decode_state(&b), Some((&b"abc"[..], &b""[..])));
        for cut in 0..b.len() {
            assert!(decode_state(&b[..cut]).is_none(), "truncated at {cut}");
        }
        let mut extra = b.clone();
        extra.push(0);
        assert!(decode_state(&extra).is_none());
        let mut huge = STATE_MAGIC.to_vec();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_state(&huge).is_none());
        assert!(decode_state(b"").is_none());
        assert!(decode_state(b"FCVST3S2\0\0\0\0\0\0\0\0").is_none());
    }

    #[test]
    fn strings_from_fixed_buffers() {
        assert_eq!(c_string(b"Gain\0junk"), "Gain");
        assert_eq!(c_string(b"NoNul"), "NoNul");
        assert_eq!(c_string(&[0xFF, 0x41, 0]), "\u{FFFD}A");
        let w: Vec<u16> = "Ré\0x".encode_utf16().collect();
        assert_eq!(wide_string(&w), "Ré");
        assert_eq!(wide_string(&[0xD800, 0]), "\u{FFFD}");
    }

    #[test]
    fn effects_and_instruments() {
        assert!(is_effect_category("Fx|Delay"));
        assert!(is_effect_category("Fx|Instrument"));
        assert!(is_effect_category(""));
        assert!(!is_effect_category("Instrument|Synth"));
    }

    #[test]
    fn scanner_finds_bundles_and_single_files() {
        let root = tmp("find");
        // a bundle with its binary for this platform
        let bundle = root.join("Vendor").join("Echo.vst3");
        let arch = arch_dirs()[0];
        let bin_dir = bundle.join("Contents").join(arch);
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin_name = if cfg!(windows) {
            "Echo.vst3"
        } else if cfg!(target_os = "macos") {
            "Echo"
        } else {
            "Echo.so"
        };
        std::fs::write(bin_dir.join(bin_name), b"not really a dll").unwrap();
        // something inside the bundle that looks like a plug-in must not be found separately
        std::fs::create_dir_all(bundle.join("Contents").join("Resources").join("Inner.vst3")).unwrap();
        // a single-file module, upper-case extension, and noise
        std::fs::write(root.join("Single.VST3"), b"x").unwrap();
        std::fs::write(root.join("readme.txt"), b"x").unwrap();
        // a bundle without a binary for this platform
        std::fs::create_dir_all(root.join("Empty.vst3").join("Contents").join("other-arch")).unwrap();
        // too deep to be searched
        let mut deep = root.clone();
        for i in 0..(MAX_DEPTH + 2) {
            deep = deep.join(format!("d{i}"));
        }
        std::fs::create_dir_all(deep.join("Deep.vst3")).unwrap();

        let found = find_vst3_bundles(&root);
        let names: Vec<String> = found.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&"Echo.vst3".to_string()), "{names:?}");
        assert!(names.contains(&"Single.VST3".to_string()), "{names:?}");
        assert!(names.contains(&"Empty.vst3".to_string()), "{names:?}");
        assert!(!names.contains(&"Inner.vst3".to_string()), "bundles are not searched inside: {names:?}");
        assert!(!names.contains(&"Deep.vst3".to_string()), "depth limit: {names:?}");
        assert_eq!(vst3_binary(&bundle), Some(bin_dir.join(bin_name)));
        assert_eq!(vst3_binary(&root.join("Single.VST3")), Some(root.join("Single.VST3")));
        assert_eq!(vst3_binary(&root.join("Empty.vst3")), None);
        assert_eq!(vst3_binary(&root.join("missing.vst3")), None);
        // a renamed binary is found when it is the only module
        std::fs::rename(
            bin_dir.join(bin_name),
            bin_dir.join(if cfg!(windows) {
                "Renamed.vst3"
            } else if cfg!(target_os = "macos") {
                "Renamed"
            } else {
                "Renamed.so"
            }),
        )
        .unwrap();
        assert!(vst3_binary(&bundle).is_some());
        assert!(find_vst3_bundles(&root.join("does-not-exist")).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn folders_exist_and_are_unique() {
        let f = vst3_folders();
        assert!(f.iter().all(|d| d.is_dir()));
        let mut u = f.clone();
        u.dedup();
        assert_eq!(u.len(), f.len());
    }
}
