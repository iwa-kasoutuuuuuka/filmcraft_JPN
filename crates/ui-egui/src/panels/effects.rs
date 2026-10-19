//! Effects panel: searchable folder tree of presets, audio/video effects and transitions (from the
//! effect definitions). Drag onto a clip or edit point; double-click applies to the selection.
//!
//! Lumetri Presets: each preset row has a thumbnail (our Lumetri on the procedural preview
//! picture, `filmcraft_render::lumetri_presets`); double-click (or right-click ▸ Apply to Selected
//! Clips, `effects.presetMenu.apply`) applies it (`lumetri.applyPreset`).
//! When the panel is wide (maximized), clicking a Lumetri Presets folder shows its presets as a
//! thumbnail grid to the right of the tree. Automation ids: `effects.lumetriPreset.<name>`,
//! `effects.presetGrid` (the grid area), `effects.presetGrid.<name>`.

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use serde_json::json;

use crate::FilmcraftApp;
use crate::icons::{self, Icon};
use crate::theme::Tokens;

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let sr = Rect::from_min_size(rect.min + vec2(8.0, 6.0), vec2(rect.width() - 16.0, 22.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sr));
    let mut q = app.ui.effects_search.clone();
    let sresp = crate::widgets::search_field(&mut child, &mut q, crate::i18n::t("Search effects", "エフェクトを検索"), sr.width(), &t);
    app.auto.add("effects.search", sresp.rect, "Search effects");
    app.ui.effects_search = q.clone();
    let body = Rect::from_min_max(pos2(rect.min.x, sr.max.y + 6.0), rect.max);
    // wide panel: the tree on the left, the Lumetri Presets thumbnail grid on the right
    let grid_area = (body.width() > 640.0).then(|| Rect::from_min_max(pos2(body.min.x + 300.0, body.min.y), body.max));
    let body = match grid_area {
        Some(g) => Rect::from_min_max(body.min, pos2(g.min.x - 4.0, body.max.y)),
        None => body,
    };
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("fx-body"));
    bui.set_clip_rect(body);
    let filter = q.to_ascii_lowercase();
    let defs = filmcraft_project::effect_defs();
    // folder tree: top-level categories in Premiere's order
    let tops = filmcraft_project::vtransition::EFFECT_TOP_FOLDERS;
    let mut apply: Option<String> = None;
    let mut lumetri_apply: Option<String> = None;
    let mut preset_action: Option<(String, String)> = None;
    egui::ScrollArea::vertical().id_salt("fx-scroll").auto_shrink([false, false]).show(&mut bui, |ui| {
        for &top in tops {
            if top == "Presets" {
                let any = filter.is_empty() || app.session.presets.all().iter().any(|p| p.name.to_ascii_lowercase().contains(&filter));
                if any && folder_row(app, ui, top, 0, &filter) {
                    preset_action = crate::panels::presets::folder_rows(app, ui, &filter).or(preset_action.take());
                }
                continue;
            }
            if top == "Lumetri Presets" {
                if let Some(name) = lumetri_rows(app, ui, &filter) {
                    lumetri_apply = Some(name);
                }
                continue;
            }
            let mut items: Vec<_> = defs
                .iter()
                .filter(|d| d.category.first() == Some(&top))
                .filter(|d| filter.is_empty() || d.name.to_ascii_lowercase().contains(&filter))
                .collect();
            // Premiere lists effect folders and effects alphabetically (obsolete ones last)
            if top == "Video Effects" || top == "Legacy" {
                items.sort_by_key(|d| (d.category.get(1) == Some(&"Obsolete"), d.category.get(1).copied().unwrap_or(""), d.name.to_ascii_lowercase()));
            }
            if !filter.is_empty() && items.is_empty() {
                continue;
            }
            let open = folder_row(app, ui, top, 0, &filter);
            if !open {
                continue;
            }
            // Sub-folders in definition order; loose items (no sub-folder, e.g. Audio Effects ▸
            // Balance / Mute / Volume) come last, outside any folder.
            let sub = |d: &filmcraft_project::EffectDef| d.category.get(1).copied().unwrap_or("");
            let mut subs: Vec<&str> = items.iter().map(|d| sub(d)).collect();
            subs.dedup();
            subs.sort_by_key(|s| s.is_empty());
            let mut seen = Vec::new();
            for s in subs {
                if seen.contains(&s) {
                    continue;
                }
                seen.push(s);
                let key = format!("{top}/{s}");
                if !s.is_empty() && !folder_row(app, ui, &key, 1, &filter) {
                    continue;
                }
                for d in items.iter().filter(|d| sub(d) == s) {
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::click_and_drag());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 0.0, t.hover);
                    }
                    let x = r.min.x + 46.0;
                    let icon_r = Rect::from_center_size(pos2(x - 10.0, r.center().y), vec2(13.0, 13.0));
                    let is_tr = matches!(d.kind, filmcraft_project::EffectKind::VideoTransition | filmcraft_project::EffectKind::AudioTransition);
                    ui.painter().rect_stroke(icon_r, 2.0, egui::Stroke::new(1.0, t.text_dim), egui::StrokeKind::Inside);
                    if is_tr {
                        ui.painter().line_segment([icon_r.left_bottom(), icon_r.right_top()], egui::Stroke::new(1.0, t.text_dim));
                    } else {
                        ui.painter().text(icon_r.center(), Align2::CENTER_CENTER, "fx", Tokens::ui(8.0), t.text_dim);
                    }
                    ui.painter().text(pos2(x, r.center().y), Align2::LEFT_CENTER, effect_display_name(d.id, d.name), Tokens::ui(12.0), t.text);
                    // badges: accelerated / 32-bit / YUV
                    let mut bx = r.max.x - 8.0;
                    for (on, label) in [(d.yuv, "YUV"), (d.float32, "32"), (d.accelerated, "⚡")] {
                        if on {
                            let br = Rect::from_min_size(pos2(bx - 22.0, r.min.y + 3.0), vec2(20.0, 14.0));
                            ui.painter().rect_filled(br, 2.0, Color32::from_rgb(48, 48, 48));
                            ui.painter().text(br.center(), Align2::CENTER_CENTER, label, Tokens::ui(8.5), t.text_dim);
                            bx -= 24.0;
                        }
                    }
                    app.auto.add(&format!("effects.item.{}", d.id), r, d.name);
                    if resp.drag_started() {
                        crate::panels::start_drag_effect(ui, d.id);
                    }
                    if resp.double_clicked() {
                        apply = Some(d.id.to_string());
                    }
                }
            }
        }
    });
    if let Some(g) = grid_area
        && let Some(name) = preset_grid(app, ui, g)
    {
        lumetri_apply = Some(name);
    }
    if let Some(name) = lumetri_apply
        && let Err(e) = app.session.execute("lumetri.applyPreset", json!({"name": name}))
    {
        app.ui.status = e.to_string();
    }
    if let Some((cmd, name)) = preset_action {
        crate::panels::presets::run(app, &cmd, &name);
    }
    if let Some(id) = apply {
        let r = app.session.execute("effects.apply", json!({"effect": id}));
        if let Err(e) = r {
            app.ui.status = e.to_string();
        }
    }
}

