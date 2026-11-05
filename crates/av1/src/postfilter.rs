//! In-loop post filters run by decode_frame_wrapup: loop filter (7.14), CDEF (7.15),
//! super-resolution upscaling (7.16) and loop restoration (7.17).

use crate::frame::RowsMut;
use crate::par::POST_TASKS;
use crate::spec_tables::*;
use crate::state::FrameState;
use crate::stats::{DecodeStats, Stage, Timer};

/// The post-filters run on the frame worker; `_pool` is for row-parallel filtering (not yet used).
/// `draft`: only super-resolution (which sets the output size) runs.
pub(crate) fn apply(fs: &mut FrameState, stats: &mut DecodeStats, pool: &crate::par::Pool, draft: bool) {
    let mut t = Timer::start();
    let lvl = fs.fh.lf.level;
    if (lvl[0] != 0 || lvl[1] != 0) && !draft {
        loop_filter(fs, pool);
    }
    stats.add(Stage::LoopFilter, t.lap());
    let cdef = if draft { None } else { crate::cdef::apply(fs, pool) };
    stats.add(Stage::Cdef, t.lap());
    let (up_cur, up_cdef) = if fs.fh.use_superres {
        let c = crate::restoration::upscale(fs, &fs.cur);
        let d = cdef.as_ref().map(|f| crate::restoration::upscale(fs, f));
        (c, d)
    } else {
        (std::mem::take(&mut fs.cur), cdef)
    };
    stats.add(Stage::Superres, t.lap());
    fs.cur = if fs.fh.lr.uses_lr && !draft {
        crate::restoration::loop_restoration(fs, &up_cur, up_cdef.as_ref().unwrap_or(&up_cur))
    } else {
        up_cdef.unwrap_or(up_cur)
    };
    stats.add(Stage::Restoration, t.lap());
}

// ---------------------------------------------------------------------------------------------
// Loop filter (7.14)

/// Rows of a 64x64 block row per band (in luma samples).
const LF_BAND: usize = 64;
/// Samples an edge's filter reads or writes on either side of it (the 16-tap filter reads 7).
const LF_REACH: usize = 8;

/// The loop filter, row-parallel. Every edge's filter reads and writes at most min(filter
/// size) / 2 samples on each side, and each filter size is at most the transform sizes on both
/// sides, so no two edges of a pass touch the same sample: the result does not depend on the
/// order edges are filtered in. Vertical edges (pass 0) filter along rows, so bands of rows are
/// independent; horizontal edges (pass 1) filter across rows, so a band also needs
/// `LF_REACH` rows above and below: even bands run together, then odd ones.
fn loop_filter(fs: &mut FrameState, pool: &crate::par::Pool) {
    let mut cur = std::mem::take(&mut fs.cur);
    loop_filter_planes(fs, &mut cur, pool);
    fs.cur = cur;
}

fn loop_filter_planes(fs: &FrameState, cur: &mut crate::frame::FrameBuf, pool: &crate::par::Pool) {
    let mi_rows = fs.fh.mi_rows as usize;
    let n = mi_rows.div_ceil(LF_BAND / 4);
    for plane in 0..fs.num_planes {
        if plane == 0 || fs.fh.lf.level[1 + plane] != 0 {
            let sy = if plane == 0 { 0 } else { fs.ssy };
            let pl = &mut cur.planes[plane];
            let band_rows = |b: usize| ((LF_BAND * b) >> sy, (LF_BAND * (b + 1)) >> sy);
            // pass 0: vertical edges
            let ranges: Vec<(usize, usize)> = (0..n).map(band_rows).collect();
            let mut bands = pl.bands_mut(&ranges);
            pool.map_mut_tasks(&mut bands, POST_TASKS, |b, band| lf_band(fs, band, plane, 0, b));
            drop(bands);
            // pass 1: horizontal edges
            for parity in 0..2 {
                let ids: Vec<usize> = (parity..n).step_by(2).collect();
                let ranges: Vec<(usize, usize)> = ids
                    .iter()
                    .map(|&b| {
                        let (a, e) = band_rows(b);
                        (a.saturating_sub(LF_REACH), e + LF_REACH)
                    })
                    .collect();
                let mut bands = pl.bands_mut(&ranges);
                pool.map_mut_tasks(&mut bands, POST_TASKS, |i, band| {
                    if let Some(&b) = ids.get(i) {
                        lf_band(fs, band, plane, 1, b);
                    }
                });
            }
        }
    }
}

