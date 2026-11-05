use super::*;

const GRAY: u16 = 1;
const RGB: u16 = 3;

/// PackBits as Photoshop writes it: runs of 3+ equal bytes repeated, the rest literal.
fn packbits(row: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < row.len() {
        let run = row[i..].iter().take(128).take_while(|&&b| b == row[i]).count();
        if run >= 3 {
            out.extend([(1 - run as i16) as u8, row[i]]);
            i += run;
        } else {
            let mut j = i + 1;
            while j < row.len() && j - i < 128 && !(j + 2 < row.len() && row[j] == row[j + 1] && row[j] == row[j + 2]) {
                j += 1;
            }
            out.push((j - i - 1) as u8);
            out.extend_from_slice(&row[i..j]);
            i = j;
        }
    }
    out
}

/// A minimal document: header, an empty colour mode section, one image resource, the given layer
/// and mask section, then the composite (`planes` are whole channels, top row first).
fn psd(mode: u16, depth: u16, w: u32, h: u32, planes: &[Vec<u16>], rle: bool, layers: &[u8]) -> Vec<u8> {
    let mut v = b"8BPS".to_vec();
    v.extend(1u16.to_be_bytes());
    v.extend([0; 6]);
    v.extend((planes.len() as u16).to_be_bytes());
    v.extend(h.to_be_bytes());
    v.extend(w.to_be_bytes());
    v.extend(depth.to_be_bytes());
    v.extend(mode.to_be_bytes());
    v.extend(0u32.to_be_bytes());
    let res = b"8BIM\x04\x0c\0\0\0\0\0\x02ab";
    v.extend((res.len() as u32).to_be_bytes());
    v.extend(res);
    v.extend((layers.len() as u32).to_be_bytes());
    v.extend(layers);
    v.extend(u16::from(rle).to_be_bytes());
    let rows: Vec<Vec<u8>> = planes
        .iter()
        .flat_map(|p| {
            p.chunks(w.max(1) as usize).map(|r| r.iter().flat_map(|&s| if depth == 8 { vec![s as u8] } else { s.to_be_bytes().to_vec() }).collect::<Vec<u8>>())
        })
        .collect();
    if rle {
        let packed: Vec<Vec<u8>> = rows.iter().map(|r| packbits(r)).collect();
        for p in &packed {
            v.extend((p.len() as u16).to_be_bytes());
        }
        packed.iter().for_each(|p| v.extend(p));
    } else {
        rows.iter().for_each(|r| v.extend(r));
    }
    v
}

/// A layer and mask section whose layer info holds only a layer count.
fn layer_count(n: i16) -> Vec<u8> {
    let mut v = 2u32.to_be_bytes().to_vec();
    v.extend(n.to_be_bytes());
    v.extend(0u32.to_be_bytes());
    v
}

/// The 16-bit form: an empty layer info, an empty global mask, then the layers in an `Lr16` block.
fn lr16_count(n: i16) -> Vec<u8> {
    let mut v = 0u32.to_be_bytes().to_vec();
    v.extend(0u32.to_be_bytes());
    v.extend(b"8BIMLr16");
    v.extend(2u32.to_be_bytes());
    v.extend(n.to_be_bytes());
    v
}

fn rgba(r: &Raster) -> Vec<[u8; 4]> {
    r.rgba.chunks(4).map(|p| [p[0], p[1], p[2], p[3]]).collect()
}

fn rgb_planes() -> Vec<Vec<u16>> {
    // 4 x 2, with a run long enough for PackBits to repeat
    vec![vec![10, 10, 10, 10, 1, 2, 3, 4], vec![20, 21, 22, 23, 5, 5, 5, 6], vec![30, 31, 32, 33, 7, 8, 9, 9]]
}

#[test]
fn rgb_8bit_raw_and_rle() {
    for rle in [false, true] {
        let r = decode("a.psd", &psd(RGB, 8, 4, 2, &rgb_planes(), rle, &[])).unwrap();
        assert_eq!((r.width, r.height, r.has_alpha), (4, 2, false), "rle {rle}");
        let p = rgba(&r);
        assert_eq!(p[0], [10, 20, 30, 255], "rle {rle}");
        assert_eq!(p[3], [10, 23, 33, 255], "rle {rle}");
        assert_eq!(p[4], [1, 5, 7, 255], "rle {rle}");
        assert_eq!(p[7], [4, 6, 9, 255], "rle {rle}");
    }
}

