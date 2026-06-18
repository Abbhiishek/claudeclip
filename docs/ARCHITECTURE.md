# Architecture

ClaudeClip is a single-process Windows tray daemon. It listens for clipboard
changes, turns incoming **images** into PNG files and **copied files** into text
paths, and augments the clipboard so the path can be pasted into a terminal while
the original content is preserved.

## Module map

| File | Responsibility |
|------|----------------|
| [`src/main.rs`](../src/main.rs) | Entry point. Arg parsing (`Run` / `Install` / `Uninstall` / `Help`), single-instance mutex, logging setup, config load, retention-sweep thread, dispatch. |
| [`src/config.rs`](../src/config.rs) | `Config` struct, TOML load/save, `PathFormat` enum, default save dir. |
| [`src/app.rs`](../src/app.rs) | The hidden listener window, `wndproc`, tray icon + context menu, debounce timer, capture-event glue, balloon notifications, runtime icon. |
| [`src/capture.rs`](../src/capture.rs) | Win32 clipboard primitives, image decode (`CF_BITMAP` → `GetDIBits` → PNG), `CF_HDROP` parsing, path formatting, and the `process_clipboard` transform. |
| [`src/installer.rs`](../src/installer.rs) | Copy to `%LOCALAPPDATA%`, autostart wiring, stop-other-instances. |
| [`src/autostart.rs`](../src/autostart.rs) | HKCU `…\Run` registry entry. |
| [`src/util.rs`](../src/util.rs) | `wide()` UTF-16 helper. |
| [`build.rs`](../build.rs) | Embeds icon + manifest via a direct `windres` call (works around spaces in the project path). |

## Data flow

```
 Win+Shift+S / copy
        │
        ▼
  Windows clipboard  ──(WM_CLIPBOARDUPDATE)──►  wndproc
        ▲                                          │
        │                                  seq == our last write?  ──yes──► ignore
        │                                          │ no
        │                                   SetTimer(150 ms)   ← debounce; resets on each new update
        │                                          │
        │                                    (WM_TIMER)
        │                                          ▼
        │                                  process_clipboard()
        │                                          │
        │             ┌────────────────────────────┼─────────────────────────────┐
        │             ▼                             ▼                              ▼
        │        CF_HDROP?                     image format?                  else → ignore
        │      (files/videos)            (PNG / CF_DIBV5 / CF_DIB)
        │             │                             │
        │      read paths               PNG passthrough, else
        │             │              CF_BITMAP → GetDIBits → encode PNG → save file
        │             │                             │
        └─── EmptyClipboard → re-set original formats + add path as CF_UNICODETEXT
                                                    │
                                          record sequence number
                                                    ▼
                                           tray balloon (optional)
```

## Threading model

- **UI thread** (the message loop) does *all* window, tray, and clipboard work.
  Win32 clipboard and tray APIs are thread-affine, and the work per event is
  small, so this keeps things simple and race-free. State mutated from here uses
  `Cell` (no locks needed — single thread).
- **Retention thread**: one background thread wakes hourly to delete our own
  captures older than `retention_days`. It only ever removes files matching
  `screenshot_*.png` in `save_dir`.

`App` lives on the heap; a raw pointer is stored in the window's
`GWLP_USERDATA` and recovered in `wndproc`. It is freed when the loop exits.

## Key design decisions

- **Event-driven, not polling.** `AddClipboardFormatListener` pushes
  `WM_CLIPBOARDUPDATE`; idle CPU is ~0%.
- **Debounce (150 ms).** Apps like Snipping Tool and .NET set the clipboard in an
  empty-then-set sequence. Reading immediately collides with them
  (`SetImage` → "Clipboard operation did not succeed"). Waiting briefly lets the
  source finish and coalesces bursts.
- **Augment, don't replace.** `EmptyClipboard` clears everything, so we read the
  original image/file formats first and re-set them verbatim alongside the path
  text. Terminal paste → path; image-app paste → image.
- **Loop prevention.** `GetClipboardSequenceNumber()` lets us skip the
  `WM_CLIPBOARDUPDATE` triggered by our own write; an idempotency check on the
  text is a backstop.
- **Universal decode.** Pulling `CF_BITMAP` (system-synthesized from any DIB) and
  running `GetDIBits` into a 32-bit top-down buffer sidesteps hand-parsing every
  DIB variant (bit depth, compression, top-down/bottom-up, palettes).
- **Local save dir.** Captures are transient and pruned, so the default avoids
  OneDrive-synced Pictures.
- **Self-contained install.** No external installer: `--install` copies the exe
  to a stable location and wires autostart there. A running exe can't be
  overwritten but *can* be renamed, so updates rename the old binary aside.

## Edge cases handled

- Clipboard locked by another process → retry (`OpenClipboard`, 10× / 20 ms).
- Oversized/garbage bitmap dimensions → rejected.
- Filename collisions → millisecond timestamp + numeric suffix.
- Empty clipboard formats → skipped safely.
- Plain text copies → never modified.
- Explorer restart → tray icon re-added on the `TaskbarCreated` broadcast.
- Resource compiler missing → build still succeeds (runtime PNG icon fallback).
- Fatal startup error → logged and shown in a dialog.

## Extension points

- **New clipboard kinds**: add a branch in `process_clipboard`.
- **New output text shapes**: extend `PathFormat` and `format_path`.
- **Settings UI**: today the tray opens `config.toml` in Notepad; a dialog could
  replace it without touching the capture core.
- **Encode offloading**: `process_clipboard` could hand raw bytes to a worker so
  the clipboard-open critical section stays microscopic (see ROADMAP).