/// The edges of one pass in band `b` (mode info rows [16 b, 16 b + 16)), in raster order.
fn lf_band(fs: &FrameState, band: &mut RowsMut<'_>, plane: usize, pass: usize, b: usize) {
    let (mi_rows, mi_cols) = (fs.fh.mi_rows as usize, fs.fh.mi_cols as usize);
    let row_step = if plane == 0 { 1 } else { 1 << fs.ssy };
    let col_step = if plane == 0 { 1 } else { 1 << fs.ssx };
    let mut row = b * (LF_BAND / 4);
    let end = (row + LF_BAND / 4).min(mi_rows);
    while row < end {
        let mut col = 0;
        while col < mi_cols {
            edge(fs, band, plane, pass, row, col);
            col += col_step;
        }
        row += row_step;
    }
}

fn edge(fs: &FrameState, pl: &mut RowsMut<'_>, plane: usize, pass: usize, row: usize, col: usize) {
    let (sub_x, sub_y) = if plane == 0 { (0, 0) } else { (fs.ssx, fs.ssy) };
    let (dx, dy) = if pass == 0 { (1i32, 0i32) } else { (0, 1) };
    let x = col * 4;
    let y = row * 4;
    let row = row | sub_y;
    let col = col | sub_x;
    let on_screen = !(x >= fs.fh.frame_width as usize || y >= fs.fh.frame_height as usize || (pass == 0 && x == 0) || (pass == 1 && y == 0));
    if !on_screen {
        return;
    }
    let xp = x >> sub_x;
    let yp = y >> sub_y;
    let prev_row = row - ((dy as usize) << sub_y);
    let prev_col = col - ((dx as usize) << sub_x);
    let tx_sz = fs.lf_tx_size(plane, row >> sub_y, col >> sub_x);
    let is_tx_edge = if pass == 0 { xp.is_multiple_of(TX_WIDTH[tx_sz] as usize) } else { yp.is_multiple_of(TX_HEIGHT[tx_sz] as usize) };
    if !is_tx_edge {
        return;
    }
    let mi = &fs.mi;
    let i = mi.idx(row, col);
    let mi_size = mi.mi_size[i] as usize;
    let plane_size = fs.plane_residual_size(mi_size, plane);
    let skip = mi.skip[i];
    let is_intra = mi.ref_frame[i][0] <= INTRA_FRAME as i8;
    let is_block_edge = if pass == 0 {
        xp.is_multiple_of(4 * NUM_4X4_BLOCKS_WIDE[plane_size] as usize)
    } else {
        yp.is_multiple_of(4 * NUM_4X4_BLOCKS_HIGH[plane_size] as usize)
    };
    if !(is_block_edge || !skip || is_intra) {
        return;
    }
    let prev_tx_sz = fs.lf_tx_size(plane, prev_row >> sub_y, prev_col >> sub_x);
    // filter size (7.14.3)
    let base_size = if pass == 0 { TX_WIDTH[prev_tx_sz].min(TX_WIDTH[tx_sz]) } else { TX_HEIGHT[prev_tx_sz].min(TX_HEIGHT[tx_sz]) } as usize;
    let filter_size = if plane == 0 { 16.min(base_size) } else { 8.min(base_size) };
    let (mut lvl, mut limit, mut blimit, mut thresh) = strength(fs, row, col, plane, pass);
    if lvl == 0 {
        (lvl, limit, blimit, thresh) = strength(fs, prev_row, prev_col, plane, pass);
    }
    if lvl == 0 {
        return;
    }
    let t = LfThresh::new(limit, blimit, thresh, fs.bit_depth);
    let stride = pl.stride;
    let (step, along) = if pass == 0 { (1, stride) } else { (stride, 1) };
    let Some(base) = yp.checked_sub(pl.y0).map(|r| r * stride + xp) else {
        return;
    };
    // The filter reads `reach` samples before the edge and reach - 1 after it, on 4 lines.
    let reach = if filter_size >= 16 { 7 } else { filter_size / 2 };
    if base < reach * step || base + 3 * along + reach * step > pl.data.len() {
        return;
    }
    let chroma = plane > 0;
    for k in 0..4 {
        sample_filter(pl.data, base + k * along, step, chroma, filter_size, t);
    }
}

