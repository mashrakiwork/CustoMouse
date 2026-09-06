//! Windows animated cursor (`.ani`) — a RIFF `ACON` file.
//!
//! ```text
//! RIFF <size> ACON
//!   anih <36>            9 x u32, see AnimHeader
//!   rate <4*steps>       optional, per-step delay in jiffies
//!   seq  <4*steps>       optional, step -> frame index
//!   LIST <size> fram
//!     icon <size> ...    each a complete .cur file
//! ```
//!
//! Two things that are widely gotten wrong, both verified against
//! `C:\Windows\Cursors\aero_busy.ani` on Windows 11:
//!
//! 1. Each `icon` chunk is a full multi-size cursor. `aero_busy.ani` carries
//!    64/48/32 px in *every* frame, so a single `.ani` covers all DPI scales.
//!    The common advice to emit one `.ani` per size is wrong.
//! 2. `_l` / `_xl` variants are not DPI variants — they hold the same dimensions
//!    with different pixel data, i.e. the accessibility large-pointer schemes.
//!
//! Timing is quantised to jiffies (1/60 s), so the achievable frame rates are
//! 60, 30, 20, 15, 12, 10 ... fps.

use crate::ico::{self, Image};
use crate::{Error, Result};

/// Frames are `.cur`/`.ico` data rather than raw DIBs.
pub const AF_ICON: u32 = 0x1;
/// A `seq ` chunk is present.
pub const AF_SEQUENCE: u32 = 0x2;

/// One jiffy is 1/60 second.
pub const JIFFIES_PER_SEC: f32 = 60.0;

/// Largest per-frame payload the Windows `.ani` loader reliably accepts.
///
/// Measured on Windows 11 by feeding generated files to `LoadImageW`, which is the
/// only authority on this — the limit is undocumented:
///
/// | frame contents      | bytes/frame | result |
/// |---------------------|-------------|--------|
/// | `[32,48,64]` x120   | 30,894      | loads (3.7 MB total, so the *file* size does not matter) |
/// | `[32,48,64,96]`     | 68,966      | loads |
/// | `[32,128]`          | 71,926      | rejected |
/// | `[128]` alone       | 67,646      | loads |
///
/// The ceiling applies per frame, not per file, and single-image frames are exempt
/// (a lone 256px frame at 270 KB loads). The smallest multi-image frame observed to
/// be rejected was 67,110 bytes, so this cap sits safely below that.
///
/// This is why Windows' own animated cursors ship 32/48/64 and never larger, and
/// why animated roles here are budgeted while static `.cur` files are not.
pub const MAX_FRAME_BYTES: usize = 65_536;

/// Choose the largest prefix of `sizes` that fits inside [`MAX_FRAME_BYTES`].
///
/// Sizes are considered smallest-first, because the small ones are what actually
/// get displayed at ordinary DPI. Returns `(kept, dropped)`; at least one size is
/// always kept, since a single-image frame is exempt from the limit.
pub fn fit_sizes(sizes: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let mut sorted: Vec<u32> = sizes.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut kept: Vec<u32> = Vec::new();
    let mut dropped = Vec::new();
    for &s in &sorted {
        let mut trial = kept.clone();
        trial.push(s);
        if kept.is_empty() || crate::ico::frame_bytes(&trial) <= MAX_FRAME_BYTES {
            kept = trial;
        } else {
            dropped.push(s);
        }
    }
    (kept, dropped)
}

/// Convert a frame rate to whole jiffies, clamped to at least one.
pub fn jiffies_for_fps(fps: f32) -> u32 {
    if !fps.is_finite() || fps <= 0.0 {
        return 6; // 10 fps, a sane fallback
    }
    ((JIFFIES_PER_SEC / fps).round() as i64).clamp(1, u32::MAX as i64) as u32
}

/// The frame rate that a jiffy count actually produces.
pub fn fps_for_jiffies(j: u32) -> f32 {
    JIFFIES_PER_SEC / j.max(1) as f32
}

/// An animated cursor: a sequence of frames, each itself a multi-size image set.
#[derive(Debug, Clone)]
pub struct Anim {
    /// Each entry is one frame, holding that frame's images at every size.
    pub frames: Vec<Vec<Image>>,
    /// Default delay for every step, in jiffies.
    pub default_rate: u32,
    /// Optional per-step delays in jiffies. Length must equal the step count.
    pub rates: Option<Vec<u32>>,
    /// Optional step -> frame index mapping, for reusing frames.
    pub seq: Option<Vec<u32>>,
}

