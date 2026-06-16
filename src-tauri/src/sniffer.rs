// ─── Stream sniffer ─────────────────────────────────────────────
// Powers the "Advanced Search" tab. Some pages don't work with yt-dlp
// because the real media is an HLS/DASH manifest (.m3u8 / .mpd) that the
// page's JavaScript only requests after login + pressing play — it is not
// in the page HTML, so a server-side fetch can't find it.
//
// We load the page in a hidden WebView and inject a small script that hooks
// the page's network activity (PerformanceObserver + fetch + XHR) and reports
// any manifest URL back via the `report_stream` command. The main window
// listens for the `sniffer://found` event and lists the streams to download.
//
// Silent-first: the finder window starts hidden. If nothing is detected
// within REVEAL_AFTER_SECS, we reveal it so the user can sign in / press play.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

const SNIFFER_LABEL: &str = "sniffer";
const REVEAL_AFTER_SECS: u64 = 10;

/// Shared across `open_sniffer`, its reveal timer, and `report_stream`.
#[derive(Default)]
pub struct SnifferState {
    /// Flips true on the first detection; suppresses the interactive reveal.
    found_any: AtomicBool,
    /// Bumped per `open_sniffer` so a stale reveal timer no-ops.
    generation: AtomicUsize,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FoundStream {
    page_url: String,
    stream_url: String,
    kind: String,
}

/// Open (or reopen) the hidden finder window pointed at `url`.
///
/// The window is built on a worker thread, not here: `WebviewWindowBuilder::build()`
/// blocks until the main event loop creates the webview, and this command runs ON the
/// main thread — building inline would deadlock (the loop can't run while it waits for
/// us to return). So we validate synchronously, then hand the build to a thread and
/// return immediately. Build/reveal status is reported via the `sniffer://status` event.
#[tauri::command]
pub fn open_sniffer(
    app: AppHandle,
    state: State<'_, Arc<SnifferState>>,
    url: String,
) -> Result<(), String> {
    let normalized = crate::downloader::parse_url(&url)?;
    let target: tauri::Url = normalized
        .parse()
        .map_err(|_| "That URL is not valid.".to_string())?;

    state.found_any.store(false, Ordering::SeqCst);
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;

    let state = state.inner().clone();
    std::thread::spawn(move || {
        // One finder at a time — close any previous one and wait for it to go away
        // before reusing the label (off the main thread, so the loop can process it).
        if let Some(window) = app.get_webview_window(SNIFFER_LABEL) {
            let _ = window.close();
            for _ in 0..100 {
                if app.get_webview_window(SNIFFER_LABEL).is_none() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        let build = || {
            WebviewWindowBuilder::new(&app, SNIFFER_LABEL, WebviewUrl::External(target.clone()))
                .title("Siphon · Stream Finder")
                .inner_size(1000.0, 720.0)
                .min_inner_size(640.0, 480.0)
                .visible(false)
                .initialization_script(SNIFF_JS)
        };
        let builder = if let Some(icon) = app.default_window_icon().cloned() {
            match build().icon(icon) {
                Ok(b) => b,
                Err(_) => build(),
            }
        } else {
            build()
        };
        let window = match builder.build() {
            Ok(w) => w,
            Err(e) => {
                let _ = app.emit("sniffer://status", format!("error: Could not open the finder window: {e}"));
                return;
            }
        };

        // Read manifest URLs at the native WebView2 layer. The remote page's CSP
        // blocks Tauri's `ipc.localhost` IPC, so the injected script can't call back
        // — but WebResourceRequested sees every request below that layer. This is
        // what populates the Advanced Search tab live (via `sniffer://found`).
        install_network_sniffer(&app, &state, target.as_str(), &window);

        // Silent-first: reveal the window for manual interaction if, after a grace
        // period, no stream has been detected (auth-gated / play-to-load pages).
        std::thread::sleep(Duration::from_secs(REVEAL_AFTER_SECS));
        if state.generation.load(Ordering::SeqCst) != generation
            || state.found_any.load(Ordering::SeqCst)
        {
            return;
        }
        if let Some(window) = app.get_webview_window(SNIFFER_LABEL) {
            let _ = window.show();
            let _ = window.set_focus();
            let _ = app.emit("sniffer://status", "interactive");
        }
    });

    Ok(())
}

/// Called from the finder window's injected script for every manifest it sees.
#[tauri::command]
pub fn report_stream(
    app: AppHandle,
    state: State<'_, Arc<SnifferState>>,
    page_url: String,
    stream_url: String,
    kind: String,
) {
    state.found_any.store(true, Ordering::SeqCst);
    let _ = app.emit(
        "sniffer://found",
        FoundStream {
            page_url,
            stream_url,
            kind,
        },
    );
}

/// Close the finder window if it is open.
#[tauri::command]
pub fn close_sniffer(app: AppHandle) {
    if let Some(window) = app.get_webview_window(SNIFFER_LABEL) {
        let _ = window.close();
    }
}

/// Clear the finder browser's stored data — cookies, cache and site storage — so no
/// signed-in sessions persist. Closes the finder first, then clears the shared WebView2
/// profile via the main window (the main window itself never logs into anything, so this
/// only affects pages opened in the finder). Completion is reported via the
/// `sniffer://cleared` event with payload "ok" or "error: …".
#[tauri::command]
pub fn clear_finder_data(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(SNIFFER_LABEL) {
        let _ = window.close();
    }
    #[cfg(windows)]
    {
        clear_profile_data_windows(&app)
    }
    #[cfg(not(windows))]
    {
        let _ = &app;
        Err("Clearing finder data isn't supported on this platform yet.".into())
    }
}

/// Clears the shared WebView2 profile (cookies + cache + DOM storage) asynchronously.
#[cfg(windows)]
fn clear_profile_data_windows(app: &AppHandle) -> Result<(), String> {
    use webview2_com::ClearBrowsingDataCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2Profile2, ICoreWebView2_13};
    use windows::core::Interface;

    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window is not available.".to_string())?;
    let app = app.clone();

    main.with_webview(move |pw| unsafe {
        let fail = |e: String| {
            let _ = app.emit("sniffer://cleared", format!("error: {e}"));
        };
        let core = match pw.controller().CoreWebView2() {
            Ok(c) => c,
            Err(e) => return fail(e.to_string()),
        };
        let core13: ICoreWebView2_13 = match core.cast() {
            Ok(c) => c,
            Err(e) => return fail(e.to_string()),
        };
        let profile = match core13.Profile() {
            Ok(p) => p,
            Err(e) => return fail(e.to_string()),
        };
        let profile2: ICoreWebView2Profile2 = match profile.cast() {
            Ok(p) => p,
            Err(e) => return fail(e.to_string()),
        };
        let done = app.clone();
        let handler = ClearBrowsingDataCompletedHandler::create(Box::new(move |hr| {
            let payload = if hr.is_ok() { "ok".to_string() } else { format!("error: {hr:?}") };
            let _ = done.emit("sniffer://cleared", payload);
            Ok(())
        }));
        if let Err(e) = profile2.ClearBrowsingDataAll(&handler) {
            fail(e.to_string());
        }
    })
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// True if a URL looks like an HLS/DASH manifest we can hand to yt-dlp.
fn is_manifest(uri: &str) -> bool {
    let u = uri.to_lowercase();
    u.contains(".m3u8") || u.contains(".mpd") || u.contains(".f4m") || u.contains("format=m3u8")
}

/// Hook the finder window's WebView2 at the native layer and emit `sniffer://found`
/// to the main window for every manifest request we see. This is the reliable
/// delivery path: the page's CSP blocks Tauri's IPC, but WebResourceRequested runs
/// below the page so it isn't affected. Windows-only; a no-op elsewhere.
#[cfg(windows)]
fn install_network_sniffer(
    app: &AppHandle,
    state: &Arc<SnifferState>,
    page_url: &str,
    window: &tauri::WebviewWindow,
) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2WebResourceRequestedEventArgs,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
    };
    use webview2_com::{take_pwstr, WebResourceRequestedEventHandler};
    use windows::core::{w, PWSTR};

    let app = app.clone();
    let state = state.clone();
    let page_url = page_url.to_string();

    let _ = window.with_webview(move |pw| unsafe {
        let core = match pw.controller().CoreWebView2() {
            Ok(c) => c,
            Err(_) => return,
        };
        if core
            .AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)
            .is_err()
        {
            return;
        }

        // `seen` is owned by this FnMut handler — fresh dedup per finder window.
        let mut seen: HashSet<String> = HashSet::new();
        let handler = WebResourceRequestedEventHandler::create(Box::new(
            move |_wv: Option<ICoreWebView2>,
                  args: Option<ICoreWebView2WebResourceRequestedEventArgs>| {
                if let Some(args) = args {
                    if let Ok(request) = args.Request() {
                        {
                            // `Uri` is an out-param getter; `take_pwstr` frees the buffer.
                            let mut uri_ptr = PWSTR::null();
                            if request.Uri(&mut uri_ptr).is_ok() {
                                let uri = take_pwstr(uri_ptr);
                                if is_manifest(&uri) && seen.insert(uri.clone()) {
                                    state.found_any.store(true, Ordering::SeqCst);
                                    let kind = if uri.to_lowercase().contains(".mpd") {
                                        "dash"
                                    } else {
                                        "hls"
                                    };
                                    let _ = app.emit(
                                        "sniffer://found",
                                        FoundStream {
                                            page_url: page_url.clone(),
                                            stream_url: uri,
                                            kind: kind.to_string(),
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
                Ok(())
            },
        ));
        let mut token: i64 = 0;
        let _ = core.add_WebResourceRequested(&handler, &mut token);
    });
}

#[cfg(not(windows))]
fn install_network_sniffer(
    _app: &AppHandle,
    _state: &Arc<SnifferState>,
    _page_url: &str,
    _window: &tauri::WebviewWindow,
) {
}

/// Injected into the finder window before the page's own scripts. It:
///   1. hooks the page's network activity (PerformanceObserver + fetch + XHR) to
///      detect HLS/DASH manifest URLs, and
///   2. draws a small in-page panel listing what it found, each with a Copy-link
///      button (a manual fallback that works even when the page's CSP blocks IPC).
/// The real delivery into Siphon happens natively via `install_network_sniffer`
/// (WebResourceRequested) — this panel is feedback + a copy affordance.
const SNIFF_JS: &str = r#"
(function () {
  if (window.__SIPHON_SNIFF__) return;
  window.__SIPHON_SNIFF__ = true;

  var seen = {};
  var count = 0;
  var RX = /(\.m3u8|\.mpd|\.f4m)(\?|$)|[?&]format=m3u8|\/manifest(\/|\?|$)/i;

  var panel, listEl, statusEl;
  function S(el, css) { el.setAttribute("style", css); return el; }
  function root() { return document.body || document.documentElement; }

  function ensurePanel() {
    if (panel || !root()) return;
    panel = S(document.createElement("div"),
      "position:fixed;top:12px;right:12px;z-index:2147483647;width:380px;max-height:74vh;overflow:auto;" +
      "font-family:system-ui,Segoe UI,Arial,sans-serif;font-size:12px;background:#0f1411;color:#e8efe9;" +
      "border:1px solid #2f6b53;border-radius:12px;box-shadow:0 16px 40px rgba(0,0,0,.55);padding:12px 14px");
    var h = S(document.createElement("div"), "font-weight:700;font-size:13px;color:#4FB286;margin-bottom:6px");
    h.textContent = "Siphon — Stream Finder";
    statusEl = S(document.createElement("div"), "color:#9fb3a8;line-height:1.45;margin-bottom:8px");
    listEl = document.createElement("div");
    panel.appendChild(h); panel.appendChild(statusEl); panel.appendChild(listEl);
    root().appendChild(panel);
    refreshStatus();
  }
  function refreshStatus() {
    if (!statusEl) return;
    if (!count) {
      statusEl.innerHTML = "Active. Press <b>Play</b> on the page to load the stream.";
    } else {
      statusEl.innerHTML = count + " stream" + (count > 1 ? "s" : "") +
        " found — added to Siphon's <b>Advanced Search</b> to download. (Or Copy below.)";
    }
  }
  function mkBtn(label, onClick) {
    var b = S(document.createElement("button"),
      "cursor:pointer;border:1px solid #2f6b53;border-radius:8px;padding:6px 10px;font-size:11px;" +
      "font-weight:600;background:#16201b;color:#cfe7db");
    b.textContent = label;
    b.addEventListener("click", onClick);
    return b;
  }
  function copyText(text, btn) {
    try {
      var ta = S(document.createElement("textarea"), "position:fixed;top:0;left:0;opacity:0;pointer-events:none");
      ta.value = text;
      root().appendChild(ta);
      ta.focus(); ta.select();
      document.execCommand("copy");
      ta.remove();
      var prev = btn.textContent; btn.textContent = "Copied";
      setTimeout(function () { btn.textContent = prev; }, 1200);
    } catch (e) {}
  }
  function addRow(url) {
    ensurePanel();
    if (!listEl) return;
    var row = S(document.createElement("div"),
      "border-top:1px solid #1e2a24;padding:8px 0;display:flex;flex-direction:column;gap:6px");
    var u = S(document.createElement("div"),
      "word-break:break-all;font-family:ui-monospace,Consolas,monospace;font-size:11px;color:#bcd2c6");
    u.textContent = url;
    var bar = S(document.createElement("div"), "display:flex;gap:6px;flex-wrap:wrap");
    bar.appendChild(mkBtn("Copy link", function (e) { copyText(url, e.currentTarget); }));
    row.appendChild(u); row.appendChild(bar);
    listEl.appendChild(row);
  }

  function report(raw) {
    try {
      if (!raw || typeof raw !== "string") return;
      var url;
      try { url = new URL(raw, location.href).href; } catch (e) { url = raw; }
      if (!RX.test(url)) return;
      if (seen[url]) return;
      seen[url] = true;
      count++;
      addRow(url);
      refreshStatus();
    } catch (e) {}
  }

  try {
    new PerformanceObserver(function (list) {
      list.getEntries().forEach(function (e) { report(e.name); });
    }).observe({ type: "resource", buffered: true });
  } catch (e) {}

  try {
    var origFetch = window.fetch;
    if (origFetch) {
      window.fetch = function (input) {
        try { report(typeof input === "string" ? input : (input && input.url)); } catch (e) {}
        return origFetch.apply(this, arguments);
      };
    }
  } catch (e) {}

  try {
    var origOpen = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function (method, url) {
      try { report(url); } catch (e) {}
      return origOpen.apply(this, arguments);
    };
  } catch (e) {}

  if (document.body) ensurePanel();
  else document.addEventListener("DOMContentLoaded", ensurePanel);
})();
"#;