#[test]
fn gray_8bit_raw_and_rle() {
    for rle in [false, true] {
        let r = decode("a.psd", &psd(GRAY, 8, 3, 1, &[vec![0, 128, 255]], rle, &[])).unwrap();
        assert_eq!(rgba(&r), [[0, 0, 0, 255], [128, 128, 128, 255], [255, 255, 255, 255]], "rle {rle}");
    }
}

#[test]
fn sixteen_bit_is_rounded_to_eight() {
    // 257 * v is exactly v at 8 bits; 0x7fff and 0x8000 straddle the rounding point between 127 and 128
    let gray = vec![0, 0xffff, 257 * 100, 0x7fff, 0x8000];
    for rle in [false, true] {
        let r = decode("a.psd", &psd(GRAY, 16, 5, 1, std::slice::from_ref(&gray), rle, &[])).unwrap();
        let y: Vec<u8> = rgba(&r).iter().map(|p| p[0]).collect();
        assert_eq!(y, [0, 255, 100, 127, 128], "rle {rle}");
        let planes = [gray.clone(), gray.iter().map(|&s| 0xffff - s).collect(), vec![257 * 7; 5]];
        let r = decode("a.psd", &psd(RGB, 16, 5, 1, &planes, rle, &[])).unwrap();
        assert_eq!(rgba(&r)[2], [100, 155, 7, 255], "rle {rle}");
    }
}

#[test]
fn merged_alpha_only_when_the_layer_count_says_so() {
    let mut planes = rgb_planes();
    planes.push(vec![255, 0, 128, 255, 255, 255, 255, 255]);
    // a negative layer count: the first extra channel is the composite's transparency
    let r = decode("a.psd", &psd(RGB, 8, 4, 2, &planes, true, &layer_count(-2))).unwrap();
    assert!(r.has_alpha);
    let a: Vec<u8> = rgba(&r).iter().map(|p| p[3]).collect();
    assert_eq!(a, [255, 0, 128, 255, 255, 255, 255, 255]);
    // a flat document (no layer section) or a positive count: the extra channel is a saved selection
    for layers in [vec![], layer_count(2)] {
        let r = decode("a.psd", &psd(RGB, 8, 4, 2, &planes, false, &layers)).unwrap();
        assert!(!r.has_alpha);
        assert!(rgba(&r).iter().all(|p| p[3] == 255));
        assert_eq!(rgba(&r)[1], [10, 21, 31, 255]);
    }
    // 16-bit documents keep their layers in an Lr16 block; grayscale has its alpha second
    // (the second pixel is 191 at 64/255 opacity, stored matted on white as 239)
    let gray = [vec![257 * 9, 257 * 239], vec![0xffff, 257 * 64]];
    let r = decode("a.psd", &psd(GRAY, 16, 2, 1, &gray, false, &lr16_count(-1))).unwrap();
    assert_eq!(rgba(&r), [[9, 9, 9, 255], [191, 191, 191, 64]]);
    let r = decode("a.psd", &psd(GRAY, 16, 2, 1, &gray, false, &lr16_count(1))).unwrap();
    assert_eq!(rgba(&r), [[9, 9, 9, 255], [239, 239, 239, 255]]);
}

/// Global tagged blocks pad their data to 4 bytes: a 6-byte block before `Lr16` takes 8.
fn padded_lr16_count(n: i16) -> Vec<u8> {
    let mut v = 0u32.to_be_bytes().to_vec();
    v.extend(0u32.to_be_bytes());
    v.extend(b"8BIMPatt");
    v.extend(6u32.to_be_bytes());
    v.extend([1, 2, 3, 4, 5, 6, 0, 0]);
    v.extend(b"8BIMLr16");
    v.extend(2u32.to_be_bytes());
    v.extend(n.to_be_bytes());
    v
}

#[test]
fn tagged_blocks_are_aligned_to_four_bytes() {
    let gray = [vec![0, 0], vec![0xffff, 0]];
    let r = decode("a.psd", &psd(GRAY, 16, 2, 1, &gray, false, &padded_lr16_count(-1))).unwrap();
    assert!(r.has_alpha);
    assert_eq!(rgba(&r)[1][3], 0);
}

