//! A minimal, honest PDF 1.4 writer.
//!
//! # Why raster-only
//!
//! The engine's one paint path is SVG, and PDF has no general SVG support. So a
//! `.pdf` export is *exactly* the same pixels as the `.png` export: one page per
//! artboard, each page holding that board's raster as an image XObject. The
//! "preview and export come from the same compiled SVG" promise therefore still
//! holds for PDF; the cost is that text is not selectable.
//!
//! # Why DEFLATE "stored" blocks
//!
//! `/Image` streams use `/FlateDecode`, i.e. a zlib-wrapped DEFLATE stream
//! (RFC 1950 + RFC 1951). `flate2` is not a direct dependency — it reaches us only
//! through `usvg`, which does not re-export it — and the `image` crate's PNG encoder
//! is no use here because a PDF image stream is raw samples, not a PNG file. So this
//! module emits DEFLATE *stored* blocks (type 00, uncompressed, with a real zlib
//! header and adler32 trailer): legal, trivially correct, accepted by every viewer.
//! The tradeoff is size — a 1080x1080 RGB page costs ~3.4 MB rather than the ~1 MB
//! Huffman coding would give — and that tradeoff is deliberate: the byte count the
//! caller reports is the true byte count of the file that was written, so the output
//! is honest even when it is fat. Real compression can be dropped in later without
//! changing this module's public shape.
//!
//! # Structure
//!
//! Objects go out in a fixed order (catalog, pages, then per page: page, content,
//! image; finally info) and each offset is recorded as its bytes are written, so the
//! xref table and `startxref` are exact. Non-ASCII strings go out as UTF-16BE hex
//! with a BOM, which is how PDF 1.4 viewers get Chinese titles.

use crate::error::{Fail, R};
use crate::util;
use serde_json::Value;

/// Pixels per point of PDF user space: 1 CSS px at 96 dpi = 72/96 = 0.75 pt.
pub const PX_TO_PT: f64 = 0.75;

/// Hard ceiling on one page's pixel count, matching `render::MAX_PIXELS`.
const MAX_PAGE_PIXELS: usize = 40_000_000;