fn folder_row(app: &mut FilmcraftApp, ui: &mut egui::Ui, key: &str, depth: usize, filter: &str) -> bool {
    let t = app.tokens;
    let open = !filter.is_empty() || app.ui.expanded_fx.iter().any(|k| k == key);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 0.0, t.hover);
    }
    let x = r.min.x + 8.0 + depth as f32 * 16.0;
    icons::paint(
        ui.painter(),
        Rect::from_center_size(pos2(x + 4.0, r.center().y), vec2(10.0, 10.0)),
        if open { Icon::ChevronDown } else { Icon::ChevronRight },
        t.text_dim,
    );
    icons::paint(ui.painter(), Rect::from_center_size(pos2(x + 18.0, r.center().y), vec2(13.0, 13.0)), Icon::Folder, t.text_dim);
    let name = key.rsplit('/').next().unwrap_or(key);
    let display = folder_display_name(name);
    ui.painter().text(pos2(x + 30.0, r.center().y), Align2::LEFT_CENTER, display, Tokens::ui(12.0), t.text);
    app.auto.add(&format!("effects.folder.{key}"), r, display);
    if resp.clicked() {
        if open {
            app.ui.expanded_fx.retain(|k| k != key);
        } else {
            app.ui.expanded_fx.push(key.to_string());
        }
    }
    open
}

/// Right-click menu of a Lumetri preset: Apply to Selected Clips (automation id
/// `effects.presetMenu.apply`). True when chosen.
fn apply_menu(app: &mut FilmcraftApp, resp: &egui::Response) -> bool {
    let mut chosen = false;
    resp.context_menu(|ui| {
        let b = ui.button(crate::i18n::t("Apply to Selected Clips", "選択したクリップに適用"));
        app.auto.add("effects.presetMenu.apply", b.rect, "Apply to Selected Clips");
        if b.clicked() {
            chosen = true;
            ui.close();
        }
    });
    chosen
}