impl Anim {
    /// Build from frames at a given frame rate.
    pub fn from_frames(frames: Vec<Vec<Image>>, fps: f32) -> Result<Self> {
        if frames.is_empty() {
            return Err(Error::malformed("ani", "no frames"));
        }
        Ok(Anim {
            frames,
            default_rate: jiffies_for_fps(fps),
            rates: None,
            seq: None,
        })
    }

    /// Number of animation steps, which may exceed the frame count when `seq` is used.
    pub fn steps(&self) -> usize {
        self.seq.as_ref().map_or(self.frames.len(), |s| s.len())
    }

    /// Total loop duration in milliseconds.
    pub fn duration_ms(&self) -> f32 {
        let total: u32 = match &self.rates {
            Some(r) => r.iter().sum(),
            None => self.default_rate * self.steps() as u32,
        };
        total as f32 / JIFFIES_PER_SEC * 1000.0
    }

    /// The sizes present, taken from the first frame.
    pub fn sizes(&self) -> Vec<u32> {
        let mut s: Vec<u32> = self.frames[0].iter().map(|i| i.size).collect();
        s.sort_unstable();
        s
    }

    fn validate(&self) -> Result<()> {
        let steps = self.steps();
        if let Some(r) = &self.rates {
            if r.len() != steps {
                return Err(Error::malformed(
                    "ani",
                    format!("rate chunk has {} entries but there are {steps} steps", r.len()),
                ));
            }
        }
        if let Some(s) = &self.seq {
            if let Some(bad) = s.iter().find(|&&i| i as usize >= self.frames.len()) {
                return Err(Error::malformed(
                    "ani",
                    format!("seq refers to frame {bad}, but there are only {}", self.frames.len()),
                ));
            }
        }
        let first: Vec<u32> = self.frames[0].iter().map(|i| i.size).collect();
        for (n, f) in self.frames.iter().enumerate() {
            if f.is_empty() {
                return Err(Error::malformed("ani", format!("frame {n} has no images")));
            }
            let sizes: Vec<u32> = f.iter().map(|i| i.size).collect();
            if sizes != first {
                return Err(Error::malformed(
                    "ani",
                    format!("frame {n} has sizes {sizes:?} but frame 0 has {first:?}"),
                ));
            }
        }
        Ok(())
    }
}

fn push_chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0); // RIFF chunks are word-aligned
    }
}

/// Encode an animated cursor.
pub fn encode(anim: &Anim) -> Result<Vec<u8>> {
    anim.validate()?;

    let steps = anim.steps();
    let mut flags = AF_ICON;
    if anim.seq.is_some() {
        flags |= AF_SEQUENCE;
    }

    // anih
    let mut anih = Vec::with_capacity(36);
    for v in [
        36u32,
        anim.frames.len() as u32,
        steps as u32,
        0, // cx - zero when frames are icons
        0, // cy
        32, // bit count
        1,  // planes
        anim.default_rate,
        flags,
    ] {
        anih.extend_from_slice(&v.to_le_bytes());
    }

    // LIST fram
    let mut fram = Vec::new();
    fram.extend_from_slice(b"fram");
    for frame in &anim.frames {
        let cur = ico::encode(frame, ico::TYPE_CURSOR)?;
        push_chunk(&mut fram, b"icon", &cur);
    }

    let mut body = Vec::new();
    body.extend_from_slice(b"ACON");
    push_chunk(&mut body, b"anih", &anih);

    if let Some(rates) = &anim.rates {
        let mut b = Vec::with_capacity(rates.len() * 4);
        for r in rates {
            b.extend_from_slice(&r.to_le_bytes());
        }
        push_chunk(&mut body, b"rate", &b);
    }
    if let Some(seq) = &anim.seq {
        let mut b = Vec::with_capacity(seq.len() * 4);
        for s in seq {
            b.extend_from_slice(&s.to_le_bytes());
        }
        push_chunk(&mut body, b"seq ", &b);
    }

    push_chunk(&mut body, b"LIST", &fram);

    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// The raw `anih` header, as read from a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimHeader {
    pub cb_size: u32,
    pub frames: u32,
    pub steps: u32,
    pub cx: u32,
    pub cy: u32,
    pub bit_count: u32,
    pub planes: u32,
    pub display_rate: u32,
    pub flags: u32,
}

/// Everything a parsed `.ani` contains, with frames left as raw `.cur` bytes.
#[derive(Debug, Clone)]
pub struct RawAni {
    pub header: AnimHeader,
    pub frames: Vec<Vec<u8>>,
    pub rates: Option<Vec<u32>>,
    pub seq: Option<Vec<u32>>,
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn read_u32_list(d: &[u8]) -> Vec<u32> {
    d.chunks_exact(4).map(|c| u32_at(c, 0)).collect()
}

/// Parse an `.ani` without decoding pixels.
pub fn parse(data: &[u8]) -> Result<RawAni> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"ACON" {
        return Err(Error::malformed("ani", "not a RIFF ACON file"));
    }

