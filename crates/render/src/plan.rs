//! Frame plans for the GPU compositor.
//!
//! [`plan_frame`] resolves what is visible at a time into a list of layers the GPU can draw
//! directly: a decoded source frame (YUV planes or RGBA), the matrix from source pixels to output
//! pixels, an opacity and a blend mode (all 27 of [`Blend`] are composited by the GPU). A media
//! clip whose enabled standard effects all have a GPU implementation ([`crate::gpufx`]) carries
//! them as a [`LayerFx`]: their parameters evaluated at the frame's time, run by the GPU on the
//! clip's working image before Motion places it, in the CPU's order (effects → Motion → Opacity /
//! blend). Effect masks and a clip's Opacity masks travel with the [`LayerFx`] as flattened
//! polygons ([`FlatMask`]): the GPU mixes each masked effect with the image before it by the
//! masks' coverage. Anything the shaders don't cover yet — other standard effects, masks with
//! more than [`MAX_GPU_MASK_POINTS`] points, opacity masks of adjustment layers, nested
//! sequences, non-dissolve transitions — is rendered on the CPU for that layer (or the whole
//! frame) and handed over as a pre-composited image, so the GPU path is always exact with respect
//! to the CPU reference.

use std::sync::Arc;

use filmcraft_color::{ColorPipeline, ColorSpace};
use filmcraft_frame::{PixelData, VideoFrame};
use filmcraft_geom::Affine;
use filmcraft_media::FrameRequest;
use filmcraft_project::{ItemId, ItemKind, Project, Sequence, TrackItem};
use filmcraft_time::Tick;

use crate::gpufx::FxOp;
use crate::mask::FlatMask;
use crate::{Blend, RenderOptions, SourceProvider, motion_matrix, output_size};

/// One layer for the GPU, bottom to top.
#[derive(Clone)]
pub struct PlanLayer {
    pub frame: Arc<VideoFrame>,
    /// Maps frame pixels (0..w, 0..h) to output pixels — or, with [`fx`](Self::fx), working-image
    /// pixels (0..fx.size).
    pub matrix: Affine,
    pub opacity: f32,
    /// How the layer combines with what is under it ([`crate::blend::composite`]).
    pub blend: Blend,
    /// Standard effects to run on the frame before it is placed (None: draw the frame directly).
    pub fx: Option<Arc<LayerFx>>,
    /// Whether this is an adjustment layer (effects run on the composite below it).
    pub is_adjustment: bool,
}

/// The GPU effect stage of a layer: the frame is decoded into a working image of `size` (the
/// frame box-decimated by `decimation`, as the CPU decodes it), `ops` run on it in order, and the
/// result is placed with the layer's matrix, opacity and blend mode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerFx {
    pub size: (u32, u32),
    pub decimation: u32,
    pub ops: Vec<FxOp>,
    /// `masks[i]` limits `ops[i]` to the masks' coverage, in working-image pixels (missing or
    /// empty: the op applies everywhere).
    pub masks: Vec<Vec<FlatMask>>,
    /// The masks of the clip's Opacity effect: they scale the result of the ops.
    pub opacity_masks: Vec<FlatMask>,
    /// The frame's [`input_matrix`] into the working gamut (applied when it is decoded).
    pub input_matrix: Option<[[f32; 3]; 3]>,
}

/// Polygon points of all the masks of a layer the GPU evaluates per pixel; a layer with more is
/// rendered on the CPU, whose row algorithm does not look at every edge for every pixel.
pub const MAX_GPU_MASK_POINTS: usize = 2048;
/// Masks of a layer the GPU evaluates (same reason).
pub const MAX_GPU_MASKS: usize = 32;

impl LayerFx {
    /// The stage on the CPU: the reference the GPU effect stage is tested against.
    pub fn apply(&self, img: &mut crate::Image) {
        for (i, op) in self.ops.iter().enumerate() {
            match self.masks.get(i).and_then(|m| crate::mask::coverage(m, img.w, img.h)) {
                Some(cov) => {
                    let original = img.clone();
                    op.apply(img);
                    crate::mask::mix(img, &original, &cov);
                }
                None => op.apply(img),
            }
        }
        if let Some(cov) = crate::mask::coverage(&self.opacity_masks, img.w, img.h) {
            crate::mask::scale_by(img, &cov);
        }
    }

