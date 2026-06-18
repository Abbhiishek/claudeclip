# ClaudeClip

A tiny Windows tray app that turns clipboard **screenshots and images into a
pasteable file path** — so you can `Win+Shift+S`, then `Ctrl+V` straight into
Claude Code (or any terminal) and have it pick up the image.

When you take a screenshot, Windows puts the *pixels* on the clipboard, not a
file. Terminals can't paste pixels, so normally you have to save the image and
drag the file in. ClaudeClip closes that gap: it watches the clipboard, saves
incoming images to a folder as PNG, and adds the saved file's path to the
clipboard as text — **without removing the image**, so pasting into design tools
still works as before.

It also surfaces **copied files** (videos, existing images, anything copied from
Explorer): their on-disk paths are added to the clipboard as text too.

## How it works

- A hidden window registers a clipboard listener (`AddClipboardFormatListener`) —
  event-driven, **zero CPU when idle**, no polling.
- On a clipboard change it waits ~150 ms (debounce) so the source app finishes
  writing, then inspects the clipboard:
  - **Image** (`CF_DIB`/`CF_DIBV5`, or a raw `PNG` clipboard format): decoded via
    GDI (`GetDIBits`, so any bit depth/compression works) and saved as PNG. A raw
    `PNG` format, if present, is written byte-for-byte (lossless, preserves alpha).
  - **Files** (`CF_HDROP`): their paths are read directly.
- The clipboard is then re-populated: the original image/file formats are kept,
  and the path is added as `CF_UNICODETEXT`.
- It ignores its own writes (clipboard sequence number) and **never touches plain
  text** you copy.

## Install

Download/build `claude-clip.exe`, then run once:

```
claude-clip.exe --install
```

This copies the app to `%LOCALAPPDATA%\ClaudeClip\`, registers it to start on
login, and launches it. A 📋-style icon appears in your system tray.

- `claude-clip.exe --install --silent` — install with no dialog (for scripts).
- `claude-clip.exe --uninstall` — remove autostart and the installed copy
  (your config and saved screenshots are left in place).
- `claude-clip.exe --help` — usage.

> Running `claude-clip.exe` with no arguments just runs it from wherever it is
> (portable mode), without installing or enabling autostart.

## Using it

1. Take a screenshot (`Win+Shift+S`) or copy any image/file.
2. Paste (`Ctrl+V`) into Claude Code.

A brief tray balloon confirms each capture. Pasting into a terminal yields the
path text; pasting into an image app (Figma, Slack, Word) still yields the image.

## Tray menu (right-click the icon)

- **Watching clipboard** — pause/resume capturing.
- **Start with Windows** — toggle launch-on-login.
- **Open screenshots folder** — open the save directory.
- **Edit settings…** — open `config.toml` in Notepad.
- **Open log** — open the log file.
- **About** / **Quit ClaudeClip**.

Double-click the tray icon to open the screenshots folder.

## Configuration

`%APPDATA%\ClaudeClip\config.toml` (created on first run). Edit and restart the
app to apply.

| Key | Default | Meaning |
|-----|---------|---------|
| `save_dir` | `%LOCALAPPDATA%\ClaudeClip\captures` | Where images are saved. Set to e.g. your Pictures folder if you want them kept there. |
| `retention_days` | `7` | Auto-delete our saved screenshots older than this (0 = never). |
| `path_format` | `"plain"` | `plain` = `C:/path/shot.png` (forward slashes), `quoted` = `"C:\path\shot.png"`, `url` = `file:///C:/path/shot.png`. |
| `max_dimension` | `0` | Cap the longer edge of saved images to this many pixels (`0` = no resize). Set to e.g. `2000` to shrink the image-token payload sent to Claude; leave `0` for pixel-exact captures. |
| `keep_image` | `true` | Keep the image on the clipboard alongside the path text. |
| `notify_on_capture` | `true` | Show a tray balloon on each capture. |
| `handle_files` | `true` | Also add a text path for copied files (videos, etc.). |
| `multi_file_separator` | `"\n"` | Joiner when multiple files are copied at once. |

> The default save location is **local** (not OneDrive-synced) on purpose —
> these captures are transient and pruned after `retention_days`, so syncing
> them to the cloud is wasteful. Change `save_dir` if you'd rather keep them in
> Pictures.

## Logs & files

- Config + log: `%APPDATA%\ClaudeClip\` (`config.toml`, `claude-clip.log`)
- Saved images: `save_dir` (default `%LOCALAPPDATA%\ClaudeClip\captures`)
- Installed binary: `%LOCALAPPDATA%\ClaudeClip\claude-clip.exe`

## Building from source

Requires the Rust toolchain. On the GNU toolchain, `windres` (MinGW binutils) is
used to embed the icon + manifest; if it's missing the build still succeeds (the
tray icon is created at runtime from an embedded PNG either way).

```
cargo build --release
# binary at target/release/claude-clip.exe
target\release\claude-clip.exe --install
```

### Tests

`scripts/` contains STA PowerShell integration tests that drive a running
instance through real clipboard operations:

```
# with an instance running:
powershell.exe -STA -File scripts\integration-test.ps1        # image capture
powershell.exe -STA -File scripts\integration-test-files.ps1  # file-drop handling
```

## Troubleshooting

- **Path doesn't attach in Claude Code** — try a different `path_format` in
  `config.toml`. `plain` (forward slashes) is the most broadly accepted.
- **Two icons after an update** — the old icon clears on hover, or restart.
- **Nothing happens** — check `Watching clipboard` is enabled in the tray menu,
  and see `claude-clip.log`.

## Releasing

Releases are automated by [`.github/workflows/release.yml`](.github/workflows/release.yml).
On every push to `main` it reads the version from `Cargo.toml`; if a `v<version>`
release doesn't exist yet, it builds on Windows and publishes that release with
the `claude-clip-v<version>.exe` binary attached. Ordinary commits are no-ops.

**To cut a release:** bump `version` in `Cargo.toml`, commit, and push to `main`.

```toml
# Cargo.toml
[package]
version = "0.2.0"   # ← bump this
```

The first push of the workflow publishes the current version (`v0.1.0`). To
remove a release: `gh release delete v0.1.0 --cleanup-tag`.

## Documentation

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — module map, data flow, threading
  model, and the reasoning behind each design decision.
- [docs/ROADMAP.md](docs/ROADMAP.md) — prioritized feature ideas and performance
  work.

## License

MIT
