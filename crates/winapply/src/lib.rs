//! Applying cursor packs on Windows, and getting the defaults back.
//!
//! Two rules shape this crate:
//!
//! 1. **Everything is per-user.** Only `HKCU` and `%LOCALAPPDATA%` are written,
//!    so no admin rights are needed and nothing under `C:\Windows` changes.
//! 2. **Restoring must never depend on us.** A restore point is captured before
//!    the first apply, and a plain `.cmd` script is written alongside it that
//!    undoes everything using only `reg.exe` — so the defaults come back even if
//!    this program will not start, or has been deleted.

#[cfg(windows)]
pub mod apply;
#[cfg(windows)]
pub mod registry;
#[cfg(windows)]
pub mod restore;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("registry {key}: {source}")]
    Registry {
        key: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("building the pack: {0}")]
    Pack(#[from] cursorpack::Error),

    #[error("could not parse {path}: {source}")]
    Json {
        path: std::path::PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// Windows itself refused the file, which means the bytes are wrong.
    #[error("Windows rejected the cursor file {path}: {msg}")]
    CursorRejected {
        path: std::path::PathBuf,
        msg: String,
    },

    #[error("could not tell Windows to reload cursors: {0}")]
    Broadcast(std::io::Error),

    #[error("no restore point has been taken yet")]
    NoRestorePoint,

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn io(path: impl Into<std::path::PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

#[cfg(windows)]
pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Tell Windows to reload every cursor from the registry, right now.
///
/// Without this the registry values are correct but the pointer on screen does
/// not change until the next sign-in.
///
/// **`SPIF_UPDATEINIFILE` must not be passed here**, despite nearly every example
/// online using `SPIF_UPDATEINIFILE | SPIF_SENDCHANGE`. Measured on Windows 11:
///
/// ```text
/// SPIF_UPDATEINIFILE | SPIF_SENDCHANGE  -> 0, "The handle is invalid" (os error 6)
/// SPIF_SENDCHANGE                       -> 1, succeeds
/// SPIF_UPDATEINIFILE                    -> 0, "The handle is invalid"
/// ```
///
/// The flag asks Windows to persist the setting to the user profile, which is
/// meaningless for `SPI_SETCURSORS` — and we have already written the registry
/// ourselves, so nothing is lost by omitting it.
#[cfg(windows)]
pub fn broadcast() -> Result<()> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPIF_SENDCHANGE, SPI_SETCURSORS,
    };

    let ok =
        unsafe { SystemParametersInfoW(SPI_SETCURSORS, 0, std::ptr::null_mut(), SPIF_SENDCHANGE) };
    if ok == 0 {
        return Err(Error::Broadcast(std::io::Error::last_os_error()));
    }
    Ok(())
}

/// What Windows reports about a cursor file it successfully loaded.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadedCursor {
    pub width: i32,
    pub height: i32,
    pub hot_x: i32,
    pub hot_y: i32,
}