    /// Whether the masks are few enough for the GPU ([`MAX_GPU_MASK_POINTS`], [`MAX_GPU_MASKS`]).
    pub fn masks_fit_gpu(&self) -> bool {
        let all = || self.masks.iter().flatten().chain(&self.opacity_masks);
        all().count() <= MAX_GPU_MASKS && all().map(|m| m.pts.len()).sum::<usize>() <= MAX_GPU_MASK_POINTS
    }
}

impl PlanLayer {
    /// A layer drawing `frame` directly.
    pub fn new(frame: Arc<VideoFrame>, matrix: Affine, opacity: f32, blend: Blend) -> Self {
        Self { frame, matrix, opacity, blend, fx: None, is_adjustment: false }
    }

    /// An adjustment layer: runs `fx` on the composite below it and blends the result.
    pub fn adjustment(matrix: Affine, opacity: f32, blend: Blend, fx: Arc<LayerFx>) -> Self {
        let dummy = Arc::new(VideoFrame::rgba_f32(1, 1, vec![0.0; 4]));
        Self { frame: dummy, matrix, opacity, blend, fx: Some(fx), is_adjustment: true }
    }

    /// Size of the picture the matrix places: the working image with effects, else the frame.
    pub fn size(&self) -> (u32, u32) {
        match &self.fx {
            Some(fx) => fx.size,
            None => (self.frame.width, self.frame.height),
        }
    }
}

#[derive(Clone)]
pub enum FramePlan {
    /// Draw these layers over black. `color` is the sequence's colour pipeline: YUV / RGBA8
    /// frames are converted into its working gamut ([`input_matrix`]), and a pipeline that is not
    /// plain (HDR or wide gamut) is converted for the monitor at the end
    /// (`colorman::to_display`).
    Layers { width: usize, height: usize, layers: Vec<PlanLayer>, color: ColorPipeline },
    /// The CPU produced the final image (fallback).
    Image(crate::Image),
}

fn cpu_frame(img: crate::Image) -> Arc<VideoFrame> {
    Arc::new(VideoFrame::rgba_f32(img.w as u32, img.h as u32, img.px))
}

fn simple_transition(id: &str) -> bool {
    matches!(id, "cross_dissolve" | "dip_to_black" | "dip_to_white")
}

fn gpu_adjustment_ok(_project: &Project, seq: &Sequence, item: &TrackItem, t: Tick, opts: RenderOptions) -> bool {
    if !opts.effects {
        return true;
    }
    if item.has_opacity_masks() {
        return false;
    }
    let mt = item.effect_time_at(t);
    let size = (seq.settings.width, seq.settings.height);
    let m = motion_matrix(seq, item, size, mt);
    if !near_identity(&m) {
        return false;
    }
    let mut masks = LayerFx::default();
    for e in item.effects.iter().filter(|e| e.def().is_some_and(|d| !d.intrinsic)) {
        if !e.enabled {
            continue;
        }
        if !crate::gpufx::GPU_EFFECTS.contains(&e.effect.as_str()) {
            return false;
        }
        masks.masks.push(crate::mask::prepare(&e.masks, mt, opts.scale));
    }
    masks.masks_fit_gpu()
}

/// The standard effects of a media clip if the GPU can draw it (any blend mode, every enabled
/// standard effect with a GPU implementation; empty without effects or with effects off). None:
/// the clip is rendered on the CPU.
fn gpu_chain<'a>(project: &Project, item: &'a TrackItem, opts: RenderOptions) -> Option<Vec<&'a filmcraft_project::EffectInstance>> {
    let is_media = project.item(item.item).is_some_and(|p| matches!(p.kind, ItemKind::Media(_) | ItemKind::Subclip { .. }));
    if !is_media {
        return None;
    }
    if !opts.effects {
        return Some(Vec::new());
    }
    let mut chain = Vec::new();
    for e in item.effects.iter().filter(|e| e.def().is_some_and(|d| !d.intrinsic) && !filmcraft_project::graphic::is_layer(e)) {
        if !e.enabled {
            continue;
        }
        if !crate::gpufx::GPU_EFFECTS.contains(&e.effect.as_str()) {
            return None;
        }
        chain.push(e);
    }
    Some(chain)
}