/// Writes a PDF.
///
/// `pages` is `(width_px, height_px, samples)` per page; samples are RGB (3 bytes
/// per pixel) or RGBA (4 bytes, composited onto white because these image XObjects
/// carry no SMask). `meta` is a JSON object read for `title`/`name`, `author`,
/// `subject`/`description`, `keywords`/`origin`, `creator`, `producer`,
/// `created`/`created_at`/`date`/`updated_at` and `revision`.
pub fn write_pdf(pages: &[(u32, u32, Vec<u8>)], meta: &Value) -> R<Vec<u8>> {
    if pages.is_empty() {
        return Err(Fail::new("INVALID_VALUE", "pdf: no pages to write")
            .hint("render.export needs at least one visible artboard"));
    }
    for (i, (w, h, px)) in pages.iter().enumerate() {
        let need = *w as usize * *h as usize;
        if need == 0 {
            return Err(Fail::new(
                "INVALID_VALUE",
                format!("pdf: page {} has zero size", i + 1),
            ));
        }
        if px.len() != need * 3 && px.len() != need * 4 {
            return Err(Fail::new(
                "INVALID_VALUE",
                format!(
                    "pdf: page {} sample length {} is not {}x{} RGB or RGBA",
                    i + 1,
                    px.len(),
                    w,
                    h
                ),
            ));
        }
        if need > MAX_PAGE_PIXELS {
            return Err(Fail::new(
                "TOO_LARGE",
                format!("pdf: page {} is {}x{} pixels", i + 1, w, h),
            )
            .hint("lower the scale, or export one artboard at a time"));
        }
    }

    let n = pages.len();
    // Object numbering is flat and contiguous: 1 catalog, 2 pages, three objects
    // per page starting at 4, then the info dictionary.
    let first_page_obj = 4usize;
    let info_obj = first_page_obj + 3 * n;
    let obj_count = info_obj;

    let mut out: Vec<u8> = Vec::new();
    let mut offsets: Vec<u64> = vec![0; obj_count + 1];
    out.extend_from_slice(b"%PDF-1.4\n");
    // A comment with high bytes: tells readers this is not a text file.
    out.extend_from_slice(b"%\xe2\xe3\xcf\xd3\n");

    {
        let out = &mut out;
        let mut obj = |id: usize, bytes: &[u8]| {
            offsets[id] = out.len() as u64;
            out.extend_from_slice(format!("{} 0 obj\n", id).as_bytes());
            out.extend_from_slice(bytes);
            out.extend_from_slice(b"\nendobj\n");
        };

        let kids: Vec<String> = (0..n)
            .map(|i| format!("{} 0 R", first_page_obj + i * 3))
            .collect();
        obj(1, b"<< /Type /Catalog /Pages 2 0 R >>");
        obj(
            2,
            format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), n).as_bytes(),
        );

        for (i, (w, h, px)) in pages.iter().enumerate() {
            let page_id = first_page_obj + i * 3;
            let content_id = page_id + 1;
            let image_id = page_id + 2;
            let pw = num(*w as f64 * PX_TO_PT);
            let ph = num(*h as f64 * PX_TO_PT);

            obj(
                page_id,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] \
                     /Resources << /XObject << /Im{} {} 0 R >> /ProcSet [/PDF /ImageC] >> \
                     /Contents {} 0 R >>",
                    pw, ph, i, image_id, content_id
                )
                .as_bytes(),
            );

            // Content stream: paint the image across the whole page. A PDF image
            // XObject fills the unit square with its first sample row at the top,
            // so scaling by (pw, ph) in default (y-up) user space is upright.
            let stream = format!("q\n{} 0 0 {} 0 0 cm\n/Im{} Do\nQ\n", pw, ph, i);
            let mut cb = Vec::new();
            cb.extend_from_slice(format!("<< /Length {} >>\nstream\n", stream.len()).as_bytes());
            cb.extend_from_slice(stream.as_bytes());
            cb.extend_from_slice(b"\nendstream");
            obj(content_id, &cb);

            let rgb = to_rgb(*w, *h, px);
            let comp = deflate_stored(&rgb);
            let mut ib = Vec::new();
            ib.extend_from_slice(
                format!(
                    "<< /Type /XObject /Subtype /Image /Width {} /Height {} \
                     /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode \
                     /Interpolate true /Length {} >>\nstream\n",
                    w,
                    h,
                    comp.len()
                )
                .as_bytes(),
            );
            ib.extend_from_slice(&comp);
            ib.extend_from_slice(b"\nendstream");
            obj(image_id, &ib);
        }

        let mut info = String::from("<< ");
        if let Some(t) = text_of(meta, &["title", "name", "project"]) {
            info.push_str(&format!("/Title {} ", pdf_string(&t)));
        }
        if let Some(a) = text_of(meta, &["author"]) {
            info.push_str(&format!("/Author {} ", pdf_string(&a)));
        }
        if let Some(s) = text_of(meta, &["subject", "description"]) {
            info.push_str(&format!("/Subject {} ", pdf_string(&s)));
        }
        if let Some(k) = text_of(meta, &["keywords", "origin"]) {
            info.push_str(&format!("/Keywords {} ", pdf_string(&k)));
        }
        info.push_str(&format!(
            "/Creator {} ",
            pdf_string(&text_of(meta, &["creator"]).unwrap_or_else(|| "AgentCanvas".into()))
        ));
        info.push_str(&format!(
            "/Producer {} ",
            pdf_string(
                &text_of(meta, &["producer"])
                    .unwrap_or_else(|| format!("AgentCanvas core {}", env!("CARGO_PKG_VERSION")))
            )
        ));
        if let Some(d) = pdf_date(meta) {
            info.push_str(&format!("/CreationDate {} /ModDate {} ", d.clone(), d));
        }
        info.push_str(">>");
        obj(info_obj, info.as_bytes());
    }

    // Cross-reference table: every entry is exactly 20 bytes.
    let xref_at = out.len() as u64;
    out.extend_from_slice(format!("xref\n0 {}\n", obj_count + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for id in 1..=obj_count {
        out.extend_from_slice(format!("{:010} 00000 n \n", offsets[id]).as_bytes());
    }
    let id_hex = util::sha256_bytes(&out[..xref_at as usize]);
    let id_hex = &id_hex[..32];
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info {} 0 R /ID [<{}> <{}>] >>\nstartxref\n{}\n%%EOF\n",
            obj_count + 1,
            info_obj,
            id_hex,
            id_hex,
            xref_at
        )
        .as_bytes(),
    );

    Ok(out)
}

