# Roadmap & ideas

Prioritized backlog. Tiers are about value-vs-effort, not commitment.

## Shipped

- **Background encode + write.** The clipboard is read/re-set on the UI thread and
  closed immediately; PNG decode/encode/write happen on a worker thread, so the
  clipboard-open window is ~microseconds and encode latency is off the message
  loop. The path is on the clipboard at once; the file lands a few ms later.
- **Skip the DIB copy when `keep_image = false`** — avoids a large allocation
  (a 4K `CF_DIB` is ~33 MB) when we aren't going to re-set it.
- **Faster PNG compression** (`png::Compression::Fast`).
- **Paused-state icon + tooltip** — the tray icon greys out when watching is off.
- **Optional max-dimension downscale** (`max_dimension` config) — area-average
  resize so the longer edge is capped, shrinking the image-token payload sent to
  Claude. Off by default (pixel-exact); set e.g. `2000` to enable.

## P0 — quick, high-value

- **"Copy last path again" / "Open last capture"** tray items.

## P1 — clearly useful

- **Format choice: PNG / JPEG / WebP.** WebP/JPEG dramatically shrink photographic
  screenshots; PNG stays default for crisp UI shots.
- **Config hot-reload.** Watch `config.toml` and apply changes without a restart.
- **Filename templates** (`{date}`, `{app}`, `{seq}`) and optional per-day subfolders.
- **Size-based retention.** Prune by total folder size / file count, in addition to age.
- **Markdown format option** (`![](path)`) for pasting into docs/issues.
- **Context-aware formatting.** Inspect the foreground window: terminal → plain
  path, markdown editor → `![]()`, etc.
- **Settings dialog.** A real window instead of editing TOML in Notepad.
- **OCR sidecar (optional).** Run Windows.Media.Ocr on captures and stash the
  recognized text (e.g. a `.txt` next to the image, or a tray "copy text" action).

## P2 — ambitious / distribution

- **Optional cloud upload → real URL.** The original ask mentioned a "URL." An
  opt-in uploader (S3 / personal endpoint / paste host) could return an `https://`
  link instead of a local path, for sharing. Needs auth + privacy controls.
- **Capture history viewer.** A small window with thumbnails of recent captures;
  click to re-copy a path or open the file.
- **Global hotkey capture mode.** A hotkey that converts the current clipboard
  image on demand — for people who don't want fully automatic behaviour.
- **Post-capture hook.** Run a user command/webhook with the saved path.
- **Distribution & trust.** winget / Scoop manifests, an MSI, and code-signing to
  avoid SmartScreen warnings. Auto-update.
- **Tests & CI.** Unit tests for `format_path` / DIB handling; GitHub Actions to
  build and run the STA integration tests.

## Explicitly out of scope (for now)

- Cross-platform (macOS/Linux): the clipboard + tray layers are Win32-specific.
  A trait-based clipboard backend could enable it later, but it's a rewrite of the
  platform layer.
- Editing/annotation: that's the screenshot tool's job.