/// Plan the frame at timeline `t`.
pub fn plan_frame(project: &Project, seq_id: ItemId, t: Tick, opts: RenderOptions, sources: &dyn SourceProvider) -> FramePlan {
    let Some(seq) = project.sequence(seq_id) else { return FramePlan::Image(crate::Image::new(1, 1)) };
    let (w, h) = output_size(seq, opts.scale);
    // HDR / wide-gamut sequences composite on the GPU too (BT.709 / sRGB media decoded there,
    // everything else converted on the CPU per layer), except where the CPU composites outside
    // the working space: working-space output (exports), captions (drawn after the display
    // transform) and blend modes other than Normal (checked below).
    let pipe = seq.settings.color;
    if !pipe.is_plain() && (opts.working_output || (opts.captions && !crate::caption_overlays(seq, t, w, h).is_empty())) {
        return FramePlan::Image(crate::render_sequence(project, seq_id, t, opts, sources));
    }
    // Whole-frame fallback: adjustment layers or complex transitions anywhere at t.
    for tr in &seq.video_tracks {
        if !tr.enabled {
            continue;
        }
        if let Some(trn) = tr.transitions.iter().find(|x| x.range().contains(t))
            && !simple_transition(&trn.effect.effect)
        {
            return FramePlan::Image(crate::render_sequence(project, seq_id, t, opts, sources));
        }
        if let Some(it) = tr.item_at(t)
            && project.item(it.item).is_some_and(|p| matches!(p.kind, ItemKind::AdjustmentLayer { .. }))
            && !gpu_adjustment_ok(project, seq, it, t, opts)
        {
            return FramePlan::Image(crate::render_sequence(project, seq_id, t, opts, sources));
        }
    }
    let mut layers = Vec::new();
    for tr in &seq.video_tracks {
        if !tr.enabled {
            continue;
        }
        if let Some(trn) = tr.transitions.iter().find(|x| x.range().contains(t)) {
            // These dissolves are symmetric: Reverse (play B→A backwards) renders the same frames.
            let p = trn.progress(t) as f32;
            let a = trn.from.and_then(|id| tr.item(id)).filter(|i| i.enabled);
            let b = trn.to.and_then(|id| tr.item(id)).filter(|i| i.enabled);
            match trn.effect.effect.as_str() {
                "dip_to_black" | "dip_to_white" => {
                    let col = if trn.effect.effect == "dip_to_black" { [0.0, 0.0, 0.0, 1.0] } else { [1.0, 1.0, 1.0, 1.0] };
                    layers.push(PlanLayer::new(Arc::new(VideoFrame::rgba_f32(1, 1, col.to_vec())), Affine::scale(w as f64, h as f64), 1.0, Blend::Normal));
                    let (it, k) = if p < 0.5 { (a, 1.0 - p * 2.0) } else { (b, (p - 0.5) * 2.0) };
                    if let Some(it) = it {
                        push_item(project, seq, it, t, opts, sources, k, Some(Blend::Normal), &mut layers);
                    }
                }
                _ => {
                    // cross dissolve: A at full, B over it at p (premultiplied over == linear mix when A is opaque)
                    if let Some(it) = a {
                        push_item(project, seq, it, t, opts, sources, 1.0 - if b.is_none() { p } else { 0.0 }, Some(Blend::Normal), &mut layers);
                    }
                    if let Some(it) = b {
                        push_item(project, seq, it, t, opts, sources, p, Some(Blend::Normal), &mut layers);
                    }
                }
            }
            continue;
        }
        let Some(item) = tr.item_at(t) else { continue };
        if !item.enabled {
            continue;
        }
        push_item(project, seq, item, t, opts, sources, 1.0, None, &mut layers);
    }
    if opts.captions {
        for o in crate::caption_overlays(seq, t, w, h) {
            layers.push(PlanLayer::new(
                Arc::new(VideoFrame::rgba_f32(o.w as u32, o.h as u32, o.px)),
                Affine::translate(o.x as f64, o.y as f64),
                1.0,
                Blend::Normal,
            ));
        }
    }
    if !pipe.is_plain() && layers.iter().any(|l| l.blend != Blend::Normal) {
        return FramePlan::Image(crate::render_sequence(project, seq_id, t, opts, sources));
    }
    FramePlan::Layers { width: w, height: h, layers, color: pipe }
}

