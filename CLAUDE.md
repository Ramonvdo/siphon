# Siphon — project guide for Claude

Siphon is a **Tauri v2 + Rust + vanilla JS/CSS** desktop media downloader. Paste a URL,
pick a format (MP4 / MP3 / WAV), and it downloads + converts the media locally. It is a
native rewrite of an earlier Node/Express downloader.

## What it does

- Paste any media URL → export **MP4**, **MP3**, or **WAV**.
- **Preview** metadata (title, thumbnail, duration, source) before downloading; clicking
  **Download** also auto-previews so every queue card has a thumbnail.
- **Direct media** links (`.mp4`, `.mp3`, `.wav`, …) are streamed with `reqwest` and
  converted with ffmpeg when the container doesn't match the chosen format.
- **Page URLs** are handled by `yt-dlp` (download + merge), final file written to the
  chosen folder. Same URL guards as the original (rejects YouTube search/channel pages).
- **Advanced Search** tab: for pages that won't download because the real media is an
  HLS/DASH manifest loaded by page JS (e.g. Headspace). Opens the page in a finder WebView
  (hidden first, revealed after ~10s so the user can log in / press play), sniffs its network
  at the native WebView2 layer for any `.m3u8`/`.mpd`, and lists found streams live in the
  tab to download via the normal pipeline (with a `--referer`/browser UA). The finder stays
  open until the user hits Stop. A **Chrome extension** (`siphon-extension/`) does the same
  scan in the browser; its "Send to Siphon" button copies the link and opens the quick window
  via the `siphon://` deep link.
- **Queue & history** card grid: live progress (percent / speed / size / ETA), open file,
  reveal in folder, copy link, retry, remove. Persisted to `history.json`.
- **System tray** (close = minimise to tray) and a **global quick-paste window**
  (`Alt+S`) with per-download "Open file / Open folder when done" toggles.
- First launch **auto-downloads** `yt-dlp` + a static `ffmpeg`/`ffprobe` into the app-data
  `bin/` (gyan.dev zip on Windows, evermeet on macOS, system PATH on Linux).

## Structure

```
siphon/
├── renderer/                 # Frontend (no framework)
│   ├── index.html            # main window
│   ├── styles.css            # design system (single source of CSS tokens)
│   ├── app.js                # UI logic + Tauri bridge (invoke / event listen)
│   ├── quicksave.html/js     # the Alt+S quick-download popup (own inline <style>)
│   ├── logo-mark.png         # square mark (titlebar / README)
│   └── text-mark.png         # "Siphon" wordmark used in the titlebars
└── src-tauri/
    ├── src/lib.rs            # app setup, settings, tray, shortcut, window controls,
    │                         #   open/reveal commands, DWM border colour
    ├── src/engine.rs         # yt-dlp + ffmpeg bootstrap, status, no-window Command
    ├── src/downloader.rs     # job model, probe, yt-dlp + direct download, progress,
    │                         #   ffmpeg conversion, history persistence (Job.referer)
    ├── src/sniffer.rs        # Advanced Search: finder WebView + native WebView2
    │                         #   WebResourceRequested sniffer + injected overlay
    ├── src/main.rs           # entry point
    ├── tauri.conf.json       # frameless window, CSP, bundle icons
    ├── capabilities/default.json   # main + quicksave windows
    ├── capabilities/sniffer.json   # remote-IPC grant for the "sniffer" finder window
    └── Cargo.toml

siphon-extension/             # standalone MV3 Chrome extension (sibling of siphon/)
    ├── manifest.json         # webRequest + storage + tabs; <all_urls>
    ├── background.js         # service worker: collect .m3u8/.mpd per tab → storage.session
    ├── popup.{html,js,css}   # list streams with Copy / "Send to Siphon" (Alt+S) buttons
    └── icon-128.png          # copied from renderer/logo-mark.png
```

## Architecture notes

- **Commands** are registered in `lib.rs`'s `invoke_handler!`. Download commands live in
  `downloader.rs` (`probe_url`, `start_download`, `get_jobs`, `cancel_download`,
  `remove_job`, `clear_finished`, `list_downloads`); engine/settings/window/file commands
  live in `lib.rs`. Advanced Search commands (`open_sniffer`, `report_stream`,
  `close_sniffer`) live in `sniffer.rs`; the finder window emits `sniffer://found` /
  `sniffer://status` events that the main window's Advanced Search tab lists for download.
