//! Image files for the artwork tests, made in code rather than committed as
//! binaries: each fixture says what it is, and a hostile one is a few lines
//! of header rather than an opaque blob.

#![allow(dead_code)]

use base64::Engine;
use image::codecs::bmp::BmpEncoder;
use image::codecs::gif::GifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder};
use relabel_render::artwork::Artwork;

pub fn rgba_px(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&f(x, y));
        }
    }
    px
}

pub fn png_rgba(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&rgba_px(w, h, f), w, h, ExtendedColorType::Rgba8)
        .unwrap();
    out
}

pub fn png_l8(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Vec<u8> {
    let px: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| f(x, y))
        .collect();
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&px, w, h, ExtendedColorType::L8)
        .unwrap();
    out
}

/// A grey PNG carrying an EXIF orientation, 1 to 8, as a big-endian TIFF with
/// one tag - which is what a PNG `eXIf` chunk holds.
pub fn png_with_exif(w: u32, h: u32, f: impl Fn(u32, u32) -> u8, orientation: u8) -> Vec<u8> {
    let px: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| f(x, y))
        .collect();
    let exif = vec![
        b'M',
        b'M',
        0,
        42,
        0,
        0,
        0,
        8, // header, first IFD at 8
        0,
        1, // one entry
        0x01,
        0x12,
        0,
        3,
        0,
        0,
        0,
        1,
        0,
        orientation,
        0,
        0, // Orientation, SHORT, 1
        0,
        0,
        0,
        0, // no next IFD
    ];
    let mut out = Vec::new();
    let mut enc = PngEncoder::new(&mut out);
    enc.set_exif_metadata(exif).unwrap();
    enc.write_image(&px, w, h, ExtendedColorType::L8).unwrap();
    out
}

pub fn png_with_icc(w: u32, h: u32) -> Vec<u8> {
    let px = vec![0u8; (w * h) as usize];
    let mut out = Vec::new();
    let mut enc = PngEncoder::new(&mut out);
    enc.set_icc_profile(vec![7u8; 128]).unwrap();
    enc.write_image(&px, w, h, ExtendedColorType::L8).unwrap();
    out
}

pub fn crc32(d: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in d {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

pub fn chunk(out: &mut Vec<u8>, ty: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut t = ty.to_vec();
    t.extend_from_slice(data);
    out.extend_from_slice(&t);
    out.extend_from_slice(&crc32(&t).to_be_bytes());
}

/// A one-frame animated PNG: an ordinary PNG with an `acTL` chunk after its
/// header and an `fcTL` before its data, so the default image is the frame.
pub fn apng(w: u32, h: u32) -> Vec<u8> {
    let plain = png_l8(w, h, |_, _| 0);
    let mut out = plain[..8].to_vec();
    let mut i = 8;
    while i < plain.len() {
        let len = u32::from_be_bytes(plain[i..i + 4].try_into().unwrap()) as usize;
        let ty: [u8; 4] = plain[i + 4..i + 8].try_into().unwrap();
        let end = i + 12 + len;
        if &ty == b"IDAT" && !out.windows(4).any(|x| x == b"fcTL") {
            let mut fctl = Vec::new();
            for v in [0u32, w, h, 0, 0] {
                fctl.extend_from_slice(&v.to_be_bytes());
            }
            fctl.extend_from_slice(&[0, 1, 0, 1, 0, 0]);
            chunk(&mut out, b"fcTL", &fctl);
        }
        out.extend_from_slice(&plain[i..end]);
        if &ty == b"IHDR" {
            let mut actl = 1u32.to_be_bytes().to_vec();
            actl.extend_from_slice(&0u32.to_be_bytes());
            chunk(&mut out, b"acTL", &actl);
        }
        i = end;
    }
    out
}

pub fn jpeg_rgb(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let px: Vec<u8> = rgba_px(w, h, |x, y| {
        let [r, g, b] = f(x, y);
        [r, g, b, 255]
    })
    .chunks_exact(4)
    .flat_map(|p| [p[0], p[1], p[2]])
    .collect();
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 95)
        .write_image(&px, w, h, ExtendedColorType::Rgb8)
        .unwrap();
    out
}

pub fn bmp_rgb(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let px: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .flat_map(|(x, y)| f(x, y))
        .collect();
    let mut out = Vec::new();
    BmpEncoder::new(&mut out)
        .encode(&px, w, h, ExtendedColorType::Rgb8)
        .unwrap();
    out
}

pub fn gif_rgba(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    GifEncoder::new(&mut out)
        .encode(&rgba_px(w, h, f), w, h, ExtendedColorType::Rgba8)
        .unwrap();
    out
}

pub fn webp_lossless(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .encode(&rgba_px(w, h, f), w, h, ExtendedColorType::Rgba8)
        .unwrap();
    out
}

/// A PNG that is all header: its size is whatever it claims, and its data is
/// an empty zlib stream.
pub fn png_bomb(w: u32, h: u32, depth: u8, colour_type: u8) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[depth, colour_type, 0, 0, 0]);
    chunk(&mut v, b"IHDR", &ihdr);
    chunk(
        &mut v,
        b"IDAT",
        &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
    );
    chunk(&mut v, b"IEND", &[]);
    v
}

