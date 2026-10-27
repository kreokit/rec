//! A small baseline JPEG encoder for the camera preview on Windows and Linux (macOS uses
//! the system's): 4:4:4, the example tables of the JPEG standard (ITU T.81, Annex K). Our own
//! so no crate with a licence outside MIT/Apache/BSD comes in.

#![cfg_attr(
    all(target_os = "macos", not(feature = "pipe-recorder")),
    allow(dead_code)
)]

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const LUMA_Q: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

const CHROMA_Q: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_VALUES: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
const AC_LUMA_VALUES: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
    0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALUES: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
    0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
    0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
    0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

/// A Huffman table: code and length per symbol.
struct Table([(u16, u8); 256]);

impl Table {
    fn new(bits: &[u8; 16], values: &[u8]) -> Table {
        let mut t = [(0u16, 0u8); 256];
        let (mut code, mut k) = (0u16, 0usize);
        for (len, count) in bits.iter().enumerate() {
            for _ in 0..*count {
                t[values[k] as usize] = (code, len as u8 + 1);
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        Table(t)
    }
}

struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn put(&mut self, value: u32, len: u32) {
        for i in (0..len).rev() {
            self.acc = self.acc << 1 | (value >> i) & 1;
            self.n += 1;
            if self.n == 8 {
                let byte = self.acc as u8;
                self.out.push(byte);
                if byte == 0xff {
                    self.out.push(0); // byte stuffing
                }
                self.acc = 0;
                self.n = 0;
            }
        }
    }

    fn flush(&mut self) {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put((1 << pad) - 1, pad);
        }
    }
}

fn quant_table(base: &[u8; 64], quality: u32) -> [u8; 64] {
    let q = quality.clamp(1, 100);
    let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };
    base.map(|v| ((v as u32 * scale + 50) / 100).clamp(1, 255) as u8)
}

/// Size category and bits of a coefficient.
fn category(v: i32) -> (u32, u32) {
    let size = 32 - v.unsigned_abs().leading_zeros();
    let bits = if v < 0 {
        (v - 1) as u32 & ((1 << size) - 1)
    } else {
        v as u32
    };
    (size, bits)
}

struct Component<'a> {
    q: [f32; 64],
    dc: &'a Table,
    ac: &'a Table,
    last_dc: i32,
}

fn encode_block(block: &[f32; 64], c: &mut Component, cos: &[[f32; 8]; 8], out: &mut Bits) {
    // Forward DCT, quantized, in zigzag order.
    let mut coeffs = [0i32; 64];
    for (k, &i) in ZIGZAG.iter().enumerate() {
        let (v, u) = (i / 8, i % 8);
        let mut sum = 0.0;
        for y in 0..8 {
            for x in 0..8 {
                sum += block[y * 8 + x] * cos[x][u] * cos[y][v];
            }
        }
        let cu = if u == 0 {
            std::f32::consts::FRAC_1_SQRT_2
        } else {
            1.0
        };
        let cv = if v == 0 {
            std::f32::consts::FRAC_1_SQRT_2
        } else {
            1.0
        };
        coeffs[k] = (0.25 * cu * cv * sum / c.q[i]).round() as i32;
    }
    let diff = coeffs[0] - c.last_dc;
    c.last_dc = coeffs[0];
    let (size, bits) = category(diff);
    let (code, len) = c.dc.0[size as usize];
    out.put(code as u32, len as u32);
    out.put(bits, size);
    let mut run = 0;
    for &v in &coeffs[1..] {
        if v == 0 {
            run += 1;
            continue;
        }
        while run >= 16 {
            let (code, len) = c.ac.0[0xf0];
            out.put(code as u32, len as u32);
            run -= 16;
        }
        let (size, bits) = category(v);
        let (code, len) = c.ac.0[(run << 4 | size) as usize];
        out.put(code as u32, len as u32);
        out.put(bits, size);
        run = 0;
    }
    if run > 0 {
        let (code, len) = c.ac.0[0x00];
        out.put(code as u32, len as u32);
    }
}

