# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

ClaudeClip is a Windows-only system-tray daemon (Rust) that watches the clipboard,
saves incoming screenshots/images to PNG files, and adds the saved file's **path**
to the clipboard as text — so a screenshot can be pasted into a terminal (Claude
Code) as a path. Copied files (`CF_HDROP`) get their paths surfaced as text too.

The full design write-up is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md); the
feature backlog is in [docs/ROADMAP.md](docs/ROADMAP.md). Read ARCHITECTURE.md
before non-trivial changes.

## Commands

```sh
cargo build                 # debug build (keeps a console + console logs)
cargo build --release       # release build -> target/release/claude-clip.exe (no console)
cargo test                  # unit tests (pure fns: path formatting, downscale math)
cargo test url_encodes_spaces   # run a single test by name

# The release exe is also the installer:
target\release\claude-clip.exe --install [--silent]   # copy to %LOCALAPPDATA%, autostart, launch
target\release\claude-clip.exe --uninstall [--silent]
```

### Integration tests (real clipboard, manual)

These drive a **running instance** through real clipboard ops, so they need an
interactive desktop and an **STA** PowerShell host. They will NOT work in
headless CI (no interactive session). Start an instance first, then:

```powershell
powershell.exe -STA -File scripts\integration-test.ps1            # image capture
powershell.exe -STA -File scripts\integration-test-files.ps1      # file-drop handling
powershell.exe -STA -File scripts\integration-test-downscale.ps1 -MaxDim 200   # requires max_dimension=200 in config
```

The single-instance mutex blocks a second copy, so when (re)launching for a test,
kill the old one first: `Get-Process claude-clip | Stop-Process -Force`.

## Runtime locations (not in the repo)

- Config + log: `%APPDATA%\ClaudeClip\` (`config.toml`, `claude-clip.log`)
- Saved captures: `save_dir` (default `%LOCALAPPDATA%\ClaudeClip\captures`)
- Installed binary: `%LOCALAPPDATA%\ClaudeClip\claude-clip.exe`

Config changes require an app restart (no hot-reload). `config.toml` is created
with defaults on first run; missing fields fall back to `Config::default()` via
`#[serde(default)]`, so adding a field is backward compatible.

## Architecture essentials

**Event-driven, single UI thread.** A hidden window (`src/app.rs`) registers
`AddClipboardFormatListener`; `WM_CLIPBOARDUPDATE` drives everything. There is no
polling. All Win32 clipboard/tray work happens on this one thread (those APIs are
thread-affine), so shared state uses `Cell`/`RefCell`, not locks. `App` lives on
the heap with a raw pointer stashed in `GWLP_USERDATA`.

**The capture pipeline** (`src/capture.rs::process_clipboard`):
1. `WM_CLIPBOARDUPDATE` → ignore if it's our own write (clipboard sequence
   number) → arm a **150 ms debounce timer**. The debounce is load-bearing:
   apps set the clipboard in an empty-then-set sequence, and reading immediately
   collides with them (real `SetImage` "Clipboard operation did not succeed"
   races). Don't remove it.
2. On `WM_TIMER`, open the clipboard, do only the cheap work (read bytes / blit
   the bitmap via `CF_BITMAP`+`GetDIBits`, re-set formats, set the path text),
   and **close it immediately**.
3. The expensive PNG encode + file write are offloaded to a worker thread
   (`spawn_encode`). The path is on the clipboard at once; the file lands a few
   ms later.

**Augment, never replace.** `EmptyClipboard` clears *all* formats, so we read the
original image/file formats first and re-set them verbatim alongside the path
text (`CF_UNICODETEXT`). Result: terminal paste → path; image-app paste → image.
Plain text the user copies is never touched.

**Module relationships:** `main.rs` (arg dispatch, single-instance mutex, logging,
retention thread) → `app.rs` (window/tray/menu/debounce, the only caller of
`capture::process_clipboard`) → `capture.rs` (all clipboard FFI + the transform).
`config.rs`, `autostart.rs` (HKCU `Run` key), `installer.rs` are leaf helpers.

## Critical gotchas

- **GNU toolchain, not MSVC.** The default/required toolchain is
  `stable-x86_64-pc-windows-gnu`. `build.rs` embeds the icon + manifest by calling
  `windres` (a GNU tool). CI uses GNU for the same reason.
- **`build.rs` hand-rolls `windres` on purpose.** The project path contains a
  space (`...\mini projects\...`), which breaks `winresource`/the C preprocessor
  (`-I` is split on the space). The workaround runs `windres` from the `assets/`
  dir with *relative* references. Do not replace it with `winresource`. Resource
  embedding is non-fatal — if `windres` is missing the build still succeeds (the
  tray icon is also created at runtime from `assets/icon.png` via
  `CreateIconFromResourceEx`).
- **Win32 bindings** use `windows-sys` (raw FFI). Standard clipboard format
  constants (`CF_DIB`, etc.) are defined locally in `capture.rs` to keep the
  feature surface small. Some functions live in surprising modules (e.g.
  `GlobalFree` is in `Win32::Foundation`, `GetDC`/`ReleaseDC` in `Graphics::Gdi`);
  `CreateMutexW` needs the `Win32_Security` feature for `SECURITY_ATTRIBUTES`.

## Releases & CI

`.github/workflows/release.yml` runs on push to `main`: it reads the version from
`Cargo.toml` and, only if a `v<version>` release doesn't already exist, builds and
publishes it. **To cut a release, bump `version` in `Cargo.toml` and push.**
Ordinary pushes are no-ops.

CI gotcha: in GitHub's `pwsh` steps, a native command's non-zero exit (e.g.
`gh release view` for a missing release) leaks as the step's exit code and fails
the step — end such steps with `exit 0`.