/// The gamut matrix from a GPU-decoded frame's own primaries into the working gamut of a
/// non-plain `pipe` (None: nothing to convert). The compositor applies it after decoding YUV and
/// RGBA8 frames, as `colorman::decode` does on the CPU; RGBA float frames (CPU-rendered layers,
/// effect results, transition colours) are in the working space already.
pub fn input_matrix(pipe: &ColorPipeline, frame: &VideoFrame) -> Option<[[f32; 3]; 3]> {
    if pipe.is_plain() || matches!(frame.data, PixelData::RgbaF32(_)) {
        return None;
    }
    let (src, wg) = (ColorSpace::from_info(&frame.color).gamut(), pipe.working_gamut());
    (src != wg).then(|| filmcraft_color::spaces::to_f32(&filmcraft_color::spaces::gamut_matrix(src, wg)))
}

/// Whether the GPU decodes a frame into the working space itself: the default (transfer-only)
/// decode of BT.709 / sRGB / linear media, plus [`input_matrix`] into a wide working gamut (YUV
/// and RGBA8 frames). Everything else (log, HDR, other gamuts, Interpret Footage overrides) is
/// converted on the CPU.
fn gpu_decodes(pipe: &ColorPipeline, cs: ColorSpace, frame: &VideoFrame) -> bool {
    if pipe.is_plain() {
        return !crate::colorman::needs_management(pipe, cs, frame);
    }
    let default_decode = cs == ColorSpace::from_info(&frame.color) && matches!(cs, ColorSpace::Rec709 | ColorSpace::Srgb | ColorSpace::Linear709);
    default_decode && !matches!(frame.data, PixelData::RgbaF32(_))
}

fn apply_matrix(img: &mut crate::Image, m: &[[f32; 3]; 3]) {
    for p in img.px.as_chunks_mut::<4>().0 {
        let c = filmcraft_color::spaces::apply3(m, [p[0], p[1], p[2]]);
        p[..3].copy_from_slice(&c);
    }
}