#[test]
fn transparent_composite_is_unmatted_from_white() {
    // Photoshop stores the composite blended over white: (204, 115, 225) at alpha 140 is saved as
    // (227, 178, 239); fully transparent pixels are saved white and stay so
    for depth in [8u16, 16] {
        let k = if depth == 8 { 1 } else { 257 };
        let planes: Vec<Vec<u16>> =
            [[204u16, 227, 255], [115, 178, 255], [225, 239, 255], [255, 140, 0]].iter().map(|c| c.iter().map(|&v| v * k).collect()).collect();
        for rle in [false, true] {
            let r = decode("a.psd", &psd(RGB, depth, 3, 1, &planes, rle, &layer_count(-1))).unwrap();
            let p = rgba(&r);
            assert_eq!(p[0], [204, 115, 225, 255], "{depth} bit");
            assert_eq!(p[1], [204, 115, 226, 140], "{depth} bit");
            assert_eq!(p[2], [255, 255, 255, 0], "{depth} bit");
        }
    }
}

/// `file` with its image resources replaced by one version info resource (1057).
fn with_version_info(file: &[u8], real_merged_data: u8) -> Vec<u8> {
    let mut res = b"8BIM\x04\x21\0\0".to_vec();
    res.extend(5u32.to_be_bytes());
    res.extend([0, 0, 0, 1, real_merged_data, 0]);
    let old = u32::from_be_bytes(file[30..34].try_into().unwrap()) as usize;
    let mut v = file[..30].to_vec();
    v.extend((res.len() as u32).to_be_bytes());
    v.extend(res);
    v.extend(&file[34 + old..]);
    v
}

#[test]
fn documents_without_a_real_composite_are_refused() {
    let file = psd(RGB, 8, 4, 2, &rgb_planes(), true, &[]);
    assert_eq!(rgba(&decode("a.psd", &with_version_info(&file, 1)).unwrap())[0], [10, 20, 30, 255]);
    let msg = decode("a.psd", &with_version_info(&file, 0)).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(msg.contains("Maximize Compatibility"), "{msg}");
}

#[test]
fn unsupported_documents_say_why() {
    let msg = |b: &[u8]| decode("a.psd", b).err().map(|e| e.to_string()).unwrap_or_default();
    let ok = psd(RGB, 8, 4, 2, &rgb_planes(), false, &[]);
    let with = |at: usize, bytes: &[u8]| {
        let mut b = ok.clone();
        b[at..at + bytes.len()].copy_from_slice(bytes);
        b
    };
    // depths a mode cannot have
    assert!(msg(&with(22, &[0, 1])).contains("1-bit RGB"));
    let cmyk = with(24, &[0, 4]);
    let mut cmyk32 = cmyk.clone();
    cmyk32[22..24].copy_from_slice(&[0, 32]);
    assert!(msg(&cmyk32).contains("32-bit CMYK"));
    let mut indexed16 = with(24, &[0, 2]);
    indexed16[22..24].copy_from_slice(&[0, 16]);
    assert!(msg(&indexed16).contains("16-bit indexed"));
    assert!(msg(&with(22, &[0, 12])).contains("unknown bit depth"));
    assert!(msg(&with(24, &[0, 5])).contains("unknown colour mode"));
    // an indexed document needs its palette
    assert!(msg(&with(24, &[0, 2])).contains("palette"));
    // raw rows read as ZIP data are not a zlib stream
    let zip = ok.len() - 3 * 8 - 2;
    assert!(decode("a.psd", &with(zip, &[0, 2])).is_err());
    assert!(decode("a.psd", &with(zip, &[0, 3])).is_err());
    assert!(msg(&with(zip, &[0, 9])).contains("unknown compression"));
    assert!(msg(&with(4, &[0, 3])).contains("unknown version"));
}