- **Stream delivery is native, not IPC.** A remote page's CSP blocks Tauri's
  `http://ipc.localhost` IPC (so an injected script's `invoke` is refused on sites like
  Headspace — Copy still works since it's pure JS). So `sniffer.rs` reads manifest URLs at
  the **WebView2 layer**: after building the finder window it calls `window.with_webview` and
  registers an `add_WebResourceRequested` handler (Windows-only, `#[cfg(windows)]`,
  `webview2-com` + `windows` target-deps pinned to the lock) that emits `sniffer://found`
  for any `.m3u8`/`.mpd`. The injected `SNIFF_JS` overlay is now just feedback + a Copy
  fallback. The `sniffer` capability's `remote.urls` grant + `report_stream` remain but are
  effectively unused on CSP-strict sites — keep the native path as the source of truth.
- **`siphon://` deep link** (for the browser extension): `tauri-plugin-deep-link` registers
  the scheme (runtime `register("siphon")` in setup, plus `tauri.conf.json` →
  `plugins.deep-link.desktop.schemes` for installed builds). `tauri-plugin-single-instance`
  is registered **first** in `run()` so a `siphon://` launch routes to the running instance
  (forwards argv → emits `open-quicksave`) instead of starting a duplicate. Both the
  single-instance callback and `deep_link().on_open_url` just `emit("open-quicksave")`, which
  the existing Alt+S listener turns into the quick window (auto-fills from the clipboard).
- **State** (all managed as `Arc<…>` so worker threads can share):
  `EngineState` (binaries + dirs), `JobStore` (jobs + history), `SettingsState` (data.json).
- **Progress is event-driven**, not polled: each job emits `download://update`; the engine
  emits `engine://status`; the quick window emits `jobs-changed` on close. The renderer
  listens via `window.__TAURI__.event.listen`.
- **Downloads + network run on `std::thread`** (not the async runtime) because they use
  blocking `reqwest` / `std::process::Command`. `probe_url` uses `spawn_blocking`.
- **Download location** is resolved per job from settings: single folder
  (`Downloads/Siphon` by default, or a custom picked folder) or **by type**
  (`Videos/Siphon` for MP4, `Music/Siphon` for audio). Stored on the job as `target_dir`.
- **Files dir / clipboard**: opening files and revealing in folder go through
  `tauri-plugin-opener` (`open_path` / `reveal_item_in_dir`) — do NOT use raw `cmd start`
  / `explorer /select` (they were buggy). Clipboard read/write uses the
  `clipboard-manager` plugin (`window.__TAURI__.clipboardManager`), never
  `navigator.clipboard` (that triggers a WebView permission prompt).

## Branding / design (locked)

- **Always dark, always Siphon green** (`#4FB286`). The light/dark toggle and accent-colour
  picker were intentionally removed — keep it that way. Settings only expose surface
  opacity + glass blur, download location, shortcut display, a **Start on login** toggle,
  and engine status.
- Fonts: **Geist** + **Geist Mono** (mono for all numbers). No Inter.
- Glassmorphism with inner-border refraction; staggered card reveals; shimmer progress.
- The Windows 11 window border is tinted green via `DwmSetWindowAttribute`
  (`apply_accent_border` in `lib.rs`, best-effort / no-op elsewhere).

## Gotchas

- **Tauri `compression` feature is disabled** in `Cargo.toml` (`default-features = false`,
  re-adding every default except `compression`). Its `brotli` dep fails to compile on this
  toolchain. `zip` is also trimmed to `["deflate"]`. Don't re-enable them.
- `[hidden] { display: none !important }` in `styles.css` is required — several components
  use `display:flex`, which otherwise overrides the `hidden` attribute (caused a phantom
  skeleton + empty red error bar on load).
- **WebView2 ≠ Edge for flex**: a flex item with both `flex:1` and a fixed `width` keeps the
  width in WebView2. The quick window's segmented control overrides `.seg { width:auto }`
  via `#qsPresetGroup` to span full width. Watch for this pattern.
- **Never build a window inside a synchronous `#[tauri::command]`**: sync commands run on
  the main thread, and `WebviewWindowBuilder::build()` blocks until the main event loop
  creates the webview — so building inline deadlocks (the IPC promise never resolves; the UI
  button that triggered it stays disabled forever). `open_sniffer` builds on a worker thread
  and returns immediately. The quicksave window is safe because it's built from an
  `app.listen` callback, not a command. Same applies to `app.run_on_main_thread` from a
  command.
- The built exe **embeds the renderer at build time** (`cargo build`/`tauri build`), so
  frontend changes need a rebuild to show in the binary; `cargo tauri dev` serves live.
- Closing hides to tray, so old instances linger and the always-on-top quick window can
  overlap a fresh launch — kill all `siphon` processes before testing.

## Run / build

```bash
cd siphon
npm run dev      # cargo tauri dev (live renderer)
npm run build    # cargo tauri build → installer in src-tauri/target/release/bundle/
```

Downloads go to `Downloads/Siphon` by default (or the configured location). App data
(settings `data.json`, `history.json`, downloaded `bin/`) lives in the OS app-data dir
under `com.siphon.desktop`.
