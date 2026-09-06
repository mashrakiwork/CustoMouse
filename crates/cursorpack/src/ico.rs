//! Windows ICO/CUR container: a multi-size directory of 32bpp BGRA DIB images.
//!
//! Layout, all little-endian:
//!
//! ```text
//! ICONDIR      0  u16 reserved = 0
//!              2  u16 type     = 2 for cursors (1 for icons)
//!              4  u16 count
//! ICONDIRENTRY (16 bytes each, `count` of them)
//!              0  u8  width       (0 means 256)
//!              1  u8  height      (0 means 256)
//!              2  u8  color_count = 0
//!              3  u8  reserved    = 0
//!              4  u16 hotspot_x   (cursors only; "planes" for icons)
//!              6  u16 hotspot_y   (cursors only; "bitcount" for icons)
//!              8  u32 bytes_in_res
//!             12  u32 image_offset
//! image        BITMAPINFOHEADER (40) + XOR bitmap (BGRA, bottom-up) + AND mask (1bpp)
//! ```
//!
//! The hotspot lives in the *directory entry*, i.e. once per image, so a
//! multi-size cursor carries a correctly scaled hotspot for every size.
//!
//! Verified against `C:\Windows\Cursors\aero_busy.ani`, whose frames report image
//! sizes of exactly 16936 / 9640 / 4264 bytes for 64/48/32 px:
//!   64 -> 40 + 64*64*4 + 8*64  = 16936
//!   48 -> 40 + 48*48*4 + 8*48  = 9640
//!   32 -> 40 + 32*32*4 + 4*32  = 4264

use crate::{Error, Result};

/// `ICONDIR.type` for a cursor.
pub const TYPE_CURSOR: u16 = 2;
/// `ICONDIR.type` for an icon.
pub const TYPE_ICON: u16 = 1;

const DIR_HEADER: usize = 6;
const DIR_ENTRY: usize = 16;
const BITMAPINFOHEADER: u32 = 40;

/// One rasterized cursor image at a single square size.
///
/// `rgba` is top-down, row-major, 8 bits per channel, straight (not premultiplied)
/// alpha — `size * size * 4` bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub size: u32,
    pub rgba: Vec<u8>,
    pub hot_x: u16,
    pub hot_y: u16,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("size", &self.size)
            .field("hot", &(self.hot_x, self.hot_y))
            .field("rgba_len", &self.rgba.len())
            .finish()
    }
}

impl Image {
    pub fn new(size: u32, rgba: Vec<u8>, hot_x: u16, hot_y: u16) -> Result<Self> {
        let want = (size as usize) * (size as usize) * 4;
        if rgba.len() != want {
            return Err(Error::malformed(
                "image",
                format!("{size}x{size} needs {want} RGBA bytes, got {}", rgba.len()),
            ));
        }
        Ok(Image {
            size,
            rgba,
            hot_x,
            hot_y,
        })
    }

    /// Bytes this image occupies inside the container.
    fn encoded_len(&self) -> usize {
        BITMAPINFOHEADER as usize + self.xor_len() + self.and_len()
    }

    fn xor_len(&self) -> usize {
        (self.size as usize) * (self.size as usize) * 4
    }

    /// 1bpp mask, each row padded up to a 4-byte boundary.
    fn and_len(&self) -> usize {
        and_stride(self.size) * self.size as usize
    }
}

fn and_stride(size: u32) -> usize {
    (((size + 31) / 32) * 4) as usize
}

/// Bytes one image occupies inside a container, without building it.
pub fn image_bytes(size: u32) -> usize {
    let s = size as usize;
    BITMAPINFOHEADER as usize + s * s * 4 + and_stride(size) * s
}

/// Bytes a complete directory of these sizes would occupy.
///
/// Used to keep animated cursors under the size ceiling the Windows `.ani`
/// loader imposes on each frame.
pub fn frame_bytes(sizes: &[u32]) -> usize {
    DIR_HEADER + DIR_ENTRY * sizes.len() + sizes.iter().map(|&s| image_bytes(s)).sum::<usize>()
}

/// Encode the width/height byte: 256 is stored as 0.
fn dim_byte(size: u32) -> u8 {
    if size >= 256 {
        0
    } else {
        size as u8
    }
}

fn decode_dim(b: u8) -> u32 {
    if b == 0 {
        256
    } else {
        b as u32
    }
}