#[test]
fn hostile_input_is_an_error_not_a_panic() {
    let rgb = |w: u32, h: u32| psd(RGB, 8, w, h, &rgb_planes(), false, &[]);
    assert!(decode("a.psd", &rgb(4, 2)).is_ok());
    assert!(decode("a.psd", &rgb(0, 2)).is_err());
    assert!(decode("a.psd", &rgb(30_001, 1)).is_err());
    assert!(decode("a.psd", &rgb(30_000, 30_000)).is_err());
    // a big but legal size whose data is not there is refused before the picture is allocated
    assert!(decode("a.psd", &rgb(16_000, 16_000)).is_err());
    assert!(decode("a.psd", &psd(RGB, 8, 4, 2, &rgb_planes()[..2], false, &[])).is_err());
    // an RLE row that decodes past the row's end
    let mut b = psd(GRAY, 8, 2, 1, &[vec![1, 2]], true, &[]);
    let n = b.len();
    b.splice(n - 3.., [0xfe, 7]);
    b.splice(n - 5..n - 3, 2u16.to_be_bytes());
    assert!(decode("a.psd", &b).is_err());
    // section lengths that point past the end
    let mut b = rgb(4, 2);
    b[26..30].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(decode("a.psd", &b).is_err());
    assert!(decode("a.psd", b"8BPS").is_err());
    assert!(decode("a.psd", b"GIF89a").is_err());

    let mut planes = rgb_planes();
    planes.push(vec![255, 0, 128, 255, 255, 255, 255, 255]);
    for file in [psd(RGB, 8, 4, 2, &planes, true, &layer_count(-2)), psd(GRAY, 16, 2, 1, &[vec![1, 2], vec![3, 4]], false, &lr16_count(-1))] {
        for cut in 0..file.len() {
            assert!(decode("a.psd", &file[..cut]).is_err(), "cut at {cut}");
        }
        for i in 0..file.len() {
            for v in [0x00, 0x7f, 0x80, 0xff] {
                let mut b = file.clone();
                b[i] = v;
                assert!(std::panic::catch_unwind(|| decode("a.psd", &b)).is_ok(), "byte {i} = {v:#x}");
            }
        }
    }
}

#[test]
fn opens_as_a_still() {
    let bytes = psd(RGB, 8, 4, 2, &rgb_planes(), true, &[]);
    assert!(crate::still::sniff(&bytes));
    assert!(crate::is_importable(std::path::Path::new("Poster.PSD")));
    let src = crate::open_bytes("a.psd", bytes.into(), &[]).unwrap();
    assert_eq!(src.info().kind, crate::MediaKind::Still);
    assert_eq!(src.info().video.as_ref().map(|v| (v.width, v.height, v.codec.as_str())), Some((4, 2, "PSD")));
}

/// A document built from whole channels of row bytes (top row first), in any mode, depth,
/// compression and version.
#[derive(Clone)]
struct Doc {
    mode: u16,
    depth: u16,
    w: u32,
    h: u32,
    planes: Vec<Vec<u8>>,
    compression: u16,
    psb: bool,
    mode_data: Vec<u8>,
    resources: Vec<u8>,
    layers: Vec<u8>,
}

impl Doc {
    fn new(mode: u16, depth: u16, w: u32, h: u32, planes: Vec<Vec<u8>>) -> Self {
        Doc { mode, depth, w, h, planes, compression: 0, psb: false, mode_data: vec![], resources: vec![], layers: vec![] }
    }

    fn row_bytes(&self) -> usize {
        if self.depth == 1 { (self.w as usize).div_ceil(8) } else { self.w as usize * usize::from(self.depth / 8) }
    }

    fn with(&self, f: impl FnOnce(&mut Doc)) -> Doc {
        let mut d = self.clone();
        f(&mut d);
        d
    }

    /// The image data section's payload (after the compression field).
    fn payload(&self) -> Vec<u8> {
        let rb = self.row_bytes().max(1);
        let rows: Vec<&[u8]> = self.planes.iter().flat_map(|p| p.chunks(rb)).collect();
        let mut v = Vec::new();
        match self.compression {
            1 => {
                let packed: Vec<Vec<u8>> = rows.iter().map(|r| packbits(r)).collect();
                for p in &packed {
                    if self.psb {
                        v.extend((p.len() as u32).to_be_bytes());
                    } else {
                        v.extend((p.len() as u16).to_be_bytes());
                    }
                }
                packed.iter().for_each(|p| v.extend(p));
            }
            2 => v.extend(miniz_oxide::deflate::compress_to_vec_zlib(&rows.concat(), 6)),
            3 => {
                let predicted: Vec<u8> = rows.iter().flat_map(|r| predict(r, self.depth)).collect();
                v.extend(miniz_oxide::deflate::compress_to_vec_zlib(&predicted, 6));
            }
            _ => rows.iter().for_each(|r| v.extend(*r)),
        }
        v
    }

