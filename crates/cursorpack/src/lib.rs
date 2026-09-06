//! Cursor pack core: manifest, rasterization, and binary cursor encoding.
//!
//! Deliberately free of OS-specific code so the whole pipeline is testable
//! without touching the registry or the live cursor.

pub mod ani;
pub mod build;
pub mod error;
pub mod ico;
pub mod import;
pub mod manifest;
pub mod raster;

pub use error::{Error, Result};

/// Sizes emitted into every cursor, covering 100% through 400% display scaling
/// plus the full range of the Windows accessibility cursor-size slider
/// (`CursorBaseSize` runs 32..256).
///
/// Animated roles are trimmed from this set by [`ani::fit_sizes`], because the
/// `.ani` loader caps how much a single frame may hold. Static `.cur` files have
/// no such limit and keep all of them.
pub const DEFAULT_SIZES: &[u32] = &[32, 48, 64, 96, 128, 192, 256];

/// Smallest source art we accept. Below this, output at 128px would be an upscale
/// and would look soft, which is the single most common flaw in cursor packs.
pub const MIN_SOURCE_PX: u32 = 128;