/// Encode a set of images as a single `.cur` (or `.ico`) file.
///
/// Images are emitted largest-first, which is what Windows' own cursors do.
pub fn encode(images: &[Image], kind: u16) -> Result<Vec<u8>> {
    if images.is_empty() {
        return Err(Error::malformed("cursor", "no images"));
    }

    let mut sorted: Vec<&Image> = images.iter().collect();
    sorted.sort_by(|a, b| b.size.cmp(&a.size));

    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&(sorted.len() as u16).to_le_bytes());

    let mut offset = (DIR_HEADER + DIR_ENTRY * sorted.len()) as u32;
    for img in &sorted {
        let len = img.encoded_len() as u32;
        out.push(dim_byte(img.size));
        out.push(dim_byte(img.size));
        out.push(0); // color count
        out.push(0); // reserved
        if kind == TYPE_CURSOR {
            out.extend_from_slice(&img.hot_x.to_le_bytes());
            out.extend_from_slice(&img.hot_y.to_le_bytes());
        } else {
            out.extend_from_slice(&1u16.to_le_bytes()); // planes
            out.extend_from_slice(&32u16.to_le_bytes()); // bitcount
        }
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += len;
    }

    for img in &sorted {
        write_dib(&mut out, img);
    }

    Ok(out)
}

fn write_dib(out: &mut Vec<u8>, img: &Image) {
    let size = img.size;

    // BITMAPINFOHEADER. Height is doubled because the DIB holds XOR + AND stacked.
    out.extend_from_slice(&BITMAPINFOHEADER.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&((size as i32) * 2).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bit count
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&((img.xor_len() + img.and_len()) as u32).to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes()); // x ppm
    out.extend_from_slice(&0i32.to_le_bytes()); // y ppm
    out.extend_from_slice(&0u32.to_le_bytes()); // colors used
    out.extend_from_slice(&0u32.to_le_bytes()); // colors important

    // XOR bitmap: BGRA, bottom-up.
    for y in (0..size).rev() {
        let row = (y as usize) * (size as usize) * 4;
        for x in 0..size as usize {
            let p = row + x * 4;
            out.push(img.rgba[p + 2]); // B
            out.push(img.rgba[p + 1]); // G
            out.push(img.rgba[p]); // R
            out.push(img.rgba[p + 3]); // A
        }
    }

    // AND mask: all zero (fully opaque). Windows composites using the alpha channel
    // for 32bpp images, but the mask must still be present and correctly sized.
    out.extend(std::iter::repeat(0u8).take(img.and_len()));
}

/// A directory entry, without decoding pixels. Cheap; used for validation and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub width: u32,
    pub height: u32,
    pub hot_x: u16,
    pub hot_y: u16,
    pub bytes: u32,
    pub offset: u32,
}

/// Read just the directory of a `.cur`/`.ico`.
pub fn read_dir(data: &[u8]) -> Result<Vec<Entry>> {
    if data.len() < DIR_HEADER {
        return Err(Error::malformed("cursor", "shorter than a directory header"));
    }
    let count = u16::from_le_bytes([data[4], data[5]]) as usize;
    if count == 0 {
        return Err(Error::malformed("cursor", "directory declares zero images"));
    }
    let need = DIR_HEADER + DIR_ENTRY * count;
    if data.len() < need {
        return Err(Error::malformed(
            "cursor",
            format!("directory of {count} needs {need} bytes, file has {}", data.len()),
        ));
    }

    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = &data[DIR_HEADER + i * DIR_ENTRY..][..DIR_ENTRY];
        out.push(Entry {
            width: decode_dim(e[0]),
            height: decode_dim(e[1]),
            hot_x: u16::from_le_bytes([e[4], e[5]]),
            hot_y: u16::from_le_bytes([e[6], e[7]]),
            bytes: u32::from_le_bytes([e[8], e[9], e[10], e[11]]),
            offset: u32::from_le_bytes([e[12], e[13], e[14], e[15]]),
        });
    }
    Ok(out)
}