    fn bytes(&self) -> Vec<u8> {
        let mut v = b"8BPS".to_vec();
        v.extend(if self.psb { 2u16 } else { 1 }.to_be_bytes());
        v.extend([0; 6]);
        v.extend((self.planes.len() as u16).to_be_bytes());
        v.extend(self.h.to_be_bytes());
        v.extend(self.w.to_be_bytes());
        v.extend(self.depth.to_be_bytes());
        v.extend(self.mode.to_be_bytes());
        for s in [&self.mode_data, &self.resources] {
            v.extend((s.len() as u32).to_be_bytes());
            v.extend(s);
        }
        if self.psb {
            v.extend((self.layers.len() as u64).to_be_bytes());
        } else {
            v.extend((self.layers.len() as u32).to_be_bytes());
        }
        v.extend(&self.layers);
        v.extend(self.compression.to_be_bytes());
        v.extend(self.payload());
        v
    }

    fn decode(&self) -> Raster {
        decode("a.psd", &self.bytes()).unwrap()
    }
}

/// ZIP prediction, the way Photoshop writes it: the inverse of `unpredict`.
fn predict(row: &[u8], depth: u16) -> Vec<u8> {
    if depth == 16 {
        let s: Vec<u16> = row.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return (0..s.len()).flat_map(|i| s[i].wrapping_sub(if i == 0 { 0 } else { s[i - 1] }).to_be_bytes()).collect();
    }
    let w = row.len() / 4;
    let shuffled: Vec<u8> = if depth == 32 { (0..4).flat_map(|k| (0..w).map(move |x| row[4 * x + k])).collect() } else { row.to_vec() };
    (0..shuffled.len()).map(|i| shuffled[i].wrapping_sub(if i == 0 { 0 } else { shuffled[i - 1] })).collect()
}

fn bytes16(s: &[u16]) -> Vec<u8> {
    s.iter().flat_map(|v| v.to_be_bytes()).collect()
}

fn bytes32(s: &[f32]) -> Vec<u8> {
    s.iter().flat_map(|v| v.to_be_bytes()).collect()
}

fn close(a: [u8; 4], b: [u8; 4], tolerance: u8) -> bool {
    a.iter().zip(b).all(|(&x, y)| x.abs_diff(y) <= tolerance)
}

fn rgb8() -> Doc {
    Doc::new(RGB, 8, 4, 2, rgb_planes().iter().map(|p| p.iter().map(|&v| v as u8).collect()).collect())
}

const BITMAP: u16 = 0;
const INDEXED: u16 = 2;
const CMYK: u16 = 4;
const MULTICHANNEL: u16 = 7;
const DUOTONE: u16 = 8;
const LAB: u16 = 9;

/// 4 x 1 CMYK, stored inverted: no ink, full cyan, half black, then some cyan with half black.
fn cmyk_planes() -> Vec<Vec<u8>> {
    vec![vec![255, 0, 255, 200], vec![255, 255, 255, 255], vec![255, 255, 255, 255], vec![255, 255, 128, 128]]
}

#[test]
fn cmyk_is_converted_without_a_profile() {
    let expect = [[255, 255, 255, 255], [0, 255, 255, 255], [128, 128, 128, 255], [100, 128, 128, 255]];
    for compression in [0, 1, 2, 3] {
        let r = Doc::new(CMYK, 8, 4, 1, cmyk_planes()).with(|d| d.compression = compression).decode();
        assert!(!r.has_alpha);
        assert_eq!(rgba(&r), expect, "compression {compression}");
    }
    let wide: Vec<Vec<u8>> = cmyk_planes().iter().map(|p| bytes16(&p.iter().map(|&v| u16::from(v) * 257).collect::<Vec<_>>())).collect();
    assert_eq!(rgba(&Doc::new(CMYK, 16, 4, 1, wide).decode()), expect);
}

#[test]
fn cmyk_fifth_channel_is_alpha_when_the_layer_count_says_so() {
    // a fully transparent pixel is stored as white (no ink); full cyan at alpha 128 is stored as
    // cyan blended over white: 0 * 128/255 + 255 * 127/255 = 127
    let planes = vec![vec![0, 255, 127], vec![255, 255, 255], vec![255, 255, 255], vec![255, 255, 255], vec![255, 0, 128]];
    let doc = Doc::new(CMYK, 8, 3, 1, planes);
    let r = doc.with(|d| d.layers = layer_count(-1)).decode();
    assert!(r.has_alpha);
    assert_eq!(rgba(&r), [[0, 255, 255, 255], [255, 255, 255, 0], [0, 255, 255, 128]]);
    // otherwise the fifth channel is a saved selection
    let r = doc.decode();
    assert!(!r.has_alpha);
    assert_eq!(rgba(&r)[2], [127, 255, 255, 255]);
}