/// Push the layer(s) of `item`. `blend` overrides the item's own blend mode: inside a transition
/// the CPU reference mixes the clips and composites the result Normal, ignoring their modes.
#[allow(clippy::too_many_arguments)]
fn push_item(
    project: &Project,
    seq: &Sequence,
    item: &TrackItem,
    t: Tick,
    opts: RenderOptions,
    sources: &dyn SourceProvider,
    extra_opacity: f32,
    blend: Option<Blend>,
    out: &mut Vec<PlanLayer>,
) {
    // frame time (`ft`) vs. effect time (`mt`): they differ inside a frame hold without Hold Filters
    let ft = item.source_time_at(t);
    let mt = item.effect_time_at(t);
    let (op, own) = crate::opacity_blend(item, mt);
    let bl = blend.unwrap_or(own);
    // A multi-camera clip that only shows its angle (no effects, untransformed, same frame size)
    // draws the angle's clip directly: no CPU pass over the nested sequence.
    if let Some(ItemKind::Sequence(nested)) = project.item(item.item).map(|p| &p.kind)
        && let Some(angle) = item.multicam_angle(nested)
        && !(opts.effects && item.has_standard_effects())
        && (nested.settings.width, nested.settings.height) == (seq.settings.width, seq.settings.height)
        && near_identity(&motion_matrix(seq, item, (nested.settings.width, nested.settings.height), mt))
        && let Some(tr) = nested.angle_video_track_index(angle).and_then(|i| nested.video_tracks.get(i))
        && !tr.transitions.iter().any(|x| x.range().contains(ft))
        && tr.item_at(ft).is_none_or(|i| crate::opacity_blend(i, i.effect_time_at(ft)).1 != Blend::Dissolve)
    {
        // Inside the nested sequence the angle's clip is composited onto an empty canvas, where
        // every mode but Dissolve is Normal; the multicam clip's own mode then applies to it.
        if let Some(inner) = tr.item_at(ft).filter(|i| i.enabled) {
            push_item(project, nested, inner, ft, opts, sources, extra_opacity * op, Some(bl), out);
        }
        return;
    }
    // Graphic clips without standard effects: the layers are rasterised (cached) into one tight
    // image the GPU places as a layer.
    if !(opts.effects && item.has_standard_effects())
        && !item.has_opacity_masks()
        && project.item(item.item).is_some_and(|p| matches!(p.kind, ItemKind::Graphic { .. }))
    {
        let Some(size) = crate::source_size(project, item.item) else { return };
        let (w, h) = output_size(seq, opts.scale);
        let m = Affine::scale(opts.scale as f64, opts.scale as f64).then_apply(&motion_matrix(seq, item, size, mt));
        if let Some((img, x, y)) = crate::graphic_clip::render_graphic_tight(item, mt, size, &m, w, h) {
            out.push(PlanLayer::new(cpu_frame(img), Affine::translate(x as f64, y as f64), op * extra_opacity, bl));
        }
        return;
    }
    // Adjustment layer with GPU effects: operates on the composite below it
    if let Some(pi) = project.item(item.item)
        && matches!(pi.kind, ItemKind::AdjustmentLayer { .. })
    {
        if !opts.effects {
            return;
        }
        let (w, h) = output_size(seq, opts.scale);
        let cx = crate::effects::FxCtx {
            t: mt,
            px_scale: opts.scale,
            seconds: (t - item.start).seconds(),
            timecode: "",
            clip_name: &item.name,
            project: Some(project),
            env: None,
            working: seq.settings.color.working,
        };
        let enabled = || item.effects.iter().filter(|e| e.enabled && e.def().is_some_and(|d| !d.intrinsic));
        let ops: Option<Vec<FxOp>> = enabled().map(|e| FxOp::eval(e, &cx, w, h).filter(FxOp::gpu_ok)).collect();
        if let Some(ops) = ops
            && (!ops.is_empty() || bl != Blend::Normal)
        {
            // (in sequence pixels, as the CPU applies them; `gpu_adjustment_ok` checked their number)
            let masks = enabled().map(|e| crate::mask::prepare(&e.masks, mt, opts.scale)).collect();
            let fx = LayerFx { size: (w as u32, h as u32), decimation: 1, ops, masks, opacity_masks: Vec::new(), input_matrix: None };
            out.push(PlanLayer::adjustment(Affine::IDENTITY, op * extra_opacity, bl, Arc::new(fx)));
        }
        return;
    }
    if let Some(chain) = gpu_chain(project, item, opts) {
        let Some(src) = sources.source(item.item) else { return };
        let Some(size) = crate::source_size(project, item.item) else { return };
        let motion = motion_matrix(seq, item, size, mt);
        let lin = ((motion.a * motion.a + motion.b * motion.b).sqrt()).max((motion.c * motion.c + motion.d * motion.d).sqrt());
        let want = (lin * opts.scale as f64).clamp(1.0 / 64.0, 1.0) as f32;
        let Ok(frame) = src.video_frame(FrameRequest { time: ft, scale: want }) else { return };
        let cs = crate::colorman::source_space(project, item.item, &frame);
        // log / HDR / wide-gamut media is converted on the CPU (below), and so are blended
        // in-between frames (Frame Blending / Optical Flow on speed-changed clips)
        let plain = gpu_decodes(&seq.settings.color, cs, &frame) && crate::interpolation_blend(item, t, src.info().frame_rate()).is_none();
        // (the Opacity effect's masks apply with effects off too, like on the CPU)
        let opacity_masks = item.effect("opacity").filter(|_| item.has_opacity_masks());
        if plain && (!chain.is_empty() || opacity_masks.is_some()) {
            // GPU effect stage: the working image the CPU would decode (`base_layer`), the
            // effects evaluated for it, placed as `item_layer` places it
            let n = crate::decimation(frame.width as f32, size.0 as f32 * want);
            let (lw, lh) = ((frame.width as usize / n).max(1), (frame.height as usize / n).max(1));
            let px_scale = lw as f32 / size.0.max(1) as f32;
            let tc = "";
            let cx = crate::effects::FxCtx {
                t: mt,
                px_scale,
                seconds: (t - item.start).seconds(),
                timecode: tc,
                clip_name: &item.name,
                project: Some(project),
                env: None,
                working: seq.settings.color.working,
            };
            let ops: Option<Vec<FxOp>> = chain.iter().map(|e| FxOp::eval(e, &cx, lw, lh).filter(FxOp::gpu_ok)).collect();
            let fx = ops.map(|ops| LayerFx {
                size: (lw as u32, lh as u32),
                decimation: n as u32,
                input_matrix: input_matrix(&seq.settings.color, &frame),
                ops,
                masks: chain.iter().map(|e| crate::mask::prepare(&e.masks, mt, px_scale)).collect(),
                opacity_masks: opacity_masks.map(|e| crate::mask::prepare(&e.masks, mt, px_scale)).unwrap_or_default(),
            });
            if let Some(fx) = fx.filter(LayerFx::masks_fit_gpu) {
                let m = Affine::scale(opts.scale as f64, opts.scale as f64)
                    .then_apply(&motion)
                    .then_apply(&Affine::scale(1.0 / px_scale as f64, 1.0 / px_scale as f64));
                out.push(PlanLayer { frame, matrix: m, opacity: op * extra_opacity, blend: bl, fx: Some(Arc::new(fx)), is_adjustment: false });
                return;
            }
        } else if plain {
            // Draft playback at reduced resolution: hand over planes box-filtered to the size
            // drawn instead of the full picture (a quarter at 1/2, a sixteenth at 1/4 of the
            // upload and sampling). The mean is taken over Y'CbCr codes, not linear light as
            // the shader's supersampling does, so this stays limited to the opt-in draft mode.
            let n = if filmcraft_media::cancel::draft() { crate::decimation(frame.width as f32, size.0 as f32 * want) } else { 1 };
            let frame = match frame.box_decimated(n) {
                Some(small) => Arc::new(small),
                None => frame,
            };
            let px_scale = frame.width as f64 / size.0.max(1) as f64;
            let m = Affine::scale(opts.scale as f64, opts.scale as f64).then_apply(&motion).then_apply(&Affine::scale(1.0 / px_scale, 1.0 / px_scale));
            out.push(PlanLayer::new(frame, m, op * extra_opacity, bl));
            return;
        }
    }
    // CPU-rendered layer (standard effects): drawn by the GPU as a pre-rendered canvas image.
    let tc = filmcraft_time::format_time(t, seq.settings.frame_rate, seq.settings.drop_frame, filmcraft_time::TimeDisplay::Timecode, 48_000);
    if let Some((img, op2, _)) = crate::item_layer(project, seq, item, t, opts, sources, &tc) {
        out.push(PlanLayer::new(cpu_frame(img), Affine::IDENTITY, op2 * extra_opacity, bl));
    }
}