/// Ask Windows to load a cursor file, which is the only authoritative check that
/// the bytes we wrote are actually a valid cursor.
///
/// Passing `size` requests a specific size; `None` uses the system default.
#[cfg(windows)]
pub fn probe_cursor(path: &std::path::Path, size: Option<u32>) -> Result<LoadedCursor> {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP, HGDIOBJ};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DestroyCursor, GetIconInfo, LoadImageW, HCURSOR, ICONINFO, IMAGE_CURSOR, LR_DEFAULTSIZE,
        LR_LOADFROMFILE,
    };

    let wide = to_wide(&path.to_string_lossy());
    let (cx, cy, flags) = match size {
        Some(s) => (s as i32, s as i32, LR_LOADFROMFILE),
        None => (0, 0, LR_LOADFROMFILE | LR_DEFAULTSIZE),
    };

    let handle: HANDLE =
        unsafe { LoadImageW(std::ptr::null_mut(), wide.as_ptr(), IMAGE_CURSOR, cx, cy, flags) };
    if handle.is_null() {
        return Err(Error::CursorRejected {
            path: path.to_path_buf(),
            msg: std::io::Error::last_os_error().to_string(),
        });
    }

    let cursor = handle as HCURSOR;
    let mut info: ICONINFO = unsafe { std::mem::zeroed() };
    let got_info = unsafe { GetIconInfo(cursor, &mut info) };
    if got_info == 0 {
        unsafe { DestroyCursor(cursor) };
        return Err(Error::CursorRejected {
            path: path.to_path_buf(),
            msg: "loaded, but Windows would not describe it".into(),
        });
    }

    // Colour bitmap for 32bpp cursors; the mask is twice as tall for 1bpp ones.
    let mut bm: BITMAP = unsafe { std::mem::zeroed() };
    let (w, h) = if !info.hbmColor.is_null() {
        unsafe {
            GetObjectW(
                info.hbmColor as HGDIOBJ,
                std::mem::size_of::<BITMAP>() as i32,
                (&mut bm as *mut BITMAP).cast(),
            )
        };
        (bm.bmWidth, bm.bmHeight)
    } else {
        unsafe {
            GetObjectW(
                info.hbmMask as HGDIOBJ,
                std::mem::size_of::<BITMAP>() as i32,
                (&mut bm as *mut BITMAP).cast(),
            )
        };
        (bm.bmWidth, bm.bmHeight / 2)
    };

    let out = LoadedCursor {
        width: w,
        height: h,
        hot_x: info.xHotspot as i32,
        hot_y: info.yHotspot as i32,
    };

    unsafe {
        if !info.hbmColor.is_null() {
            DeleteObject(info.hbmColor as HGDIOBJ);
        }
        if !info.hbmMask.is_null() {
            DeleteObject(info.hbmMask as HGDIOBJ);
        }
        DestroyCursor(cursor);
    }

    Ok(out)
}

/// Load a cursor file at `size` and read back the pixels Windows produced, as BGRA.
///
/// This is how we measure what Windows actually renders, rather than trusting that
/// the size we embedded is the size it chose.
#[cfg(windows)]
pub fn read_cursor_pixels(path: &std::path::Path, size: u32) -> Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DestroyCursor, GetIconInfo, LoadImageW, HCURSOR, ICONINFO, IMAGE_CURSOR, LR_LOADFROMFILE,
    };

    let wide = to_wide(&path.to_string_lossy());
    let handle: HANDLE = unsafe {
        LoadImageW(
            std::ptr::null_mut(),
            wide.as_ptr(),
            IMAGE_CURSOR,
            size as i32,
            size as i32,
            LR_LOADFROMFILE,
        )
    };
    if handle.is_null() {
        return Err(Error::CursorRejected {
            path: path.to_path_buf(),
            msg: std::io::Error::last_os_error().to_string(),
        });
    }
    let cursor = handle as HCURSOR;

    let mut info: ICONINFO = unsafe { std::mem::zeroed() };
    if unsafe { GetIconInfo(cursor, &mut info) } == 0 {
        unsafe { DestroyCursor(cursor) };
        return Err(Error::CursorRejected {
            path: path.to_path_buf(),
            msg: "no icon info".into(),
        });
    }
    if info.hbmColor.is_null() {
        unsafe {
            if !info.hbmMask.is_null() {
                DeleteObject(info.hbmMask as HGDIOBJ);
            }
            DestroyCursor(cursor);
        }
        return Err(Error::Other("cursor has no colour bitmap".into()));
    }

    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: size as i32,
        // Negative height asks for a top-down buffer, matching our own layout.
        biHeight: -(size as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB as u32,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };

    let mut buf = vec![0u8; (size * size * 4) as usize];
    let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
    let lines = unsafe {
        GetDIBits(
            dc,
            info.hbmColor,
            0,
            size,
            buf.as_mut_ptr().cast(),
            &mut bmi,
            DIB_RGB_COLORS,
        )
    };

    unsafe {
        DeleteDC(dc);
        DeleteObject(info.hbmColor as HGDIOBJ);
        if !info.hbmMask.is_null() {
            DeleteObject(info.hbmMask as HGDIOBJ);
        }
        DestroyCursor(cursor);
    }

    if lines == 0 {
        return Err(Error::Other("GetDIBits returned no scanlines".into()));
    }
    Ok(buf)
}