#[test]
fn lab_is_converted_to_srgb() {
    // L* 0..100 over 0..255, a* and b* offset by 128; sRGB red is about (54.3, 80.8, 69.9) in D50 Lab,
    // and L* 50 neutral is sRGB 119
    let planes = vec![vec![255, 0, 138, 128], vec![128, 128, 209, 128], vec![128, 128, 198, 128]];
    for compression in [0, 1, 3] {
        let p = rgba(&Doc::new(LAB, 8, 4, 1, planes.clone()).with(|d| d.compression = compression).decode());
        assert!(close(p[0], [255, 255, 255, 255], 1), "{:?}", p[0]);
        assert_eq!(p[1], [0, 0, 0, 255]);
        assert!(close(p[2], [255, 0, 0, 255], 8), "{:?}", p[2]);
        assert!(close(p[3], [119, 119, 119, 255], 2), "{:?}", p[3]);
    }
    let wide = vec![bytes16(&[65_535, 0]), bytes16(&[32_768, 32_768]), bytes16(&[32_768, 32_768])];
    let p = rgba(&Doc::new(LAB, 16, 2, 1, wide).decode());
    assert!(close(p[0], [255, 255, 255, 255], 1), "{:?}", p[0]);
    assert_eq!(p[1], [0, 0, 0, 255]);
}

fn palette() -> Vec<u8> {
    let mut p = vec![0u8; 768];
    for (i, (r, g, b)) in [(10, 20, 30), (200, 100, 50), (1, 2, 3)].into_iter().enumerate() {
        p[i] = r;
        p[256 + i] = g;
        p[512 + i] = b;
    }
    p
}

#[test]
fn indexed_colour_uses_the_palette_and_its_transparency_index() {
    let doc = Doc::new(INDEXED, 8, 4, 1, vec![vec![0, 1, 2, 1]]).with(|d| d.mode_data = palette());
    for compression in [0, 1, 2] {
        let r = doc.with(|d| d.compression = compression).decode();
        assert!(!r.has_alpha);
        assert_eq!(rgba(&r), [[10, 20, 30, 255], [200, 100, 50, 255], [1, 2, 3, 255], [200, 100, 50, 255]]);
    }
    // transparency index resource 1047
    let mut res = b"8BIM\x04\x17\0\0".to_vec();
    res.extend(2u32.to_be_bytes());
    res.extend(1u16.to_be_bytes());
    let r = doc.with(|d| d.resources = res).decode();
    assert!(r.has_alpha);
    assert_eq!(rgba(&r).iter().map(|p| p[3]).collect::<Vec<_>>(), [255, 0, 255, 0]);
    // a short palette is refused
    assert!(decode("a.psd", &doc.with(|d| d.mode_data.truncate(767)).bytes()).is_err());
}

#[test]
fn bitmap_set_bits_are_black() {
    // 10 x 2: two bytes a row, the last 6 bits padding
    let doc = Doc::new(BITMAP, 1, 10, 2, vec![vec![0b1010_0000, 0b0100_0000, 0xff, 0xc0]]);
    for compression in [0, 1, 3] {
        let r = doc.with(|d| d.compression = compression).decode();
        let y: Vec<u8> = rgba(&r).iter().map(|p| p[0]).collect();
        assert_eq!(y[..10], [0, 255, 0, 255, 255, 255, 255, 255, 255, 0], "compression {compression}");
        assert!(y[10..].iter().all(|&v| v == 0));
        assert!(!r.has_alpha);
    }
}

#[test]
fn duotone_and_multichannel_read_their_first_channel_as_grayscale() {
    let r = Doc::new(DUOTONE, 8, 3, 1, vec![vec![0, 128, 255]]).with(|d| d.mode_data = vec![1, 2, 3, 4]).decode();
    assert_eq!(rgba(&r), [[0, 0, 0, 255], [128, 128, 128, 255], [255, 255, 255, 255]]);
    let r = Doc::new(MULTICHANNEL, 8, 3, 1, vec![vec![0, 128, 255], vec![9, 9, 9], vec![7, 7, 7]]).decode();
    assert_eq!(rgba(&r), [[0, 0, 0, 255], [128, 128, 128, 255], [255, 255, 255, 255]]);
    let r = Doc::new(MULTICHANNEL, 16, 1, 1, vec![bytes16(&[257 * 40]), bytes16(&[0])]).decode();
    assert_eq!(rgba(&r), [[40, 40, 40, 255]]);
}