/// Adaptive filter strength (7.14.4): (lvl, limit, blimit, thresh).
fn strength(fs: &FrameState, row: usize, col: usize, plane: usize, pass: usize) -> (i32, i32, i32, i32) {
    let mi = &fs.mi;
    let i = mi.idx(row, col);
    let segment = mi.segment_id[i] as usize;
    let rf = mi.ref_frame[i][0];
    let mode = mi.y_mode[i] as usize;
    let mode_type = (mode >= NEARESTMV && mode != GLOBALMV && mode != GLOBAL_GLOBALMV) as usize;
    let delta_lf = if fs.fh.delta_lf_multi { mi.delta_lf[i][if plane == 0 { pass } else { plane + 1 }] } else { mi.delta_lf[i][0] } as i32;
    // 7.14.5
    let idx = if plane == 0 { pass } else { plane + 1 };
    let base = (delta_lf + fs.fh.lf.level[idx] as i32).clamp(0, MAX_LOOP_FILTER as i32);
    let mut lvl_seg = base;
    let feature = SEG_LVL_ALT_LF_Y_V + idx;
    if fs.fh.seg.enabled && fs.fh.seg.features.enabled[segment][feature] {
        lvl_seg = (fs.fh.seg.features.data[segment][feature] + lvl_seg).clamp(0, MAX_LOOP_FILTER as i32);
    }
    if fs.fh.lf.delta_enabled {
        let n_shift = lvl_seg >> 5;
        let d = &fs.fh.lf.deltas;
        if rf == INTRA_FRAME as i8 {
            lvl_seg += d.ref_deltas[INTRA_FRAME] << n_shift;
        } else if rf > INTRA_FRAME as i8 {
            lvl_seg += (d.ref_deltas[rf as usize] << n_shift) + (d.mode_deltas[mode_type] << n_shift);
        }
        lvl_seg = lvl_seg.clamp(0, MAX_LOOP_FILTER as i32);
    }
    let lvl = lvl_seg;
    let sharp = fs.fh.lf.sharpness as i32;
    let shift = if sharp > 4 {
        2
    } else if sharp > 0 {
        1
    } else {
        0
    };
    let limit = if sharp > 0 { (lvl >> shift).clamp(1, 9 - sharp) } else { (lvl >> shift).max(1) };
    let blimit = 2 * (lvl + 2) + limit;
    let thresh = lvl >> 4;
    (lvl, limit, blimit, thresh)
}

/// Per-edge thresholds of the sample filter, scaled to the bit depth.
#[derive(Clone, Copy)]
struct LfThresh {
    limit: i32,
    blimit: i32,
    thresh: i32,
    /// Flatness threshold 1 << (BitDepth - 8).
    flat: i32,
    /// 0x80 << (BitDepth - 8) and 1 << (BitDepth - 1).
    off: i32,
    half: i32,
}

impl LfThresh {
    fn new(limit: i32, blimit: i32, thresh: i32, bd: u32) -> LfThresh {
        let sh = bd - 8;
        LfThresh { limit: limit << sh, blimit: blimit << sh, thresh: thresh << sh, flat: 1 << sh, off: 0x80 << sh, half: 1 << (bd - 1) }
    }
}

