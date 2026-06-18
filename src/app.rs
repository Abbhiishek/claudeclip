//! The tray application: hidden listener window, system-tray icon, context menu,
//! and the Win32 message loop.

use crate::capture::{self, CaptureKind};
use crate::config::Config;
use crate::{autostart, util::wide};
use anyhow::{bail, Result};
use std::cell::Cell;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, GetClipboardSequenceNumber, RegisterClipboardFormatW,
    RemoveClipboardFormatListener,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NOTIFYICONDATAW, NOTIFY_ICON_DATA_FLAGS, NOTIFY_ICON_MESSAGE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DestroyWindow, DispatchMessageW, GetMessageW, GetSystemMetrics, GetWindowLongPtrW,
    KillTimer, LoadIconW, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SetForegroundWindow, SetTimer, SetWindowLongPtrW, TrackPopupMenu,
    TranslateMessage, CW_USEDEFAULT, GWLP_USERDATA, IDI_APPLICATION, MSG, WNDCLASSW, WS_OVERLAPPED,
};

/// The tray/app icons, embedded at compile time.
const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");
const ICON_PAUSED_PNG: &[u8] = include_bytes!("../assets/icon_paused.png");
const SM_CXSMICON: i32 = 49;
const SM_CYSMICON: i32 = 50;
const LR_DEFAULTCOLOR: u32 = 0;

// --- message / id constants ------------------------------------------------
const WMAPP_TRAYMSG: u32 = 0x8000 + 1; // WM_APP + 1
const TRAY_UID: u32 = 0x4343; // "CC"

const WM_DESTROY: u32 = 0x0002;
const WM_TIMER: u32 = 0x0113;
const WM_CLIPBOARDUPDATE: u32 = 0x031D;
const WM_NULL: u32 = 0x0000;
const WM_CONTEXTMENU: u32 = 0x007B;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_LBUTTONDBLCLK: u32 = 0x0203;

// Debounce: after a clipboard change, wait this long before reading so the
// source app (Snipping Tool, browsers, .NET) finishes its empty-then-set
// sequence. Avoids clipboard contention and coalesces rapid updates.
const TIMER_DEBOUNCE: usize = 1;
const DEBOUNCE_MS: u32 = 150;

// Shell_NotifyIcon
const NIM_ADD: NOTIFY_ICON_MESSAGE = 0;
const NIM_MODIFY: NOTIFY_ICON_MESSAGE = 1;
const NIM_DELETE: NOTIFY_ICON_MESSAGE = 2;
const NIM_SETVERSION: NOTIFY_ICON_MESSAGE = 4;
const NIF_MESSAGE: NOTIFY_ICON_DATA_FLAGS = 0x01;
const NIF_ICON: NOTIFY_ICON_DATA_FLAGS = 0x02;
const NIF_TIP: NOTIFY_ICON_DATA_FLAGS = 0x04;
const NIF_INFO: NOTIFY_ICON_DATA_FLAGS = 0x10;
const NIF_SHOWTIP: NOTIFY_ICON_DATA_FLAGS = 0x80;
const NIIF_INFO: u32 = 0x01;
const NOTIFYICON_VERSION_4: u32 = 4;

// Menu flags
const MF_STRING: u32 = 0x0000;
const MF_DISABLED: u32 = 0x0002;
const MF_GRAYED: u32 = 0x0001;
const MF_SEPARATOR: u32 = 0x0800;
const MF_CHECKED: u32 = 0x0008;

// TrackPopupMenu flags
const TPM_RIGHTBUTTON: u32 = 0x0002;
const TPM_RETURNCMD: u32 = 0x0100;
const TPM_NONOTIFY: u32 = 0x0080;

// MessageBox flags
const MB_OK: u32 = 0x0000;
const MB_ICONINFORMATION: u32 = 0x0040;

// Menu command ids
const ID_TOGGLE_ENABLED: usize = 1;
const ID_AUTOSTART: usize = 2;
const ID_OPEN_FOLDER: usize = 3;
const ID_OPEN_CONFIG: usize = 4;
const ID_OPEN_LOG: usize = 5;
const ID_ABOUT: usize = 6;
const ID_EXIT: usize = 7;