#[test]
fn thirty_two_bit_is_linear_float() {
    let srgb = |v: f32| (filmcraft_color::linear_to_srgb(v) * 255.0 + 0.5) as u8;
    let gray = [0.0, 1.0, 0.5, 2.0, -1.0, f32::NAN, f32::INFINITY, 0.214];
    let expect: Vec<u8> = vec![0, 255, srgb(0.5), 255, 0, 0, 255, srgb(0.214)];
    for compression in [0, 1, 2, 3] {
        let r = Doc::new(GRAY, 32, 8, 1, vec![bytes32(&gray)]).with(|d| d.compression = compression).decode();
        assert_eq!(rgba(&r).iter().map(|p| p[0]).collect::<Vec<_>>(), expect, "compression {compression}");
    }
    // linear 0.214 is about half way in sRGB
    assert!(srgb(0.214).abs_diff(128) <= 1);
    let planes = vec![bytes32(&[1.0, 0.0]), bytes32(&[0.5, 0.0]), bytes32(&[0.0, 0.0]), bytes32(&[1.0, 0.5])];
    let doc = Doc::new(RGB, 32, 2, 1, planes).with(|d| d.layers = layer_count(-1));
    for compression in [0, 3] {
        let p = rgba(&doc.with(|d| d.compression = compression).decode());
        assert_eq!(p[0], [255, srgb(0.5), 0, 255]);
        // alpha is not gamma-encoded
        assert_eq!(p[1][3], 128);
    }
}

#[test]
fn zip_matches_uncompressed_at_every_depth() {
    let docs = [
        Doc::new(RGB, 8, 5, 3, (0..3).map(|c| (0..15).map(|i| (i * 37 + c * 11) as u8).collect()).collect()),
        Doc::new(GRAY, 16, 5, 3, vec![bytes16(&(0..15).map(|i| (i * 4099) as u16).collect::<Vec<_>>())]),
        Doc::new(RGB, 32, 5, 3, (0..3).map(|c| bytes32(&(0..15).map(|i| (i + c) as f32 / 17.0).collect::<Vec<_>>())).collect()),
    ];
    for doc in docs {
        let raw = doc.decode();
        for compression in [1, 2, 3] {
            let r = doc.with(|d| d.compression = compression).decode();
            assert_eq!(r.rgba, raw.rgba, "depth {} compression {compression}", doc.depth);
        }
    }
}

#[test]
fn zip_may_hold_one_stream_per_channel() {
    let doc = rgb8();
    let raw = doc.decode();
    let zip = doc.with(|d| d.compression = 2);
    let mut file = zip.bytes();
    file.truncate(file.len() - zip.payload().len());
    for p in &doc.planes {
        file.extend(miniz_oxide::deflate::compress_to_vec_zlib(p, 6));
    }
    assert_eq!(decode("a.psd", &file).unwrap().rgba, raw.rgba);
}

/// The PSB form of a layer and mask section whose 16-bit layers say whether the composite has
/// alpha: an 8-byte empty layer info, an empty global mask, a block with a 4-byte length, then
/// `Lr16` with its 8-byte length.
fn psb_lr16_count(n: i16) -> Vec<u8> {
    let mut v = 0u64.to_be_bytes().to_vec();
    v.extend(0u32.to_be_bytes());
    v.extend(b"8BIMPatt");
    v.extend(4u32.to_be_bytes());
    v.extend([1, 2, 3, 4]);
    v.extend(b"8BIMLr16");
    v.extend(2u64.to_be_bytes());
    v.extend(n.to_be_bytes());
    v
}

/// The PSB form of a layer info that holds only a layer count (8-byte length).
fn psb_layer_count(n: i16) -> Vec<u8> {
    let mut v = 2u64.to_be_bytes().to_vec();
    v.extend(n.to_be_bytes());
    v.extend(0u32.to_be_bytes());
    v
}