/// A preset's thumbnail texture (rendered once per size, cached by name).
fn preset_texture(app: &mut FilmcraftApp, ctx: &egui::Context, p: &filmcraft_render::lumetri_presets::LumetriPreset, w: usize) -> egui::TextureId {
    let name = format!("lumetri-preset-{}-{w}", p.name);
    if let Some((id, _)) = app.texture_existing(&name) {
        return id;
    }
    let h = (w * 9 / 16).max(1);
    let img = filmcraft_render::lumetri_presets::thumbnail(p, w, h);
    let rgba = crate::frames::Rgba { w: img.w, h: img.h, px: img.over_black_rgba8() };
    let key = crate::frames::FrameKey {
        target: crate::frames::Target::Item(filmcraft_project::ItemId(u64::MAX)),
        frame: 0,
        size: w as u32,
        revision: 0,
        draft: false,
    };
    app.texture_for(ctx, &name, key, &rgba)
}

/// The Lumetri Presets folder of the tree: sub-folders and preset rows with thumbnails. Returns
/// the preset to apply (double-click).
fn lumetri_rows(app: &mut FilmcraftApp, ui: &mut egui::Ui, filter: &str) -> Option<String> {
    use filmcraft_render::lumetri_presets as lp;
    let t = app.tokens;
    let top = "Lumetri Presets";
    let presets: Vec<lp::LumetriPreset> = lp::presets().into_iter().filter(|p| filter.is_empty() || p.name.to_ascii_lowercase().contains(filter)).collect();
    if !filter.is_empty() && presets.is_empty() {
        return None;
    }
    if !folder_row(app, ui, top, 0, filter) {
        return None;
    }
    let mut apply = None;
    for folder in lp::FOLDERS {
        let items: Vec<&lp::LumetriPreset> = presets.iter().filter(|p| p.folder == folder).collect();
        if items.is_empty() {
            continue;
        }
        let key = format!("{top}/{folder}");
        let was_open = !filter.is_empty() || app.ui.expanded_fx.contains(&key);
        let open = folder_row(app, ui, &key, 1, filter);
        // the folder clicked last is the one the wide panel shows as a grid (`folder_row` toggles
        // the expanded state on a click and returns the state it drew)
        let now = !filter.is_empty() || app.ui.expanded_fx.contains(&key);
        if now != was_open {
            app.ui.lumetri_grid_folder = Some(folder.to_string());
        }
        if !open {
            continue;
        }
        for p in items {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, 0.0, t.hover);
            }
            let tex = preset_texture(app, ui.ctx(), p, 64);
            let tr = Rect::from_min_size(pos2(r.min.x + 40.0, r.min.y + 3.0), vec2(32.0, 18.0));
            ui.painter().image(tex, tr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            ui.painter().text(pos2(tr.max.x + 8.0, r.center().y), Align2::LEFT_CENTER, p.name, Tokens::ui(12.0), t.text);
            app.auto.add(&format!("effects.lumetriPreset.{}", p.name), r, p.name);
            let resp = resp.on_hover_text(p.description);
            if apply_menu(app, &resp) {
                apply = Some(p.name.to_string());
            }
            if resp.double_clicked() {
                apply = Some(p.name.to_string());
            }
        }
    }
    apply
}