    let mut header: Option<AnimHeader> = None;
    let mut frames = Vec::new();
    let mut rates = None;
    let mut seq = None;

    let mut off = 12usize;
    while off + 8 <= data.len() {
        let id = &data[off..off + 4];
        let size = u32_at(data, off + 4) as usize;
        let body = off + 8;
        let end = body
            .checked_add(size)
            .filter(|e| *e <= data.len())
            .ok_or_else(|| {
                Error::malformed("ani", format!("chunk at {off} claims {size} bytes, past end"))
            })?;

        match id {
            b"anih" => {
                if size < 36 {
                    return Err(Error::malformed("ani", "anih chunk shorter than 36 bytes"));
                }
                let d = &data[body..];
                header = Some(AnimHeader {
                    cb_size: u32_at(d, 0),
                    frames: u32_at(d, 4),
                    steps: u32_at(d, 8),
                    cx: u32_at(d, 12),
                    cy: u32_at(d, 16),
                    bit_count: u32_at(d, 20),
                    planes: u32_at(d, 24),
                    display_rate: u32_at(d, 28),
                    flags: u32_at(d, 32),
                });
            }
            b"rate" => rates = Some(read_u32_list(&data[body..end])),
            b"seq " => seq = Some(read_u32_list(&data[body..end])),
            b"LIST" if size >= 4 && &data[body..body + 4] == b"fram" => {
                let mut o = body + 4;
                while o + 8 <= end {
                    let sub = &data[o..o + 4];
                    let ssize = u32_at(data, o + 4) as usize;
                    let sbody = o + 8;
                    if sbody + ssize > end {
                        return Err(Error::malformed("ani", "frame chunk runs past the LIST"));
                    }
                    if sub == b"icon" {
                        frames.push(data[sbody..sbody + ssize].to_vec());
                    }
                    o = sbody + ssize + (ssize & 1);
                }
            }
            _ => {}
        }

        off = end + (size & 1);
    }

    let header = header.ok_or_else(|| Error::malformed("ani", "no anih chunk"))?;
    if frames.is_empty() {
        return Err(Error::malformed("ani", "no icon frames"));
    }
    Ok(RawAni {
        header,
        frames,
        rates,
        seq,
    })
}

