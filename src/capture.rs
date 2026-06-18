//! Clipboard inspection and the screenshot -> file -> path-text transform.
//!
//! The clipboard is opened on the UI thread inside the debounce handler. To keep
//! that critical section as short as possible we do only the cheap work while the
//! clipboard is held (read bytes / blit the bitmap, re-set formats, set the path
//! text) and hand the expensive PNG encode + file write to a background worker.
//! The path is therefore on the clipboard almost immediately; the file lands a
//! few milliseconds later — well before the user submits it.

use crate::config::{Config, PathFormat};
use anyhow::{anyhow, bail, Result};
use std::ffi::{c_void, OsString};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows_sys::Win32::Graphics::Gdi::{
    GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows_sys::Win32::UI::Shell::{DragQueryFileW, HDROP};

// Standard clipboard format identifiers (winuser.h).
const CF_BITMAP: u32 = 2;
const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;

/// What kind of clipboard content we acted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    Image,
    Files,
}

/// Result of a successful capture (the image file may still be encoding).
#[derive(Debug, Clone)]
pub struct CaptureInfo {
    pub kind: CaptureKind,
    pub text: String,
    pub file_path: Option<PathBuf>,
    pub file_count: usize,
}

/// The pixel data handed to the encode worker.
enum EncodePayload {
    /// Raw `PNG` clipboard bytes — written verbatim (lossless, keeps alpha).
    Png(Vec<u8>),
    /// 32-bit top-down BGRX from GDI (alpha undefined; worker forces opaque).
    Bgrx { w: u32, h: u32, data: Vec<u8> },
}

/// RAII guard so the clipboard is always closed, even on early returns / errors.
struct ClipboardGuard;
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}

/// Inspect the clipboard; if it holds an image or (optionally) files, save/convert
/// and augment the clipboard with the path text. Returns `Ok(None)` when there is
/// nothing to do, so the caller leaves the clipboard alone.
///
/// # Safety
/// Calls into Win32; must run on the thread that owns the listener window.
pub unsafe fn process_clipboard(
    hwnd: HWND,
    cfg: &Config,
    png_format: u32,
) -> Result<Option<CaptureInfo>> {
    // Phase A: everything that needs the clipboard open (kept short).
    let outcome = {
        if !open_clipboard_retry(hwnd) {
            bail!("could not open clipboard");
        }
        let guard = ClipboardGuard;
        let res = collect(cfg, png_format);
        drop(guard); // close ASAP, before any encoding/spawning
        res?
    };

    // Phase B: offload the expensive encode + write.
    match outcome {
        Some(Captured { info, job: Some(job) }) => {
            spawn_encode(job, cfg.max_dimension);
            Ok(Some(info))
        }
        Some(Captured { info, job: None }) => Ok(Some(info)),
        None => Ok(None),
    }
}

struct Captured {
    info: CaptureInfo,
    job: Option<EncodeJob>,
}

struct EncodeJob {
    path: PathBuf,
    payload: EncodePayload,
}