/// The wide Effects panel's thumbnail grid of the selected Lumetri Presets folder. Returns the
/// preset to apply (double-click).
fn preset_grid(app: &mut FilmcraftApp, ui: &mut egui::Ui, area: Rect) -> Option<String> {
    use filmcraft_render::lumetri_presets as lp;
    let t = app.tokens;
    let folder = app.ui.lumetri_grid_folder.clone()?;
    app.auto.add("effects.presetGrid", area, &folder);
    let presets: Vec<lp::LumetriPreset> = lp::presets().into_iter().filter(|p| p.folder == folder).collect();
    let (cw, ch, gap, label_h) = (150.0f32, 84.0f32, 6.0f32, 18.0f32);
    let cols = (((area.width() - gap) / (cw + gap)).floor() as usize).max(1);
    let mut apply = None;
    for (k, p) in presets.iter().enumerate() {
        let x = area.min.x + gap + (k % cols) as f32 * (cw + gap);
        let y = area.min.y + gap + (k / cols) as f32 * (ch + label_h + gap);
        let cell = Rect::from_min_size(pos2(x, y), vec2(cw, ch + label_h));
        if cell.max.y > area.max.y {
            break;
        }
        let pic = Rect::from_min_size(cell.min, vec2(cw, ch));
        let tex = preset_texture(app, ui.ctx(), p, 160);
        let resp = ui.interact(cell, egui::Id::new(("lumetri-grid", p.name)), Sense::click()).on_hover_text(p.description);
        ui.painter().rect_filled(cell, 3.0, if resp.hovered() { t.hover } else { t.field_bg });
        ui.painter().image(tex, pic.shrink(2.0), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let label = ui.painter().layout(p.name.to_string(), Tokens::ui(11.0), t.text, cw - 6.0);
        ui.painter().galley(pos2(cell.min.x + 4.0, pic.max.y + 2.0), label, t.text);
        app.auto.add(&format!("effects.presetGrid.{}", p.name), cell, p.name);
        if apply_menu(app, &resp) {
            apply = Some(p.name.to_string());
        }
        if resp.double_clicked() {
            apply = Some(p.name.to_string());
        }
    }
    apply
}

/// The folder name shown in the Effects tree (Japanese when the UI is Japanese).
fn folder_display_name(name: &str) -> &str {
    if !crate::i18n::is_japanese() {
        return name;
    }
    let Some(ja) = folder_ja(name) else {
        return name;
    };
    crate::i18n::remember(name, ja);
    ja
}

/// Japanese name of an Effects-panel folder or sub-folder (top-level categories, effect /
/// transition sub-folders, Lumetri Presets folders).
fn folder_ja(name: &str) -> Option<&'static str> {
    Some(match name {
        // top level
        "Presets" => "プリセット",
        "Lumetri Presets" => "Lumetri プリセット",
        "Audio Effects" => "オーディオエフェクト",
        "Audio Transitions" => "オーディオトランジション",
        "Video Effects" => "ビデオエフェクト",
        "Video Transitions" => "ビデオトランジション",
        "Legacy" => "レガシー",
        "Obsolete" => "旧バージョン",
        // video effects
        "Adjust" => "色調補正",
        "Blur & Sharpen" => "ブラー＆シャープ",
        "Color Correction" => "カラー補正",
        "Distort" => "ディストーション",
        "Generate" => "描画",
        "Image Control" => "イメージコントロール",
        "Immersive Video" => "イマーシブビデオ",
        "Keying" => "キーイング",
        "Lights & Glows" => "ライト＆グロー",
        "Noise & Grain" => "ノイズ＆グレイン",
        "Perspective" => "遠近",
        "Stylize" => "スタイライズ",
        "Time" => "時間",
        "Transform" => "トランスフォーム",
        "Transition" => "トランジション",
        "Utility" => "ユーティリティ",
        "Video" => "ビデオ",
        // audio effects / transitions
        "Amplitude and Compression" => "振幅と圧縮",
        "Delay and Echo" => "ディレイとエコー",
        "Filter and EQ" => "フィルターと EQ",
        "Modulation" => "モジュレーション",
        "Noise Reduction/Restoration" => "ノイズリダクション / レストレーション",
        "Reverb" => "リバーブ",
        "Special" => "スペシャル",
        "Stereo Imagery" => "ステレオイメージ",
        "Time and Pitch" => "タイムとピッチ",
        "Crossfade" => "クロスフェード",
        // video transitions
        "Animation" => "アニメーション",
        "Dissolve" => "ディゾルブ",
        "Grunge & Distort" => "グランジ＆ディストーション",
        "Iris" => "アイリス",
        "Lights & Blurs" => "ライト＆ブラー",
        "Page Peel" => "ページピール",
        "Slide" => "スライド",
        "Smart Tools" => "スマートツール",
        "Text" => "テキスト",
        "Transformers" => "トランスフォーマー",
        "Wipe" => "ワイプ",
        "Zoom" => "ズーム",
        // Lumetri Presets
        "Cinematic" => "シネマティック",
        "Film Emulation" => "フィルムエミュレーション",
        "Monochrome" => "モノクロ",
        "Technical" => "テクニカル",
        "Temperature" => "色温度",
        "Clean" => "クリーン",
        _ => return None,
    })
}

