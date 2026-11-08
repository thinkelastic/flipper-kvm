//! JPEG helpers for the stream.
//!
//! UVC capture devices send MJPEG frames without the Huffman tables (no DHT segment),
//! relying on the standard ones from the JPEG spec (Annex K.3). Browsers mostly cope,
//! but not all, so the tables are put back in before a frame goes out.

/// Annex K.3: bits (code counts per length) and values for the four standard tables.
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

/// One DHT segment carrying all four standard tables.
fn standard_dht() -> Vec<u8> {
    let tables: [(u8, &[u8; 16], &[u8]); 4] = [
        (0x00, &DC_LUMA_BITS, &DC_VALUES),
        (0x10, &AC_LUMA_BITS, &AC_LUMA_VALUES),
        (0x01, &DC_CHROMA_BITS, &DC_VALUES),
        (0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALUES),
    ];
    let mut body = Vec::new();
    for (class_id, bits, values) in tables {
        body.push(class_id);
        body.extend_from_slice(bits);
        body.extend_from_slice(values);
    }
    let mut seg = vec![0xFF, 0xC4];
    seg.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
    seg.extend_from_slice(&body);
    seg
}

/// The frame with the standard Huffman tables added if it has none. Markers are walked
/// up to the start of scan; anything that does not look like a JPEG is passed as is.
pub fn with_huffman_tables(jpeg: &[u8]) -> Vec<u8> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return jpeg.to_vec();
    }
    let mut i = 2;
    while i + 4 <= jpeg.len() && jpeg[i] == 0xFF {
        let marker = jpeg[i + 1];
        match marker {
            0xC4 => return jpeg.to_vec(), // has tables
            0xDA => {
                // Start of scan without tables: put them right before it.
                let mut out = Vec::with_capacity(jpeg.len() + 432);
                out.extend_from_slice(&jpeg[..i]);
                out.extend_from_slice(&standard_dht());
                out.extend_from_slice(&jpeg[i..]);
                return out;
            }
            0xD8 | 0x01 | 0xD0..=0xD7 => i += 2,
            _ => i += 2 + usize::from(u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]])),
        }
    }
    jpeg.to_vec()
}

/// Encode RGB (or YCbCr) pixels as a 4:2:0 JPEG.
pub fn encode(pixels: &[u8], width: u32, height: u32, color: jpeg_encoder::ColorType, quality: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() / 10);
    let mut enc = jpeg_encoder::Encoder::new(&mut out, quality);
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_2_0);
    if enc.encode(pixels, width as u16, height as u16, color).is_err() {
        out.clear();
    }
    out
}

/// YUYV (4:2:2, Y0 U Y1 V) to interleaved 4:4:4 YCbCr, which the encoder takes directly.
pub fn yuyv_to_ycbcr(yuyv: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(width * height * 3);
    for row in 0..height {
        let line = &yuyv[row * stride..row * stride + width * 2];
        for px in line.chunks_exact(4) {
            let (y0, u, y1, v) = (px[0], px[1], px[2], px[3]);
            out.extend_from_slice(&[y0, u, v, y1, u, v]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every DHT segment in a JPEG, as its body bytes.
    fn dht_bodies(jpeg: &[u8]) -> Vec<u8> {
        let mut i = 2;
        let mut out = Vec::new();
        while i + 4 <= jpeg.len() && jpeg[i] == 0xFF && jpeg[i + 1] != 0xDA {
            let len = usize::from(u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]));
            if jpeg[i + 1] == 0xC4 {
                out.extend_from_slice(&jpeg[i + 4..i + 2 + len]);
            }
            i += 2 + len;
        }
        out
    }

    #[test]
    fn table_sizes_match_their_counts() {
        for (bits, n) in [
            (DC_LUMA_BITS, DC_VALUES.len()),
            (DC_CHROMA_BITS, DC_VALUES.len()),
            (AC_LUMA_BITS, AC_LUMA_VALUES.len()),
            (AC_CHROMA_BITS, AC_CHROMA_VALUES.len()),
        ] {
            assert_eq!(bits.iter().map(|&b| usize::from(b)).sum::<usize>(), n);
        }
    }

    /// The encoder writes the same standard tables, so a frame with its DHT taken out
    /// and ours put back must carry exactly the tables it started with.
    #[test]
    fn our_tables_are_the_encoders_standard_tables() {
        let rgb = vec![128u8; 64 * 32 * 3];
        let jpeg = encode(&rgb, 64, 32, jpeg_encoder::ColorType::Rgb, 80);
        let mut want: Vec<Vec<u8>> = split_tables(&dht_bodies(&jpeg));
        let mut got: Vec<Vec<u8>> = split_tables(&dht_bodies(&standard_dht_frame()));
        want.sort();
        got.sort();
        assert_eq!(got, want);
    }

    fn standard_dht_frame() -> Vec<u8> {
        let mut f = vec![0xFF, 0xD8];
        f.extend_from_slice(&standard_dht());
        f.extend_from_slice(&[0xFF, 0xDA, 0, 2]);
        f
    }

    /// A DHT body split into its tables (class/id byte, 16 counts, values).
    fn split_tables(mut body: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while body.len() >= 17 {
            let n: usize = body[1..17].iter().map(|&b| usize::from(b)).sum();
            out.push(body[..17 + n].to_vec());
            body = &body[17 + n..];
        }
        out
    }

    #[test]
    fn tables_go_in_once_before_the_scan() {
        let rgb = vec![200u8; 16 * 16 * 3];
        let full = encode(&rgb, 16, 16, jpeg_encoder::ColorType::Rgb, 80);
        assert_eq!(with_huffman_tables(&full), full, "a frame with tables is left alone");

        // Take the tables out, as a UVC camera sends it.
        let mut stripped = full[..2].to_vec();
        let mut i = 2;
        while full[i + 1] != 0xDA {
            let len = usize::from(u16::from_be_bytes([full[i + 2], full[i + 3]]));
            if full[i + 1] != 0xC4 {
                stripped.extend_from_slice(&full[i..i + 2 + len]);
            }
            i += 2 + len;
        }
        stripped.extend_from_slice(&full[i..]);
        assert!(dht_bodies(&stripped).is_empty());

        let fixed = with_huffman_tables(&stripped);
        assert_eq!(fixed.len(), stripped.len() + standard_dht().len());
        assert!(!dht_bodies(&fixed).is_empty());
    }

    #[test]
    fn yuyv_expands_chroma() {
        let px = yuyv_to_ycbcr(&[10, 20, 30, 40], 2, 1, 4);
        assert_eq!(px, vec![10, 20, 40, 30, 20, 40]);
    }
}