/// Shared application state. Lives on the heap for the duration of the message
/// loop; a raw pointer is stashed in the window's `GWLP_USERDATA`.
///
/// Single-threaded (UI thread only), so `Cell` is enough for mutability.
pub struct App {
    pub hwnd: Cell<HWND>,
    pub config: Config,
    pub config_path: PathBuf,
    pub log_path: PathBuf,
    pub enabled: Cell<bool>,
    pub last_self_seq: Cell<u32>,
    pub png_format: u32,
    pub hicon_active: isize,
    pub hicon_paused: isize,
    pub taskbar_created_msg: u32,
}

/// Build the window, register the clipboard listener, add the tray icon, and run
/// the message loop until quit.
pub fn run_app(config: Config, config_path: PathBuf, log_path: PathBuf) -> Result<()> {
    unsafe {
        let hicon_active = load_icon(ICON_PNG);
        let hicon_paused = load_icon(ICON_PAUSED_PNG);
        let png_format = RegisterClipboardFormatW(wide("PNG").as_ptr());
        let taskbar_created_msg = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());

        let app = Box::new(App {
            hwnd: Cell::new(std::ptr::null_mut()),
            config,
            config_path,
            log_path,
            enabled: Cell::new(true),
            last_self_seq: Cell::new(0),
            png_format,
            hicon_active,
            hicon_paused,
            taskbar_created_msg,
        });
        let app_ptr = Box::into_raw(app);

        let hwnd = create_window();
        if hwnd.is_null() {
            drop(Box::from_raw(app_ptr));
            bail!("CreateWindowExW failed");
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app_ptr as isize);
        (*app_ptr).hwnd.set(hwnd);

        if AddClipboardFormatListener(hwnd) == 0 {
            log::warn!("AddClipboardFormatListener failed");
        }
        add_tray(&*app_ptr);
        log::info!(
            "running; save_dir={}",
            (*app_ptr).config.save_dir.display()
        );

        let mut msg: MSG = std::mem::zeroed();
        loop {
            let ret = GetMessageW(&mut msg as *mut MSG, std::ptr::null_mut(), 0, 0);
            if ret <= 0 {
                break; // 0 = WM_QUIT, -1 = error
            }
            TranslateMessage(&msg as *const MSG);
            DispatchMessageW(&msg as *const MSG);
        }

        RemoveClipboardFormatListener(hwnd);
        drop(Box::from_raw(app_ptr));
    }
    Ok(())
}

/// Build an `HICON` from an embedded PNG, scaled to the small-icon size. Falls
/// back to the stock application icon if decoding fails.
unsafe fn load_icon(png: &[u8]) -> isize {
    let cx = GetSystemMetrics(SM_CXSMICON);
    let cy = GetSystemMetrics(SM_CYSMICON);
    let hicon = CreateIconFromResourceEx(
        png.as_ptr(),
        png.len() as u32,
        1, // fIcon
        0x0003_0000,
        cx,
        cy,
        LR_DEFAULTCOLOR,
    );
    if !hicon.is_null() {
        return hicon as isize;
    }
    log::warn!("CreateIconFromResourceEx failed; using stock icon");
    LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) as isize
}

/// The icon matching the current watching/paused state.
fn current_icon(app: &App) -> isize {
    if app.enabled.get() {
        app.hicon_active
    } else {
        app.hicon_paused
    }
}