#[test]
fn large_document_format() {
    let doc = rgb8().with(|d| d.psb = true);
    for compression in [0, 1, 2, 3] {
        let p = rgba(&doc.with(|d| d.compression = compression).decode());
        assert_eq!((p[0], p[7]), ([10, 20, 30, 255], [4, 6, 9, 255]), "compression {compression}");
    }
    let gray = vec![bytes16(&[257 * 9, 257 * 239]), bytes16(&[0xffff, 257 * 64])];
    let g = Doc::new(GRAY, 16, 2, 1, gray).with(|d| (d.psb, d.compression) = (true, 1));
    for layers in [psb_lr16_count(-1), psb_layer_count(-1)] {
        assert_eq!(rgba(&g.with(|d| d.layers = layers).decode()), [[9, 9, 9, 255], [191, 191, 191, 64]]);
    }
    for layers in [psb_lr16_count(1), psb_layer_count(1)] {
        assert!(!g.with(|d| d.layers = layers).decode().has_alpha);
    }
    // a PSB may be wider than a PSD, within the pixel cap
    let wide = Doc::new(GRAY, 8, 40_000, 1, vec![vec![7; 40_000]]).with(|d| (d.psb, d.compression) = (true, 1));
    assert_eq!(wide.decode().width, 40_000);
    assert!(decode("a.psd", &wide.with(|d| d.psb = false).bytes()).is_err());
}

#[test]
fn hostile_zip_and_new_modes_are_errors_not_panics() {
    let rgb = rgb8();
    // bad, short and garbage zlib streams
    let zip = rgb.with(|d| d.compression = 2);
    let file = zip.bytes();
    let data = file.len() - zip.payload().len();
    let mut bad = file.clone();
    bad[data + 3] ^= 0x55;
    assert!(decode("a.psd", &bad).is_err());
    let mut short = file[..data].to_vec();
    short.extend(miniz_oxide::deflate::compress_to_vec_zlib(&rgb.planes.concat()[..20], 6));
    assert!(decode("a.psd", &short).is_err());
    let mut garbage = file[..data].to_vec();
    garbage.extend([0xff; 40]);
    assert!(decode("a.psd", &garbage).is_err());
    // a huge picture whose ZIP data cannot hold it is refused before inflating
    let big = Doc::new(RGB, 8, 16_000, 16_000, vec![vec![]; 3]).with(|d| d.compression = 2);
    assert!(decode("a.psd", &big.bytes()).is_err());

    let cmyk_alpha: Vec<Vec<u8>> = cmyk_planes().into_iter().map(|p| p[..3].to_vec()).chain([vec![1, 2, 3]]).collect();
    let docs = [
        zip,
        rgb.with(|d| d.compression = 3),
        Doc::new(GRAY, 32, 3, 2, vec![bytes32(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6])]).with(|d| d.compression = 3),
        Doc::new(CMYK, 8, 3, 1, cmyk_alpha).with(|d| (d.layers, d.compression) = (layer_count(-1), 1)),
        Doc::new(INDEXED, 8, 4, 1, vec![vec![0, 1, 2, 1]]).with(|d| d.mode_data = palette()),
        Doc::new(BITMAP, 1, 10, 2, vec![vec![1, 2, 3, 4]]).with(|d| d.compression = 1),
        Doc::new(GRAY, 16, 2, 1, vec![bytes16(&[1, 2]), bytes16(&[3, 4])]).with(|d| (d.psb, d.layers, d.compression) = (true, psb_lr16_count(-1), 1)),
        Doc::new(LAB, 16, 2, 1, vec![bytes16(&[1, 2]), bytes16(&[3, 4]), bytes16(&[5, 6])]).with(|d| d.compression = 3),
    ];
    for doc in docs {
        let file = doc.bytes();
        assert!(decode("a.psd", &file).is_ok());
        // a zlib stream can give all the picture before its last bytes (the checksum, or channels
        // that are not read), so cutting those may still decode
        let zip = doc.compression >= 2;
        for cut in 0..file.len() {
            let r = std::panic::catch_unwind(|| decode("a.psd", &file[..cut]));
            assert!(r.is_ok(), "cut at {cut}");
            if !zip || cut + 8 < file.len() {
                assert!(r.is_ok_and(|r| r.is_err()), "cut at {cut}");
            }
        }
        for i in 0..file.len() {
            for v in [0x00, 0x01, 0x7f, 0x80, 0xff] {
                let mut b = file.clone();
                b[i] = v;
                assert!(std::panic::catch_unwind(|| decode("a.psd", &b)).is_ok(), "byte {i} = {v:#x}");
            }
        }
    }
}