/// The effect / transition name shown in the Effects tree (Japanese when the UI is Japanese).
fn effect_display_name<'a>(id: &str, name: &'a str) -> &'a str {
    if !crate::i18n::is_japanese() {
        return name;
    }
    let Some(ja) = effect_ja(id) else {
        return name;
    };
    crate::i18n::remember(name, ja);
    ja
}

/// Japanese name of a catalog effect / transition, by id.
pub(crate) fn effect_ja(id: &str) -> Option<&'static str> {
    Some(match id {
        // intrinsic effects and graphics
        "motion" => "モーション",
        "opacity" => "不透明度",
        "time_remap" => "タイムリマップ",
        "volume" => "ボリューム",
        "channel_volume" => "チャンネルボリューム",
        "panner" => "パンナー",
        "graphic_text" => "テキスト",
        "graphic_shape" => "シェイプ",
        // Video Effects ▸ Adjust
        "levels" => "レベル",
        "proc_amp" => "プロアンプ",
        "extract" => "抽出",
        "lighting_effects" => "ライティング効果",
        // Blur & Sharpen
        "gaussian_blur" => "ブラー (ガウス)",
        "directional_blur" => "方向ブラー",
        "sharpen" => "シャープ",
        "unsharp_mask" => "アンシャープマスク",
        "bokeh_blur" => "ボケブラー",
        "channel_blur" => "チャンネルブラー",
        "compound_blur" => "合成ブラー",
        "focus_blur" => "フォーカスブラー",
        "reduce_interlace_flicker" => "インターレースのちらつき軽減",
        // Color Correction
        "brightness_contrast" => "輝度＆コントラスト",
        "tint" => "色合い",
        "lumetri" => "Lumetri カラー",
        "video_limiter" => "ビデオリミッター",
        "vignette" => "ビネット",
        // Distort
        "mirror" => "ミラー",
        "lens_distortion" => "レンズ歪み",
        "twirl" => "渦巻き",
        "wave_warp" => "波形ワープ",
        "corner_pin" => "コーナーピン",
        "magnify" => "拡大",
        "spherize" => "球面",
        "turbulent_displace" => "タービュレントディスプレイス",
        "warp_stabilizer" => "ワープスタビライザー",
        // Generate
        "four_color_gradient" => "4 色グラデーション",
        "gradient" => "グラデーション",
        // Image Control
        "black_white" => "白黒",
        "color_pass" => "カラーパス",
        "gamma_correction" => "ガンマ補正",
        "invert" => "反転",
        "channel_mix" => "チャンネルミキサー",
        "color_replace" => "カラー置き換え",
        "rounded_crop" => "角丸クロップ",
        // Immersive Video
        "vr_blur" => "VR ブラー",
        "vr_chromatic_aberrations" => "VR 色収差",
        "vr_color_gradients" => "VR カラーグラデーション",
        "vr_denoise" => "VR ノイズ除去",
        "vr_digital_glitch" => "VR デジタルグリッチ",
        "vr_fractal_noise" => "VR フラクタルノイズ",
        "vr_glow" => "VR グロー",
        "vr_plane_to_sphere" => "VR 平面から球面",
        "vr_projection" => "VR 投影",
        "vr_rotate_sphere" => "VR 球面を回転",
        "vr_sharpen" => "VR シャープ",
        // Keying
        "ultra_key" => "Ultra キー",
        "track_matte" => "トラックマットキー",
        "color_key" => "カラーキー",
        "luma_key" => "ルミナンスキー",
        "alpha_adjust" => "アルファ調整",
        "logo_cutout" => "ロゴ切り抜き",
        // Lights & Glows
        "lens_flare" => "レンズフレア",
        "echo_glow" => "エコーグロー",
        "edge_glow" => "エッジグロー",
        "glint" => "グリント",
        "light_leaks" => "ライトリーク",
        "rgb_split" => "RGB 分割",
        "volumetric_rays" => "ボリュームライト",
        "wonder_glow" => "ワンダーグロー",
        // Noise & Grain
        "noise" => "ノイズ",
        // Perspective
        "basic_3d" => "基本 3D",
        "drop_shadow" => "ドロップシャドウ",
        "long_shadow" => "ロングシャドウ",
        // Stylize
        "posterize" => "ポスタリゼーション",
        "mosaic" => "モザイク",
        "find_edges" => "輪郭検出",
        "strobe" => "ストロボライト",
        "brush_strokes" => "ブラシストローク",
        "color_emboss" => "カラーエンボス",
        "roughen_edges" => "エッジを粗く",
        // Time
        "posterize_time" => "ポスタリゼーション時間",
        // Transform
        "horizontal_flip" => "水平反転",
        "vertical_flip" => "垂直反転",
        "transform" => "トランスフォーム",
        "offset" => "オフセット",
        "rotate_3d" => "3D 回転",
        "auto_reframe" => "オートリフレーム",
        "camera_shake" => "カメラシェイク",
        "grow" => "拡大アニメーション",
        "shrink" => "縮小アニメーション",
        "move" => "移動",
        "spin" => "スピン",
        "spacer" => "スペーサー",
        "wiggle" => "ウィグル",
        // Utility / Video
        "simple_text" => "シンプルテキスト",
        "auto_align" => "自動整列",
        "cineon_converter" => "Cineon コンバーター",
        "clone" => "クローン",
        "stroke" => "線",
        "metadata_burnin" => "メタデータ＆タイムコードのバーンイン",
        // Video Effects ▸ Obsolete
        "color_balance" => "カラーバランス",
        "leave_color" => "色抜き",
        "change_to_color" => "他のカラーへ変更",
        "emboss" => "エンボス",
        "median" => "メディアン",
        "bevel_alpha" => "ベベルアルファ",
        "circle" => "円",
        "grid" => "グリッド",
        "timecode" => "タイムコード",
        "clip_name" => "クリップ名",
        "echo" => "エコー",
        "lightning" => "稲妻",
        "cell_pattern" => "セルパターン",
        "checkerboard" => "チェッカーボード",
        "ellipse" => "楕円",
        "paint_bucket" => "ペイントバケツ",
        "write_on" => "書き込み",
        // Legacy ▸ Video Effects
        "camera_blur" => "カメラブラー",
        "replicate" => "複製",
        "crop" => "クロップ",
        "edge_feather" => "エッジのぼかし",
        "ramp" => "ランプ",
        "alpha_glow" => "アルファグロー",
        "block_dissolve" => "ブロックディゾルブ",
        "directional_blur_legacy" => "方向ブラー (レガシー)",
        "gaussian_blur_legacy" => "ガウス (レガシー)",
        "gradient_wipe_legacy" => "グラデーションワイプ (レガシー)",
        "linear_wipe_legacy" => "リニアワイプ (レガシー)",
        "magnify_legacy" => "拡大 (レガシー)",
        "mosaic_legacy" => "モザイク (レガシー)",
        "noise_legacy" => "ノイズ (レガシー)",
        "twirl_legacy" => "渦巻き (レガシー)",
        // Audio Transitions ▸ Crossfade
        "constant_power" => "コンスタントパワー",
        "constant_gain" => "コンスタントゲイン",
        "exponential_fade" => "指数フェード",
        // Audio Effects ▸ Amplitude and Compression
        "amplify" => "増幅",
        "channel_mixer_a" => "チャンネルミキサー",
        "channel_volume_a" => "チャンネルボリューム",
        "deesser" => "ディエッサー",
        "dynamics_rack" => "ダイナミクス",
        "dynamics" => "ダイナミクス処理",
        "hard_limiter" => "ハードリミッター",
        "multiband_compressor" => "マルチバンドコンプレッサー",
        "single_band_compressor" => "シングルバンドコンプレッサー",
        "tube_compressor" => "チューブモデルコンプレッサー",
        // Delay and Echo
        "analog_delay" => "アナログディレイ",
        "delay" => "ディレイ",
        "multitap_delay" => "マルチタップディレイ",
        // Filter and EQ
        "bandpass" => "バンドパス",
        "bass" => "低音",
        "fft_filter" => "FFT フィルター",
        "graphic_eq" => "グラフィックイコライザー (10 バンド)",
        "graphic_eq_20" => "グラフィックイコライザー (20 バンド)",
        "graphic_eq_30" => "グラフィックイコライザー (30 バンド)",
        "highpass" => "ハイパス",
        "lowpass" => "ローパス",
        "notch" => "ノッチフィルター",
        "parametric_eq" => "パラメトリックイコライザー",
        "scientific_filter" => "サイエンティフィックフィルター",
        "simple_notch" => "シンプルノッチフィルター",
        "simple_eq" => "シンプルパラメトリック EQ",
        "treble" => "高音",
        // Modulation
        "chorus_flanger" => "コーラス / フランジャー",
        "flanger" => "フランジャー",
        "phaser" => "フェイザー",
        // Noise Reduction/Restoration
        "declicker" => "自動クリック除去",
        "dehummer" => "ハム除去",
        "denoise" => "ノイズ除去",
        "dereverb" => "リバーブ除去",
        "speech_enhance" => "スピーチを強調",
        // Reverb
        "convolution_reverb" => "コンボリューションリバーブ",
        "studio_reverb" => "スタジオリバーブ",
        "surround_reverb" => "サラウンドリバーブ",
        // Special
        "binauralizer" => "バイノーラライザー - アンビソニックス",
        "distortion" => "ディストーション",
        "fill_left" => "左チャンネルを右チャンネルで埋める",
        "fill_right" => "右チャンネルを左チャンネルで埋める",
        "guitar_suite" => "ギタースイート",
        "invert_a" => "反転",
        "loudness_radar" => "ラウドネスメーター",
        "mastering" => "マスタリング",
        "panner_ambisonics" => "パンナー - アンビソニックス",
        "swap_channels" => "チャンネルの入れ替え",
        "vocal_enhancer" => "ボーカル強調",
        // Stereo Imagery / Time and Pitch / loose
        "stereo_expander" => "ステレオエキスパンダー",
        "stereo_width" => "ステレオ幅",
        "pitch_shifter" => "ピッチシフター",
        "balance_a" => "バランス",
        "mute" => "ミュート",
        "volume_a" => "ボリューム",
        // Video Transitions ▸ Animation
        "block_motion" => "ブロックモーション",
        "flip_motion" => "フリップモーション",
        "fold_motion" => "フォールドモーション",
        "pop_motion" => "ポップモーション",
        "pull_motion" => "プルモーション",
        "spin_motion" => "スピンモーション",
        "spring_motion" => "スプリングモーション",
        "travel_motion" => "トラベルモーション",
        // Dissolve
        "additive_dissolve" => "加算ディゾルブ",
        "blur_dissolve" => "ブラーディゾルブ",
        "burn_alpha" => "バーンアルファ",
        "cross_dissolve" => "クロスディゾルブ",
        "dip_to_black" => "暗転 (黒へディップ)",
        "dip_to_color" => "カラーへディップ",
        "dip_to_white" => "明転 (白へディップ)",
        "film_dissolve" => "フィルムディゾルブ",
        "luma_fade" => "ルミナンスフェード",
        "morph_cut" => "モーフカット",
        "mosaic_transition" => "モザイク",
        // Grunge & Distort
        "chaos" => "カオス",
        "earthquake" => "地震",
        "flicker" => "フリッカー",
        "glass" => "ガラス",
        "glitch" => "グリッチ",
        "grunge" => "グランジ",
        "kaleidoscope" => "万華鏡",
        "liquid_distortion" => "液体ディストーション",
        "tv_power" => "テレビの電源",
        "vhs_damage" => "VHS ダメージ",
        // Immersive Video
        "vr_chroma_leaks" => "VR クロマリーク",
        "vr_gradient_wipe" => "VR グラデーションワイプ",
        "vr_iris_wipe" => "VR アイリスワイプ",
        "vr_light_leaks" => "VR ライトリーク",
        "vr_light_rays" => "VR 光線",
        "vr_mobius_zoom" => "VR メビウスズーム",
        "vr_random_blocks" => "VR ランダムブロック",
        "vr_spherical_blur" => "VR 球面ブラー",
        // Lights & Blurs
        "burn_chroma" => "バーンクロマ",
        "chroma_leak" => "クロマリーク",
        "cross_zoom" => "クロスズーム",
        "directional_blur_transition" => "方向ブラー",
        "flare" => "フレア",
        "flash" => "フラッシュ",
        "glow" => "グロー",
        "lens_blur" => "レンズブラー",
        "light_leak" => "ライトリーク",
        "light_sweep" => "ライトスイープ",
        "phosphor" => "フォスファー",
        "radial_blur" => "放射状ブラー",
        "ray" => "レイ",
        "solarize" => "ソラリゼーション",
        "stripe" => "ストライプ",
        "zoom_blur" => "ズームブラー",
        // Slide
        "roll_3d" => "3D ロール",
        "film_roll" => "フィルムロール",
        "push" => "プッシュ",
        "roll" => "ロール",
        "slide" => "スライド",
        "split" => "スプリット",
        "stretch" => "ストレッチ",
        "whip" => "ホイップ",
        // Smart Tools
        "motion_camera" => "モーションカメラ",
        "motion_tween" => "モーショントゥイーン",
        "shape_dissolve" => "シェイプディゾルブ",
        "shape_flow" => "シェイプフロー",
        // Text
        "text_animator" => "テキストアニメーター",
        "typewriter" => "タイプライター",
        // Transformers
        "spin_3d" => "3D スピン",
        "spinback_3d" => "3D スピンバック",
        "frame" => "フレーム",
        "louver" => "ルーバー",
        "mirror_transition" => "ミラー",
        "page_peel" => "ページピール",
        "slice" => "スライス",
        "wave" => "ウェーブ",
        // Wipe
        "clock_wipe" => "クロックワイプ",
        "linear_wipe" => "リニアワイプ",
        "neon_wipe" => "ネオンワイプ",
        "panel_wipe" => "パネルワイプ",
        "plateau_wipe" => "プラトーワイプ",
        "radial_wipe" => "放射状ワイプ",
        "soft_wipe" => "ソフトワイプ",
        "star_wipe" => "スターワイプ",
        "stretch_wipe" => "ストレッチワイプ",
        // Legacy ▸ Video Transitions
        "additive_dissolve_legacy" => "加算ディゾルブ (レガシー)",
        "barn_doors" => "バーンドア",
        "center_split" => "センタースプリット",
        "clock_wipe_legacy" => "クロックワイプ (レガシー)",
        "cross_dissolve_legacy" => "クロスディゾルブ (レガシー)",
        "cross_zoom_legacy" => "クロスズーム (レガシー)",
        "dip_to_black_legacy" => "黒へディップ (レガシー)",
        "dip_to_white_legacy" => "白へディップ (レガシー)",
        "film_dissolve_legacy" => "フィルムディゾルブ (レガシー)",
        "inset" => "インセット",
        "iris_box" => "アイリス (ボックス)",
        "iris_cross" => "アイリス (クロス)",
        "iris_diamond" => "アイリス (ダイヤモンド)",
        "iris_round" => "アイリス (円形)",
        "non_additive_dissolve" => "非加算ディゾルブ",
        "push_legacy" => "プッシュ (レガシー)",
        "radial_wipe_legacy" => "放射状ワイプ (レガシー)",
        "slide_legacy" => "スライド (レガシー)",
        "split_legacy" => "スプリット (レガシー)",
        "whip_legacy" => "ホイップ (レガシー)",
        "wipe" => "ワイプ",
        // Obsolete ▸ Video Transitions
        "band_slide" => "バンドスライド",
        "checker_wipe" => "チェッカーワイプ",
        "cube_spin" => "キューブスピン",
        "flip_over" => "フリップオーバー",
        "gradient_wipe" => "グラデーションワイプ",
        "page_turn" => "ページターン",
        "venetian_blinds" => "ベネチアンブラインド",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Catalog ids whose display name legitimately stays ASCII (an industry-standard name).
    const ASCII_IDS: &[&str] = &["asc_cdl"];

    #[test]
    fn every_effect_and_folder_has_a_japanese_name() {
        let defs = filmcraft_project::effect_defs();
        assert!(defs.len() > 250, "catalog unexpectedly small: {}", defs.len());
        let mut folders: Vec<&str> = filmcraft_project::vtransition::EFFECT_TOP_FOLDERS.to_vec();
        folders.extend(filmcraft_render::lumetri_presets::FOLDERS);
        for d in defs.iter() {
            folders.extend(d.category.iter().copied());
        }
        folders.sort_unstable();
        folders.dedup();
        // Check the pure lookups (the display functions use them when Japanese is selected): the
        // UI language is process-wide and other tests expect English, so it is not switched here.
        let missing_fx: Vec<String> = defs
            .iter()
            .filter(|d| !ASCII_IDS.contains(&d.id))
            .filter(|d| effect_ja(d.id).is_none_or(|ja| ja.is_ascii()))
            .map(|d| format!("{} ({})", d.id, d.name))
            .collect();
        let missing_folders: Vec<&str> = folders.iter().copied().filter(|f| folder_ja(f).is_none_or(|ja| ja.is_ascii())).collect();
        assert!(
            missing_fx.is_empty() && missing_folders.is_empty(),
            "untranslated effects ({}): {missing_fx:?}\nuntranslated folders ({}): {missing_folders:?}",
            missing_fx.len(),
            missing_folders.len()
        );
        assert_eq!(effect_ja("cross_dissolve"), Some("クロスディゾルブ"));
        assert_eq!(folder_ja("Animation"), Some("アニメーション"));
    }
}