fn segment(out: &mut Vec<u8>, marker: u8, body: &[u8]) {
    out.extend([0xff, marker]);
    out.extend(((body.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(body);
}

/// Encodes `rgb` (`width` × `height`, 3 bytes a pixel) as a JPEG.
pub fn encode(rgb: &[u8], width: u32, height: u32, quality: u32) -> Vec<u8> {
    let (w, h) = (width.max(1) as usize, height.max(1) as usize);
    let lq = quant_table(&LUMA_Q, quality);
    let cq = quant_table(&CHROMA_Q, quality);
    let mut out = vec![0xff, 0xd8];
    segment(&mut out, 0xe0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    for (id, table) in [(0u8, &lq), (1, &cq)] {
        let mut body = vec![id];
        body.extend(ZIGZAG.iter().map(|&i| table[i]));
        segment(&mut out, 0xdb, &body);
    }
    let mut sof = vec![8];
    sof.extend((h as u16).to_be_bytes());
    sof.extend((w as u16).to_be_bytes());
    sof.extend([3, 1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1]);
    segment(&mut out, 0xc0, &sof);
    for (class, bits, values) in [
        (0x00, &DC_LUMA_BITS, &DC_VALUES[..]),
        (0x10, &AC_LUMA_BITS, &AC_LUMA_VALUES[..]),
        (0x01, &DC_CHROMA_BITS, &DC_VALUES[..]),
        (0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALUES[..]),
    ] {
        let mut body = vec![class];
        body.extend_from_slice(bits);
        body.extend_from_slice(values);
        segment(&mut out, 0xc4, &body);
    }
    segment(&mut out, 0xda, &[3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);

    let tables = [
        Table::new(&DC_LUMA_BITS, &DC_VALUES),
        Table::new(&AC_LUMA_BITS, &AC_LUMA_VALUES),
        Table::new(&DC_CHROMA_BITS, &DC_VALUES),
        Table::new(&AC_CHROMA_BITS, &AC_CHROMA_VALUES),
    ];
    let q = |t: &[u8; 64]| t.map(|v| v as f32);
    let mut comps = [
        Component {
            q: q(&lq),
            dc: &tables[0],
            ac: &tables[1],
            last_dc: 0,
        },
        Component {
            q: q(&cq),
            dc: &tables[2],
            ac: &tables[3],
            last_dc: 0,
        },
        Component {
            q: q(&cq),
            dc: &tables[2],
            ac: &tables[3],
            last_dc: 0,
        },
    ];
    let mut cos = [[0f32; 8]; 8];
    for (x, row) in cos.iter_mut().enumerate() {
        for (u, v) in row.iter_mut().enumerate() {
            *v = (((2 * x + 1) * u) as f32 * std::f32::consts::PI / 16.0).cos();
        }
    }
    let mut bits = Bits {
        out: Vec::new(),
        acc: 0,
        n: 0,
    };
    let mut blocks = [[0f32; 64]; 3];
    for by in (0..h).step_by(8) {
        for bx in (0..w).step_by(8) {
            for i in 0..64 {
                // Edge blocks repeat the last row and column.
                let x = (bx + i % 8).min(w - 1);
                let y = (by + i / 8).min(h - 1);
                let p = &rgb[(y * w + x) * 3..][..3];
                let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
                blocks[0][i] = 0.299 * r + 0.587 * g + 0.114 * b - 128.0;
                blocks[1][i] = -0.168_736 * r - 0.331_264 * g + 0.5 * b;
                blocks[2][i] = 0.5 * r - 0.418_688 * g - 0.081_312 * b;
            }
            for (block, comp) in blocks.iter().zip(comps.iter_mut()) {
                encode_block(block, comp, &cos, &mut bits);
            }
        }
    }
    bits.flush();
    out.extend_from_slice(&bits.out);
    out.extend([0xff, 0xd9]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tables_are_complete_prefix_codes() {
        for (bits, values) in [
            (&DC_LUMA_BITS, &DC_VALUES[..]),
            (&DC_CHROMA_BITS, &DC_VALUES[..]),
            (&AC_LUMA_BITS, &AC_LUMA_VALUES[..]),
            (&AC_CHROMA_BITS, &AC_CHROMA_VALUES[..]),
        ] {
            assert_eq!(
                bits.iter().map(|b| *b as usize).sum::<usize>(),
                values.len()
            );
            // Kraft: a prefix code, and no code of all ones.
            let kraft: f64 = bits
                .iter()
                .enumerate()
                .map(|(l, n)| *n as f64 / 2f64.powi(l as i32 + 1))
                .sum();
            assert!(kraft < 1.0);
        }
        // Every AC symbol the encoder may send: end of block, 16 zeros, runs 0-15 × sizes 1-10.
        for values in [&AC_LUMA_VALUES, &AC_CHROMA_VALUES] {
            let mut all: Vec<u8> = vec![0x00, 0xf0];
            all.extend((0..16u8).flat_map(|r| (1..=10u8).map(move |s| r << 4 | s)));
            let mut sorted = values.to_vec();
            sorted.sort();
            all.sort();
            assert_eq!(sorted, all);
        }
    }

    #[test]
    fn ffmpeg_reads_it_back() {
        // A colour gradient with a sharp edge, 37 × 21 (partial blocks).
        let (w, h) = (37u32, 21u32);
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let edge = if x > 20 { 200 } else { 30 };
                rgb.extend([(x * 6) as u8, (y * 10) as u8, edge]);
            }
        }
        let jpeg = encode(&rgb, w, h, 90);
        let dir = std::env::temp_dir().join("kreokit-rec-jpeg-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jpg");
        std::fs::write(&path, &jpeg).unwrap();
        let output = std::process::Command::new(crate::ffmpeg::locate("ffmpeg").unwrap())
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout.len(), rgb.len());
        let worst = rgb
            .iter()
            .zip(&output.stdout)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        let mean: f64 = rgb
            .iter()
            .zip(&output.stdout)
            .map(|(a, b)| a.abs_diff(*b) as f64)
            .sum::<f64>()
            / rgb.len() as f64;
        assert!(mean < 4.0 && worst < 80, "mean {mean}, worst {worst}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