/// Runs with the clipboard open. Decides the content kind, grabs the minimum data
/// needed, re-populates the clipboard, and returns what to do next.
unsafe fn collect(cfg: &Config, png_format: u32) -> Result<Option<Captured>> {
    let has_files = is_avail(CF_HDROP);
    let has_png = png_format != 0 && is_avail(png_format);
    let has_dibv5 = is_avail(CF_DIBV5);
    let has_dib = is_avail(CF_DIB);

    // --- Files (videos, existing images, anything on disk) ---------------------
    if has_files {
        if !cfg.handle_files {
            return Ok(None);
        }
        let hdrop = GetClipboardData(CF_HDROP) as HDROP;
        if hdrop.is_null() {
            return Ok(None);
        }
        let paths = query_hdrop(hdrop);
        if paths.is_empty() {
            return Ok(None);
        }
        let text = format_paths(&paths, cfg.path_format, &cfg.multi_file_separator);
        if current_unicode_text().as_deref() == Some(text.as_str()) {
            return Ok(None); // already done
        }

        let raw_hdrop = get_clipboard_bytes(CF_HDROP);
        if EmptyClipboard() == 0 {
            bail!("EmptyClipboard failed");
        }
        if let Some(ref bytes) = raw_hdrop {
            let _ = set_clipboard_bytes(CF_HDROP, bytes);
        }
        set_unicode_text(&text)?;

        return Ok(Some(Captured {
            info: CaptureInfo {
                kind: CaptureKind::Files,
                text,
                file_path: None,
                file_count: paths.len(),
            },
            job: None,
        }));
    }

    // --- Image (in-memory bitmap from a screenshot / "copy image") -------------
    if has_png || has_dibv5 || has_dib {
        if let Some(existing) = current_unicode_text() {
            if looks_like_our_path(&existing, &cfg.save_dir) {
                return Ok(None); // already processed
            }
        }

        // The file payload: prefer the lossless PNG bytes; else blit the bitmap.
        let raw_png = if has_png {
            get_clipboard_bytes(png_format)
        } else {
            None
        };
        let payload = match raw_png {
            Some(ref bytes) => EncodePayload::Png(bytes.clone()),
            None => {
                let hbitmap = GetClipboardData(CF_BITMAP) as HBITMAP;
                if hbitmap.is_null() {
                    bail!("no CF_BITMAP available to decode");
                }
                let (w, h, data) = bitmap_to_bgrx(hbitmap)?;
                EncodePayload::Bgrx { w, h, data }
            }
        };

        // Only copy the (potentially large) DIB bytes if we'll re-set them.
        let (raw_dibv5, raw_dib) = if cfg.keep_image {
            (
                if has_dibv5 {
                    get_clipboard_bytes(CF_DIBV5)
                } else {
                    None
                },
                if has_dib {
                    get_clipboard_bytes(CF_DIB)
                } else {
                    None
                },
            )
        } else {
            (None, None)
        };

        let path = new_capture_path(&cfg.save_dir);
        let text = format_path(&path, cfg.path_format);

        if EmptyClipboard() == 0 {
            bail!("EmptyClipboard failed");
        }
        if cfg.keep_image {
            if let Some(ref b) = raw_dibv5 {
                let _ = set_clipboard_bytes(CF_DIBV5, b);
            }
            if let Some(ref b) = raw_dib {
                let _ = set_clipboard_bytes(CF_DIB, b);
            }
            if let Some(ref b) = raw_png {
                let _ = set_clipboard_bytes(png_format, b);
            }
        }
        set_unicode_text(&text)?;

        return Ok(Some(Captured {
            info: CaptureInfo {
                kind: CaptureKind::Image,
                text,
                file_path: Some(path.clone()),
                file_count: 0,
            },
            job: Some(EncodeJob { path, payload }),
        }));
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// Background encode worker
// ---------------------------------------------------------------------------

fn spawn_encode(job: EncodeJob, max_dimension: u32) {
    std::thread::spawn(move || {
        if let Err(e) = run_encode(&job, max_dimension) {
            log::warn!("encode of {} failed: {e:#}", job.path.display());
        } else {
            log::info!("saved {}", job.path.display());
        }
    });
}

fn run_encode(job: &EncodeJob, max_dimension: u32) -> Result<()> {
    match &job.payload {
        // Verbatim PNG passthrough — lossless, no resize (would require decoding).
        EncodePayload::Png(bytes) => std::fs::write(&job.path, bytes)
            .map_err(|e| anyhow!("writing {}: {e}", job.path.display())),
        EncodePayload::Bgrx { w, h, data } => {
            // GDI gives BGRX top-down with undefined alpha; make RGBA, force opaque.
            let mut rgba = data.clone();
            for px in rgba.chunks_exact_mut(4) {
                px.swap(0, 2);
                px[3] = 255;
            }
            let (ow, oh, out) = maybe_downscale(*w, *h, rgba, max_dimension);
            write_png(&job.path, ow, oh, &out)
        }
    }
}

/// Area-average downscale so the longer edge is at most `max_dimension`. No-op if
/// disabled or already small enough. Good quality for shrinking (text stays legible).
fn maybe_downscale(w: u32, h: u32, rgba: Vec<u8>, max_dimension: u32) -> (u32, u32, Vec<u8>) {
    let longest = w.max(h);
    if max_dimension == 0 || longest <= max_dimension || w == 0 || h == 0 {
        return (w, h, rgba);
    }
    let scale = max_dimension as f64 / longest as f64;
    let dw = ((w as f64 * scale).round() as u32).max(1);
    let dh = ((h as f64 * scale).round() as u32).max(1);

    let mut out = vec![0u8; (dw as usize) * (dh as usize) * 4];
    let (sw, sh) = (w as usize, h as usize);
    for dy in 0..dh as usize {
        let sy0 = dy * sh / dh as usize;
        let sy1 = (((dy + 1) * sh / dh as usize).max(sy0 + 1)).min(sh);
        for dx in 0..dw as usize {
            let sx0 = dx * sw / dw as usize;
            let sx1 = (((dx + 1) * sw / dw as usize).max(sx0 + 1)).min(sw);
            let (mut r, mut g, mut b, mut a, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
            for sy in sy0..sy1 {
                let row = sy * sw * 4;
                for sx in sx0..sx1 {
                    let i = row + sx * 4;
                    r += rgba[i] as u32;
                    g += rgba[i + 1] as u32;
                    b += rgba[i + 2] as u32;
                    a += rgba[i + 3] as u32;
                    n += 1;
                }
            }
            let n = n.max(1);
            let o = (dy * dw as usize + dx) * 4;
            out[o] = (r / n) as u8;
            out[o + 1] = (g / n) as u8;
            out[o + 2] = (b / n) as u8;
            out[o + 3] = (a / n) as u8;
        }
    }
    (dw, dh, out)
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    use std::io::BufWriter;
    let file =
        std::fs::File::create(path).map_err(|e| anyhow!("creating {}: {e}", path.display()))?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    // These captures are transient; favour encode speed over a few extra KB.
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Clipboard primitives
// ---------------------------------------------------------------------------

unsafe fn open_clipboard_retry(hwnd: HWND) -> bool {
    for _ in 0..10 {
        if OpenClipboard(hwnd) != 0 {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

unsafe fn is_avail(format: u32) -> bool {
    IsClipboardFormatAvailable(format) != 0
}

unsafe fn get_clipboard_bytes(format: u32) -> Option<Vec<u8>> {
    let handle = GetClipboardData(format);
    if handle.is_null() {
        return None;
    }
    let ptr = GlobalLock(handle as HGLOBAL);
    if ptr.is_null() {
        return None;
    }
    let size = GlobalSize(handle as HGLOBAL);
    let out = if size == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(ptr as *const u8, size).to_vec()
    };
    GlobalUnlock(handle as HGLOBAL);
    Some(out)
}

/// Set a clipboard format from raw bytes. Must follow `EmptyClipboard`.
unsafe fn set_clipboard_bytes(format: u32, data: &[u8]) -> bool {
    let len = data.len().max(1);
    let hmem = GlobalAlloc(GMEM_MOVEABLE, len);
    if hmem.is_null() {
        return false;
    }
    let ptr = GlobalLock(hmem);
    if ptr.is_null() {
        GlobalFree(hmem);
        return false;
    }
    if !data.is_empty() {
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
    }
    GlobalUnlock(hmem);
    if SetClipboardData(format, hmem as HANDLE).is_null() {
        GlobalFree(hmem);
        return false;
    }
    true
}

unsafe fn set_unicode_text(text: &str) -> Result<()> {
    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = std::slice::from_raw_parts(utf16.as_ptr() as *const u8, utf16.len() * 2);
    if !set_clipboard_bytes(CF_UNICODETEXT, bytes) {
        bail!("SetClipboardData(CF_UNICODETEXT) failed");
    }
    Ok(())
}

unsafe fn current_unicode_text() -> Option<String> {
    let handle = GetClipboardData(CF_UNICODETEXT);
    if handle.is_null() {
        return None;
    }
    let ptr = GlobalLock(handle as HGLOBAL) as *const u16;
    if ptr.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
    GlobalUnlock(handle as HGLOBAL);
    Some(s)
}

// ---------------------------------------------------------------------------
// Bitmap decode (UI thread — needs the clipboard-owned HBITMAP)
// ---------------------------------------------------------------------------

/// Blit `hbitmap` into a 32-bit top-down buffer via GDI. Returns raw BGRX; the
/// worker converts to RGBA. `GetDIBits` normalizes any source bit depth/compression.
unsafe fn bitmap_to_bgrx(hbitmap: HBITMAP) -> Result<(u32, u32, Vec<u8>)> {
    let mut bmp: BITMAP = std::mem::zeroed();
    let got = GetObjectW(
        hbitmap as HGDIOBJ,
        std::mem::size_of::<BITMAP>() as i32,
        &mut bmp as *mut _ as *mut c_void,
    );
    if got == 0 {
        bail!("GetObjectW failed on bitmap");
    }
    let (w, h) = (bmp.bmWidth, bmp.bmHeight);
    if w <= 0 || h <= 0 || w > 200_000 || h > 200_000 {
        bail!("unreasonable bitmap dimensions {w}x{h}");
    }
    let (wu, hu) = (w as u32, h as u32);

    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h; // top-down
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB as u32;

    let mut buf = vec![0u8; (wu as usize) * (hu as usize) * 4];
    let hdc = GetDC(std::ptr::null_mut());
    let scanlines = GetDIBits(
        hdc,
        hbitmap,
        0,
        hu,
        buf.as_mut_ptr() as *mut c_void,
        &mut bi as *mut BITMAPINFO,
        DIB_RGB_COLORS,
    );
    ReleaseDC(std::ptr::null_mut(), hdc);
    if scanlines == 0 {
        bail!("GetDIBits returned no scanlines");
    }
    Ok((wu, hu, buf))
}

// ---------------------------------------------------------------------------
// File-drop list
// ---------------------------------------------------------------------------

unsafe fn query_hdrop(hdrop: HDROP) -> Vec<PathBuf> {
    let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, std::ptr::null_mut(), 0);
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let len = DragQueryFileW(hdrop, i, std::ptr::null_mut(), 0);
        if len == 0 {
            continue;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let got = DragQueryFileW(hdrop, i, buf.as_mut_ptr(), buf.len() as u32);
        if got == 0 {
            continue;
        }
        buf.truncate(got as usize);
        out.push(PathBuf::from(OsString::from_wide(&buf)));
    }
    out
}

// ---------------------------------------------------------------------------
// Path / filename helpers
// ---------------------------------------------------------------------------

fn new_capture_path(save_dir: &Path) -> PathBuf {
    let now = chrono::Local::now();
    let base = now.format("screenshot_%Y%m%d_%H%M%S_%3f").to_string();
    let mut candidate = save_dir.join(format!("{base}.png"));
    let mut i = 1;
    while candidate.exists() {
        candidate = save_dir.join(format!("{base}_{i}.png"));
        i += 1;
    }
    candidate
}

pub fn format_path(path: &Path, fmt: PathFormat) -> String {
    let raw = path.to_string_lossy();
    match fmt {
        PathFormat::Plain => raw.replace('\\', "/"),
        PathFormat::Quoted => format!("\"{}\"", raw),
        PathFormat::Url => format!("file:///{}", raw.replace('\\', "/").replace(' ', "%20")),
    }
}

fn format_paths(paths: &[PathBuf], fmt: PathFormat, sep: &str) -> String {
    paths
        .iter()
        .map(|p| format_path(p, fmt))
        .collect::<Vec<_>>()
        .join(sep)
}

/// Heuristic: does this clipboard text already point into our save dir?
fn looks_like_our_path(text: &str, save_dir: &Path) -> bool {
    let t = text.trim().trim_matches('"');
    let t = t.strip_prefix("file:///").unwrap_or(t);
    let normalized = t.replace('/', "\\").to_lowercase();
    let dir = save_dir.to_string_lossy().replace('/', "\\").to_lowercase();
    normalized.starts_with(&dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_uses_forward_slashes() {
        let p = Path::new(r"C:\a\b\shot.png");
        assert_eq!(format_path(p, PathFormat::Plain), "C:/a/b/shot.png");
    }

    #[test]
    fn url_encodes_spaces() {
        let p = Path::new(r"C:\a b\shot.png");
        assert_eq!(format_path(p, PathFormat::Url), "file:///C:/a%20b/shot.png");
    }

    #[test]
    fn downscale_caps_longest_edge_and_is_noop_when_small() {
        let src = vec![128u8; 100 * 40 * 4];
        let (w, h, _) = maybe_downscale(100, 40, src.clone(), 50);
        assert_eq!((w, h), (50, 20));
        let (w2, h2, out2) = maybe_downscale(100, 40, src, 0);
        assert_eq!((w2, h2), (100, 40));
        assert_eq!(out2.len(), 100 * 40 * 4);
    }
}
