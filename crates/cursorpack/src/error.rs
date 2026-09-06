//! Error type. Messages are written to be shown directly to the user in the
//! import wizard, so they name the file and say what to do about it.

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path}: not valid JSON: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("{path}: could not read this SVG: {msg}")]
    Svg { path: PathBuf, msg: String },

    #[error("{path}: could not read this image: {msg}")]
    Image { path: PathBuf, msg: String },

    /// The source art is too small to make a sharp cursor.
    #[error(
        "{path} is {got}x{got} px, but cursors need at least {want}x{want}. \
         Use an SVG, or a PNG that is {want}px or larger — upscaling would look blurry."
    )]
    SourceTooSmall {
        path: PathBuf,
        got: u32,
        want: u32,
    },

    /// Numbered PNG sequences must be contiguous, or frames play out of order.
    #[error(
        "{dir}: frame numbering has a gap — found {found} frames numbered up to {max}. \
         Missing: {missing}. Rename them so they run consecutively from {first}."
    )]
    FrameGap {
        dir: PathBuf,
        found: usize,
        max: usize,
        first: usize,
        missing: String,
    },

    #[error(
        "{path} is {w}x{h}, but the first frame is {fw}x{fh}. \
         Every frame in an animation must be the same size."
    )]
    FrameSizeMismatch {
        path: PathBuf,
        w: u32,
        h: u32,
        fw: u32,
        fh: u32,
    },

    #[error("{dir}: no usable images found. Expected .svg, .png, or .gif files.")]
    NoFrames { dir: PathBuf },

    #[error(
        "hotspot ({x},{y}) for role '{role}' is outside the {size}x{size} canvas. \
         It must be within 0..{max}."
    )]
    HotspotOutOfBounds {
        role: String,
        x: i32,
        y: i32,
        size: u32,
        max: u32,
    },

    #[error(
        "sprite sheet {path} is {w}x{h}, which does not divide evenly into \
         {cols}x{rows} cells. Check the grid values."
    )]
    SheetNotDivisible {
        path: PathBuf,
        w: u32,
        h: u32,
        cols: u32,
        rows: u32,
    },

    #[error(
        "sprite sheet {path}: asked for {frames} frames but the {cols}x{rows} grid \
         only holds {capacity}."
    )]
    SheetFrameCount {
        path: PathBuf,
        frames: u32,
        cols: u32,
        rows: u32,
        capacity: u32,
    },

    #[error("'{0}' is not a cursor role this system uses")]
    UnknownRole(String),

    #[error("pack has no roles defined — it would not change any cursor")]
    EmptyPack,

    #[error("{path}: not a {what} file (bad signature)")]
    BadSignature { path: PathBuf, what: &'static str },

    #[error("malformed {what}: {msg}")]
    Malformed { what: &'static str, msg: String },

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }

    pub fn malformed(what: &'static str, msg: impl Into<String>) -> Self {
        Error::Malformed {
            what,
            msg: msg.into(),
        }
    }
}