unsafe fn create_window() -> HWND {
    let hinstance = GetModuleHandleW(std::ptr::null());
    let class_name = wide("ClaudeClipWndClass");

    let mut wc: WNDCLASSW = std::mem::zeroed();
    wc.lpfnWndProc = Some(wndproc);
    wc.hInstance = hinstance;
    wc.lpszClassName = class_name.as_ptr();
    RegisterClassW(&wc as *const WNDCLASSW);

    let title = wide("ClaudeClip");
    // A normal (never-shown) top-level window — needed to receive the
    // "TaskbarCreated" broadcast so we can re-add the tray icon if Explorer
    // restarts. A message-only window would not get that broadcast.
    CreateWindowExW(
        0,
        class_name.as_ptr(),
        title.as_ptr(),
        WS_OVERLAPPED,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        0,
        0,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        hinstance,
        std::ptr::null(),
    )
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if !app_ptr.is_null() {
        let app = &*app_ptr;
        match msg {
            WM_CLIPBOARDUPDATE => {
                on_clipboard_update(app, hwnd);
                return 0;
            }
            WM_TIMER => {
                if wparam == TIMER_DEBOUNCE {
                    KillTimer(hwnd, TIMER_DEBOUNCE);
                    process_clipboard_now(app, hwnd);
                }
                return 0;
            }
            WMAPP_TRAYMSG => {
                on_tray_message(app, hwnd, wparam, lparam);
                return 0;
            }
            WM_DESTROY => {
                remove_tray(app);
                PostQuitMessage(0);
                return 0;
            }
            _ => {
                if msg == app.taskbar_created_msg {
                    add_tray(app);
                    return 0;
                }
            }
        }
    } else if msg == WM_DESTROY {
        PostQuitMessage(0);
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Fast path on the actual clipboard event: filter out our own writes, then
/// (re)arm the debounce timer. Reading the clipboard is deferred to `WM_TIMER`.
fn on_clipboard_update(app: &App, hwnd: HWND) {
    if !app.enabled.get() {
        return;
    }
    let seq = unsafe { GetClipboardSequenceNumber() };
    if seq != 0 && seq == app.last_self_seq.get() {
        return; // our own write — ignore
    }
    // (Re)start the timer; a fresh update within the window resets it.
    unsafe {
        SetTimer(hwnd, TIMER_DEBOUNCE, DEBOUNCE_MS, None);
    }
}

/// Debounced handler: actually inspect and transform the clipboard.
fn process_clipboard_now(app: &App, hwnd: HWND) {
    if !app.enabled.get() {
        return;
    }
    match unsafe { capture::process_clipboard(hwnd, &app.config, app.png_format) } {
        Ok(Some(info)) => {
            // Remember the sequence number we produced so the resulting
            // WM_CLIPBOARDUPDATE for our own write is ignored.
            app.last_self_seq.set(unsafe { GetClipboardSequenceNumber() });
            log::info!("captured ({:?}) -> {}", info.kind, info.text);
            notify_capture(app, &info);
        }
        Ok(None) => {}
        Err(e) => log::warn!("clipboard processing failed: {e:#}"),
    }
}

fn notify_capture(app: &App, info: &capture::CaptureInfo) {
    if !app.config.notify_on_capture {
        return;
    }
    let (title, body) = match info.kind {
        CaptureKind::Image => {
            let name = info
                .file_path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            (
                "Screenshot captured".to_string(),
                format!("{name}\nPath copied — paste into Claude Code."),
            )
        }
        CaptureKind::Files => (
            "Path copied".to_string(),
            format!(
                "{} file path(s) on clipboard — paste into Claude Code.",
                info.file_count
            ),
        ),
    };
    unsafe { show_balloon(app, &title, &body) };
}

// ---------------------------------------------------------------------------
// Tray icon
// ---------------------------------------------------------------------------

unsafe fn base_nid(app: &App) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = app.hwnd.get();
    nid.uID = TRAY_UID;
    nid
}

unsafe fn add_tray(app: &App) {
    let mut nid = base_nid(app);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = WMAPP_TRAYMSG;
    nid.hIcon = current_icon(app) as windows_sys::Win32::UI::WindowsAndMessaging::HICON;
    copy_wide(&mut nid.szTip, tray_tip(app));
    Shell_NotifyIconW(NIM_ADD, &nid as *const NOTIFYICONDATAW);

    // Opt into v4 behaviour (richer callback coordinates).
    nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    Shell_NotifyIconW(NIM_SETVERSION, &nid as *const NOTIFYICONDATAW);
}

/// Reflect the current watching/paused state in the tray icon + tooltip.
unsafe fn update_tray_icon(app: &App) {
    let mut nid = base_nid(app);
    nid.uFlags = NIF_ICON | NIF_TIP;
    nid.hIcon = current_icon(app) as windows_sys::Win32::UI::WindowsAndMessaging::HICON;
    copy_wide(&mut nid.szTip, tray_tip(app));
    Shell_NotifyIconW(NIM_MODIFY, &nid as *const NOTIFYICONDATAW);
}

fn tray_tip(app: &App) -> &'static str {
    if app.enabled.get() {
        "ClaudeClip — watching clipboard"
    } else {
        "ClaudeClip — paused"
    }
}

unsafe fn remove_tray(app: &App) {
    let nid = base_nid(app);
    Shell_NotifyIconW(NIM_DELETE, &nid as *const NOTIFYICONDATAW);
}