fn near_identity(m: &Affine) -> bool {
    (m.a - 1.0).abs() < 1e-9 && (m.d - 1.0).abs() < 1e-9 && m.b.abs() < 1e-9 && m.c.abs() < 1e-9 && m.e.abs() < 1e-6 && m.f.abs() < 1e-6
}

/// A layer's working image with its effects applied, on the CPU (the reference for the GPU
/// effect stage).
pub fn effect_image(frame: &VideoFrame, fx: &LayerFx) -> crate::Image {
    let (w, h, px) = frame.to_linear_f32_decimated(fx.decimation.max(1) as usize);
    let mut img = crate::Image { w, h, px };
    if let Some(m) = &fx.input_matrix {
        apply_matrix(&mut img, m);
    }
    fx.apply(&mut img);
    img
}

/// Execute a plan on the CPU (reference for the GPU compositor).
pub fn execute_cpu(plan: &FramePlan) -> crate::Image {
    match plan {
        FramePlan::Image(img) => img.clone(),
        FramePlan::Layers { width, height, layers, color } => {
            let mut canvas = crate::Image::new(*width, *height);
            for l in layers {
                let src = if l.is_adjustment {
                    let mut img = canvas.clone();
                    if let Some(fx) = &l.fx {
                        fx.apply(&mut img);
                    }
                    img
                } else {
                    match &l.fx {
                        Some(fx) => effect_image(&l.frame, fx),
                        None => {
                            let mut img = crate::Image { w: l.frame.width as usize, h: l.frame.height as usize, px: l.frame.to_linear_f32() };
                            if let Some(m) = input_matrix(color, &l.frame) {
                                apply_matrix(&mut img, &m);
                            }
                            img
                        }
                    }
                };
                let placed = if l.matrix == Affine::scale(*width as f64, *height as f64) && src.w == 1 && src.h == 1 {
                    crate::Image::filled(*width, *height, src.get(0, 0))
                } else if l.matrix == Affine::IDENTITY && src.w == *width && src.h == *height {
                    src
                } else {
                    src.transformed(*width, *height, &l.matrix)
                };
                crate::blend::composite(&mut canvas, &placed, l.opacity, l.blend);
            }
            crate::colorman::to_display(&mut canvas, color);
            canvas
        }
    }
}