/// Sample filtering process (7.14.6) for one line across the edge: `base` indexes q0 and
/// `step` is the distance between samples across the edge.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn sample_filter(d: &mut [u16], base: usize, step: usize, chroma: bool, filter_size: usize, t: LfThresh) {
    let at = |d: &[u16], k: isize| -> i32 { d[(base as isize + k * step as isize) as usize] as i32 };
    let (q0, q1, p0, p1) = (at(d, 0), at(d, 1), at(d, -1), at(d, -2));
    let lim = t.limit;
    // filter mask (7.14.6.2)
    if (p1 - p0).abs() > lim || (q1 - q0).abs() > lim || (p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 > t.blimit {
        return;
    }
    let hev = (p1 - p0).abs() > t.thresh || (q1 - q0).abs() > t.thresh;
    let mut flat = false;
    if filter_size > 4 {
        let (q2, p2) = (at(d, 2), at(d, -3));
        if (p2 - p1).abs() > lim || (q2 - q1).abs() > lim {
            return;
        }
        let tf = t.flat;
        flat = (p1 - p0).abs() <= tf && (q1 - q0).abs() <= tf && (p2 - p0).abs() <= tf && (q2 - q0).abs() <= tf;
        if !chroma {
            // filterLen >= 8
            let (q3, p3) = (at(d, 3), at(d, -4));
            if (p3 - p2).abs() > lim || (q3 - q2).abs() > lim {
                return;
            }
            flat = flat && (p3 - p0).abs() <= tf && (q3 - q0).abs() <= tf;
        }
    }
    if !flat {
        // narrow filter (7.14.6.3)
        let c = |v: i32| v.clamp(-t.half, t.half - 1);
        let off = t.off;
        let (ps1, ps0, qs0, qs1) = (p1 - off, p0 - off, q0 - off, q1 - off);
        let mut filter = if hev { c(ps1 - qs1) } else { 0 };
        filter = c(filter + 3 * (qs0 - ps0));
        let filter1 = c(filter + 4) >> 3;
        let filter2 = c(filter + 3) >> 3;
        d[base] = (c(qs0 - filter1) + off) as u16;
        d[base - step] = (c(ps0 + filter2) + off) as u16;
        if !hev {
            let f = (filter1 + 1) >> 1;
            d[base + step] = (c(qs1 - f) + off) as u16;
            d[base - 2 * step] = (c(ps1 + f) + off) as u16;
        }
        return;
    }
    if chroma {
        wide::<2, 1, 3>(d, base, step);
    } else if filter_size == 8 {
        wide::<3, 0, 3>(d, base, step);
    } else {
        let tf = t.flat;
        let q0 = at(d, 0);
        let p0 = at(d, -1);
        let flat2 = (4..7).all(|k| (at(d, k) - q0).abs() <= tf && (at(d, -k - 1) - p0).abs() <= tf);
        if flat2 { wide::<6, 1, 4>(d, base, step) } else { wide::<3, 0, 3>(d, base, step) }
    }
}

/// Wide filter process (7.14.6.4) with n = N, the 2-weight taps |j| <= N2 and log2Size = LOG2.
#[inline(always)]
fn wide<const N: usize, const N2: usize, const LOG2: u32>(d: &mut [u16], base: usize, step: usize) {
    // v[k] = sample at offset k - (N + 1) from q0, k in 0 .. 2N + 2
    let mut v = [0i32; 14];
    let first = base - (N + 1) * step;
    for (k, x) in v[..2 * N + 2].iter_mut().enumerate() {
        *x = d[first + k * step] as i32;
    }
    // e[m] = v[clamp(m - N, 0, 2N + 1)]: the clamped tap positions as one array, so
    // F[i] = sum_{jj = 0}^{2N} e[i + 1 + jj] * (2 if |jj - N| <= N2 else 1), a sliding window.
    let mut e = [0i32; 25];
    for (m, x) in e[..4 * N + 1].iter_mut().enumerate() {
        *x = v[m.saturating_sub(N).min(2 * N + 1)];
    }
    let mut f = [0i32; 12];
    let mut win: i32 = e[1..2 * N + 2].iter().sum();
    for i in 0..2 * N {
        if i > 0 {
            win += e[i + 1 + 2 * N] - e[i];
        }
        let centre: i32 = e[i + 1 + N - N2..=i + 1 + N + N2].iter().sum();
        f[i] = (win + centre + (1 << (LOG2 - 1))) >> LOG2;
    }
    let out = base - N * step;
    for i in 0..2 * N {
        d[out + i * step] = f[i] as u16;
    }
}