/// A 24-bit BMP header claiming `w` x `h`, followed by a few bytes of pixels.
pub fn bmp_bomb(w: i32, h: i32) -> Vec<u8> {
    let mut v = b"BM".to_vec();
    v.extend_from_slice(&70u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&54u32.to_le_bytes());
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&w.to_le_bytes());
    v.extend_from_slice(&h.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&24u16.to_le_bytes());
    for _ in 0..6 {
        v.extend_from_slice(&0u32.to_le_bytes());
    }
    v.extend_from_slice(&[0; 16]);
    v
}

/// A GIF whose logical screen is `w` x `h`, holding one 1 x 1 image.
pub fn gif_bomb(w: u16, h: u16) -> Vec<u8> {
    let mut v = b"GIF89a".to_vec();
    v.extend_from_slice(&w.to_le_bytes());
    v.extend_from_slice(&h.to_le_bytes());
    v.extend_from_slice(&[0, 0, 0]);
    v.push(0x2C);
    v.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0, 0]);
    v.extend_from_slice(&[2, 2, 0x4C, 0x01, 0]);
    v.push(0x3B);
    v
}

/// A small JPEG whose frame header has been rewritten to claim `w` x `h`.
pub fn jpeg_bomb(w: u16, h: u16) -> Vec<u8> {
    let mut v = jpeg_rgb(8, 8, |_, _| [0, 0, 0]);
    let at = v
        .windows(2)
        .position(|m| m == [0xFF, 0xC0])
        .expect("a baseline frame header");
    v[at + 5..at + 7].copy_from_slice(&h.to_be_bytes());
    v[at + 7..at + 9].copy_from_slice(&w.to_be_bytes());
    v
}

/// Artwork as an image element stores it.
pub fn stored(art: &Artwork) -> String {
    base64::engine::general_purpose::STANDARD.encode(art.to_png().unwrap())
}

/// Artwork from a function of the dot.
pub fn art(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Artwork {
    Artwork {
        w,
        h,
        px: (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| f(x, y))
            .collect(),
    }
}

/// A one-frame animated WebP: an extended header with the animation flag,
/// an animation chunk, and one frame around a lossless still.
pub fn webp_animated(w: u32, h: u32) -> Vec<u8> {
    fn riff_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(tag);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
    }
    let u24 = |v: u32| v.to_le_bytes()[..3].to_vec();
    let still = webp_lossless(w, h, |_, _| [0, 0, 0, 255]);
    let frame = &still[12..];
    let mut body = b"WEBP".to_vec();
    let mut vp8x = vec![0x02, 0, 0, 0];
    vp8x.extend(u24(w - 1));
    vp8x.extend(u24(h - 1));
    riff_chunk(&mut body, b"VP8X", &vp8x);
    riff_chunk(&mut body, b"ANIM", &[255, 255, 255, 255, 0, 0]);
    let mut anmf = [u24(0), u24(0), u24(w - 1), u24(h - 1), u24(100)].concat();
    anmf.push(0);
    anmf.extend_from_slice(frame);
    riff_chunk(&mut body, b"ANMF", &anmf);
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

/// A small JPEG whose frame header says progressive and claims `w` x `h`.
pub fn progressive_jpeg_bomb(w: u16, h: u16) -> Vec<u8> {
    let mut v = jpeg_bomb(w, h);
    let at = v.windows(2).position(|m| m == [0xFF, 0xC0]).unwrap();
    v[at + 1] = 0xC2;
    v
}