/// Fully decode a `.cur`/`.ico` back to images.
///
/// Handles the 32bpp BI_RGB form we emit and that Windows' own cursors use. PNG-payload
/// entries (Vista icon style) and paletted DIBs are reported rather than silently skipped.
pub fn decode(data: &[u8]) -> Result<Vec<Image>> {
    let entries = read_dir(data)?;
    let mut out = Vec::with_capacity(entries.len());

    for e in entries {
        let start = e.offset as usize;
        let end = start
            .checked_add(e.bytes as usize)
            .ok_or_else(|| Error::malformed("cursor", "entry length overflows"))?;
        if end > data.len() {
            return Err(Error::malformed(
                "cursor",
                format!("image at {start}..{end} runs past end of {} bytes", data.len()),
            ));
        }
        let img = &data[start..end];

        if img.starts_with(&[0x89, b'P', b'N', b'G']) {
            return Err(Error::malformed(
                "cursor",
                "image is a PNG payload, which Windows cursors do not support",
            ));
        }
        if img.len() < BITMAPINFOHEADER as usize {
            return Err(Error::malformed("cursor", "image shorter than its header"));
        }

        let width = i32::from_le_bytes([img[4], img[5], img[6], img[7]]);
        let height2 = i32::from_le_bytes([img[8], img[9], img[10], img[11]]);
        let bitcount = u16::from_le_bytes([img[14], img[15]]);
        let compression = u32::from_le_bytes([img[16], img[17], img[18], img[19]]);

        if bitcount != 32 {
            return Err(Error::malformed(
                "cursor",
                format!("{bitcount}bpp images are not supported (only 32bpp BGRA)"),
            ));
        }
        if compression != 0 {
            return Err(Error::malformed(
                "cursor",
                format!("compression {compression} is not supported (only BI_RGB)"),
            ));
        }
        // The stored height covers XOR + AND stacked, so the real height is half.
        let w = width.max(0) as u32;
        let h = (height2.max(0) / 2) as u32;
        if w == 0 || h == 0 || w != h {
            return Err(Error::malformed(
                "cursor",
                format!("expected a square image, got {w}x{h}"),
            ));
        }

        let xor_len = (w as usize) * (h as usize) * 4;
        let xor_start = BITMAPINFOHEADER as usize;
        if img.len() < xor_start + xor_len {
            return Err(Error::malformed("cursor", "pixel data truncated"));
        }
        let xor = &img[xor_start..xor_start + xor_len];

        // Un-flip and swap BGRA -> RGBA.
        let mut rgba = vec![0u8; xor_len];
        for y in 0..h as usize {
            let src_row = (h as usize - 1 - y) * (w as usize) * 4;
            let dst_row = y * (w as usize) * 4;
            for x in 0..w as usize {
                let s = src_row + x * 4;
                let d = dst_row + x * 4;
                rgba[d] = xor[s + 2]; // R
                rgba[d + 1] = xor[s + 1]; // G
                rgba[d + 2] = xor[s]; // B
                rgba[d + 3] = xor[s + 3]; // A
            }
        }

        out.push(Image {
            size: w,
            rgba,
            hot_x: e.hot_x,
            hot_y: e.hot_y,
        });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(size: u32, rgba: [u8; 4]) -> Image {
        Image {
            size,
            rgba: rgba.iter().copied().cycle().take((size * size * 4) as usize).collect(),
            hot_x: 0,
            hot_y: 0,
        }
    }

    /// The byte sizes Microsoft's own cursors report, recomputed from our model.
    #[test]
    fn image_lengths_match_windows_cursors() {
        assert_eq!(solid(64, [0; 4]).encoded_len(), 16936);
        assert_eq!(solid(48, [0; 4]).encoded_len(), 9640);
        assert_eq!(solid(32, [0; 4]).encoded_len(), 4264);
    }

    #[test]
    fn and_mask_stride_is_padded_to_four_bytes() {
        assert_eq!(and_stride(32), 4);
        assert_eq!(and_stride(48), 8);
        assert_eq!(and_stride(64), 8);
        assert_eq!(and_stride(96), 12);
        assert_eq!(and_stride(128), 16);
    }

    #[test]
    fn round_trip_preserves_pixels_and_hotspots() {
        let mut a = solid(32, [10, 20, 30, 255]);
        a.hot_x = 3;
        a.hot_y = 2;
        let mut b = solid(64, [40, 50, 60, 128]);
        b.hot_x = 6;
        b.hot_y = 4;

        let bytes = encode(&[a.clone(), b.clone()], TYPE_CURSOR).unwrap();
        let back = decode(&bytes).unwrap();

        // Largest first.
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].size, 64);
        assert_eq!(back[1].size, 32);
        assert_eq!(back[0], b);
        assert_eq!(back[1], a);
    }

    #[test]
    fn declared_offsets_land_on_real_images() {
        let bytes = encode(&[solid(32, [1, 2, 3, 4]), solid(64, [5, 6, 7, 8])], TYPE_CURSOR).unwrap();
        let dir = read_dir(&bytes).unwrap();
        let mut cursor = (DIR_HEADER + DIR_ENTRY * 2) as u32;
        for e in &dir {
            assert_eq!(e.offset, cursor, "entries must be contiguous");
            cursor += e.bytes;
        }
        assert_eq!(cursor as usize, bytes.len(), "no trailing slack");
    }

    #[test]
    fn rejects_wrong_buffer_size() {
        assert!(Image::new(32, vec![0; 10], 0, 0).is_err());
        assert!(Image::new(32, vec![0; 32 * 32 * 4], 0, 0).is_ok());
    }

    #[test]
    fn size_256_is_stored_as_zero() {
        assert_eq!(dim_byte(256), 0);
        assert_eq!(decode_dim(0), 256);
        assert_eq!(dim_byte(128), 128);
        assert_eq!(decode_dim(128), 128);
    }
}