/// Parse and fully decode into an [`Anim`].
pub fn decode(data: &[u8]) -> Result<Anim> {
    let raw = parse(data)?;
    let mut frames = Vec::with_capacity(raw.frames.len());
    for f in &raw.frames {
        frames.push(ico::decode(f)?);
    }
    Ok(Anim {
        frames,
        default_rate: raw.header.display_rate,
        rates: raw.rates,
        seq: raw.seq,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(size: u32, tint: u8) -> Image {
        Image {
            size,
            rgba: (0..size * size * 4).map(|i| (i as u8).wrapping_add(tint)).collect(),
            hot_x: (size / 8) as u16,
            hot_y: (size / 16) as u16,
        }
    }

    fn anim(n: usize, fps: f32) -> Anim {
        let frames = (0..n).map(|i| vec![img(32, i as u8), img(64, i as u8)]).collect();
        Anim::from_frames(frames, fps).unwrap()
    }

    /// 20 fps must land on exactly 3 jiffies, matching Windows' own aero_busy.ani.
    #[test]
    fn frame_rate_quantises_to_jiffies() {
        assert_eq!(jiffies_for_fps(20.0), 3);
        assert_eq!(jiffies_for_fps(60.0), 1);
        assert_eq!(jiffies_for_fps(30.0), 2);
        assert_eq!(jiffies_for_fps(10.0), 6);
        assert_eq!(jiffies_for_fps(0.0), 6); // fallback, not a divide by zero
        assert_eq!(jiffies_for_fps(1000.0), 1); // clamped, never zero
    }

    /// The realised duration must stay within one jiffy of what was asked for.
    #[test]
    fn duration_is_within_one_jiffy_of_target() {
        for &fps in &[10.0f32, 12.0, 15.0, 20.0, 24.0, 30.0, 60.0] {
            let n = 12;
            let a = anim(n, fps);
            let target = n as f32 / fps * 1000.0;
            let jiffy_ms = 1000.0 / JIFFIES_PER_SEC;
            let drift = (a.duration_ms() - target).abs();
            assert!(
                drift <= jiffy_ms * n as f32,
                "at {fps} fps drift was {drift}ms"
            );
        }
    }

    #[test]
    fn round_trip_preserves_frames_sizes_and_rate() {
        let a = anim(6, 20.0);
        let bytes = encode(&a).unwrap();
        let back = decode(&bytes).unwrap();

        assert_eq!(back.frames.len(), 6);
        assert_eq!(back.default_rate, 3);
        assert_eq!(back.sizes(), vec![32, 64]);
        for (orig, got) in a.frames.iter().zip(&back.frames) {
            // encode() sorts largest-first, so compare as sets by size.
            for o in orig {
                let g = got.iter().find(|g| g.size == o.size).expect("size present");
                assert_eq!(g.rgba, o.rgba);
                assert_eq!((g.hot_x, g.hot_y), (o.hot_x, o.hot_y));
            }
        }
    }

    #[test]
    fn header_matches_the_shape_windows_writes() {
        let bytes = encode(&anim(18, 20.0)).unwrap();
        let raw = parse(&bytes).unwrap();
        assert_eq!(raw.header.cb_size, 36);
        assert_eq!(raw.header.frames, 18);
        assert_eq!(raw.header.steps, 18);
        assert_eq!(raw.header.bit_count, 32);
        assert_eq!(raw.header.planes, 1);
        assert_eq!(raw.header.display_rate, 3);
        assert_eq!(raw.header.flags, AF_ICON, "no seq chunk means AF_ICON alone");
        assert!(raw.rates.is_none());
        assert!(raw.seq.is_none());
    }

    #[test]
    fn riff_size_field_matches_actual_length() {
        let bytes = encode(&anim(5, 15.0)).unwrap();
        let declared = u32_at(&bytes, 4) as usize;
        assert_eq!(declared + 8, bytes.len());
        assert_eq!(bytes.len() % 2, 0, "top level should stay word aligned");
    }

    #[test]
    fn sequence_and_rate_chunks_survive() {
        let mut a = anim(3, 20.0);
        a.seq = Some(vec![0, 1, 2, 1]);
        a.rates = Some(vec![3, 6, 3, 9]);

        let bytes = encode(&a).unwrap();
        let raw = parse(&bytes).unwrap();
        assert_eq!(raw.header.flags, AF_ICON | AF_SEQUENCE);
        assert_eq!(raw.header.steps, 4);
        assert_eq!(raw.header.frames, 3);
        assert_eq!(raw.seq.unwrap(), vec![0, 1, 2, 1]);
        assert_eq!(raw.rates.unwrap(), vec![3, 6, 3, 9]);
        assert_eq!(a.duration_ms(), 21.0 / 60.0 * 1000.0);
    }

    #[test]
    fn rejects_inconsistent_frames() {
        // A frame missing a size would silently change resolution mid-animation.
        let bad = Anim {
            frames: vec![vec![img(32, 0), img(64, 0)], vec![img(32, 1)]],
            default_rate: 3,
            rates: None,
            seq: None,
        };
        assert!(encode(&bad).is_err());

        let bad_seq = Anim {
            frames: vec![vec![img(32, 0)]],
            default_rate: 3,
            rates: None,
            seq: Some(vec![0, 5]),
        };
        assert!(encode(&bad_seq).is_err());

        let bad_rate = Anim {
            frames: vec![vec![img(32, 0)], vec![img(32, 1)]],
            default_rate: 3,
            rates: Some(vec![3]),
            seq: None,
        };
        assert!(encode(&bad_rate).is_err());
    }

    /// Animated cursors must land on the size set Windows itself ships, and never
    /// exceed the loader's per-frame budget.
    #[test]
    fn size_budget_keeps_the_small_sizes_and_drops_the_large() {
        let (kept, dropped) = fit_sizes(&[32, 48, 64, 96, 128]);
        assert_eq!(kept, vec![32, 48, 64], "should match what Windows ships");
        assert_eq!(dropped, vec![96, 128]);
        assert!(crate::ico::frame_bytes(&kept) <= MAX_FRAME_BYTES);

        // Sets that already fit are left alone.
        let (kept, dropped) = fit_sizes(&[32, 48]);
        assert_eq!(kept, vec![32, 48]);
        assert!(dropped.is_empty());

        // A single oversized size is still kept: one-image frames are exempt.
        let (kept, dropped) = fit_sizes(&[256]);
        assert_eq!(kept, vec![256]);
        assert!(dropped.is_empty());

        // Input order and duplicates must not matter.
        assert_eq!(fit_sizes(&[128, 32, 48, 32, 64]).0, vec![32, 48, 64]);
    }

    #[test]
    fn rejects_junk() {
        assert!(parse(b"not a riff file at all").is_err());
        assert!(parse(b"RIFF\x04\x00\x00\x00WAVE").is_err());
    }
}