// ------------------------------------------------------------------ internals

/// PDF's DeviceRGB image has no alpha here, so composite onto white: a transparent
/// board then prints the way it would on paper.
fn to_rgb(w: u32, h: u32, px: &[u8]) -> Vec<u8> {
    let n = w as usize * h as usize;
    if px.len() == n * 3 {
        return px.to_vec();
    }
    let mut out = Vec::with_capacity(n * 3);
    for p in px[..n * 4].chunks_exact(4) {
        let a = p[3] as u32;
        if a == 255 {
            out.extend_from_slice(&p[..3]);
        } else if a == 0 {
            out.extend_from_slice(&[255, 255, 255]);
        } else {
            for i in 0..3 {
                let v = (p[i] as u32 * a + 255 * (255 - a)) / 255;
                out.push(v.min(255) as u8);
            }
        }
    }
    out
}

/// A zlib stream (RFC 1950) of DEFLATE stored blocks (RFC 1951 type 00).
fn deflate_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + (data.len() / 0xffff + 1) * 5 + 8);
    // CMF=0x78: CM 8 (deflate), CINFO 7 (32 KiB window). FLG=0x01: check bits make
    // (CMF<<8|FLG) a multiple of 31, FLEVEL=0, FDICT=0.
    out.extend_from_slice(&[0x78, 0x01]);
    let mut pos = 0usize;
    loop {
        let block = (data.len() - pos).min(0xffff);
        let last = pos + block >= data.len();
        out.push(if last { 1 } else { 0 });
        out.push((block & 0xff) as u8);
        out.push((block >> 8) as u8);
        let nl = !(block as u16);
        out.push((nl & 0xff) as u8);
        out.push((nl >> 8) as u8);
        if block > 0 {
            out.extend_from_slice(&data[pos..pos + block]);
            pos += block;
        }
        if last {
            break;
        }
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 is the largest run that cannot overflow the 16-bit accumulators.
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// A PDF real number with no trailing zeros: `810`, not `810.0000`.
fn num(v: f64) -> String {
    let r = {
        let m = 1e4;
        (v * m).round() / m
    };
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{}", r)
    }
}

fn text_of(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match v.get(*k)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// PDF text string. ASCII goes out as an escaped literal string; anything else
/// becomes a hex UTF-16BE string with a BOM.
fn pdf_string(s: &str) -> String {
    let plain = s.chars().all(|c| (0x20..0x7f).contains(&(c as u32)) || c == '\t');
    if plain {
        let mut out = String::from("(");
        for c in s.chars() {
            match c {
                '(' | ')' | '\\' => {
                    out.push('\\');
                    out.push(c);
                }
                '\t' => out.push_str("\\t"),
                _ => out.push(c),
            }
        }
        out.push(')');
        out
    } else {
        let mut hex = String::from("<FEFF");
        for u in s.encode_utf16() {
            hex.push_str(&format!("{:04X}", u));
        }
        hex.push('>');
        hex
    }
}

/// `D:YYYYMMDDHHMMSS+08'00'` from whatever date-ish field the document carries.
/// Engine stamps are `YYYY-MM-DD HH:MM` in UTC+8.
fn pdf_date(meta: &Value) -> Option<String> {
    let raw = text_of(meta, &["created", "created_at", "date", "updated_at"])?;
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 8 {
        return None;
    }
    let mut s = String::from("D:");
    s.push_str(&digits[..8]);
    if digits.len() >= 14 {
        s.push_str(&digits[8..14]);
    } else if digits.len() >= 12 {
        s.push_str(&digits[8..12]);
        s.push_str("00");
    } else {
        s.push_str("000000");
    }
    Some(format!("{}+08'00'", s))
}