unsafe fn show_balloon(app: &App, title: &str, body: &str) {
    let mut nid = base_nid(app);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;
    copy_wide(&mut nid.szInfoTitle, title);
    copy_wide(&mut nid.szInfo, body);
    Shell_NotifyIconW(NIM_MODIFY, &nid as *const NOTIFYICONDATAW);
}

/// Copy a string into a fixed-size UTF-16 buffer, leaving room for a NUL.
fn copy_wide(dst: &mut [u16], s: &str) {
    let src: Vec<u16> = s.encode_utf16().collect();
    let n = src.len().min(dst.len().saturating_sub(1));
    dst[..n].copy_from_slice(&src[..n]);
    dst[n] = 0;
}

// ---------------------------------------------------------------------------
// Tray interaction / context menu
// ---------------------------------------------------------------------------

unsafe fn on_tray_message(app: &App, hwnd: HWND, wparam: WPARAM, lparam: LPARAM) {
    let event = (lparam as u32) & 0xFFFF;
    match event {
        e if e == WM_CONTEXTMENU || e == WM_RBUTTONUP => {
            // With v4, the anchor point is packed into wParam.
            let x = (wparam & 0xFFFF) as i16 as i32;
            let y = ((wparam >> 16) & 0xFFFF) as i16 as i32;
            show_menu(app, hwnd, x, y);
        }
        e if e == WM_LBUTTONDBLCLK => {
            open_folder(app);
        }
        _ => {}
    }
}

unsafe fn show_menu(app: &App, hwnd: HWND, x: i32, y: i32) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }

    append(menu, MF_STRING | MF_DISABLED | MF_GRAYED, 0, "ClaudeClip");
    append_separator(menu);

    let enabled_flags = MF_STRING | if app.enabled.get() { MF_CHECKED } else { 0 };
    append(menu, enabled_flags, ID_TOGGLE_ENABLED, "Watching clipboard");

    let autostart_on = autostart::is_enabled();
    let autostart_flags = MF_STRING | if autostart_on { MF_CHECKED } else { 0 };
    append(menu, autostart_flags, ID_AUTOSTART, "Start with Windows");

    append_separator(menu);
    append(menu, MF_STRING, ID_OPEN_FOLDER, "Open screenshots folder");
    append(menu, MF_STRING, ID_OPEN_CONFIG, "Edit settings…");
    append(menu, MF_STRING, ID_OPEN_LOG, "Open log");
    append_separator(menu);
    append(menu, MF_STRING, ID_ABOUT, "About");
    append(menu, MF_STRING, ID_EXIT, "Quit ClaudeClip");

    // Required so the menu dismisses correctly when clicking elsewhere.
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
        x,
        y,
        0,
        hwnd,
        std::ptr::null(),
    );
    DestroyMenu(menu);
    PostMessageW(hwnd, WM_NULL, 0, 0);

    match cmd as usize {
        ID_TOGGLE_ENABLED => {
            let now = !app.enabled.get();
            app.enabled.set(now);
            update_tray_icon(app);
            log::info!("watching = {now}");
        }
        ID_AUTOSTART => {
            if let Err(e) = autostart::set_enabled(!autostart_on) {
                log::warn!("autostart toggle failed: {e:#}");
            }
        }
        ID_OPEN_FOLDER => open_folder(app),
        ID_OPEN_CONFIG => {
            let _ = std::process::Command::new("notepad")
                .arg(&app.config_path)
                .spawn();
        }
        ID_OPEN_LOG => {
            let _ = std::process::Command::new("notepad")
                .arg(&app.log_path)
                .spawn();
        }
        ID_ABOUT => show_about(hwnd),
        ID_EXIT => {
            DestroyWindow(hwnd);
        }
        _ => {}
    }
}

fn open_folder(app: &App) {
    let _ = std::process::Command::new("explorer")
        .arg(&app.config.save_dir)
        .spawn();
}

unsafe fn append(menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU, flags: u32, id: usize, text: &str) {
    let w = wide(text);
    AppendMenuW(menu, flags, id, w.as_ptr());
}

unsafe fn append_separator(menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU) {
    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
}

unsafe fn show_about(hwnd: HWND) {
    let text = wide(
        "ClaudeClip\n\nWatches the clipboard for screenshots and images, saves them \
         to a folder, and puts the file path on the clipboard so you can paste it \
         straight into Claude Code.\n\nRight-click the tray icon for options.",
    );
    let title = wide("About ClaudeClip");
    MessageBoxW(hwnd, text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONINFORMATION);
}
