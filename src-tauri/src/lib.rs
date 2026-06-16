// ─── Siphon ─────────────────────────────────────────────────────
// Tauri entry point: app state, appearance settings, system tray,
// global quick-paste shortcut, window controls, and command wiring.
// Download/convert logic lives in `downloader.rs`; binary bootstrap
// in `engine.rs`.

mod downloader;
mod engine;
mod sniffer;

use downloader::JobStore;
use engine::EngineState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Listener, Manager, State, WebviewUrl, WebviewWindowBuilder};

// ─── Time helpers (shared with downloader) ──────────────────────

pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub fn now_iso() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let total_secs = duration.as_secs();

    let secs_per_day: u64 = 86_400;
    let mut remaining_days = (total_secs / secs_per_day) as i64;
    let time_secs = total_secs % secs_per_day;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;

    let mut year: i64 = 1970;
    loop {
        let days_in_year = if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
            366
        } else {
            365
        };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let is_leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_months = [
        31,
        if is_leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month: usize = 0;
    for (i, &dim) in days_in_months.iter().enumerate() {
        if remaining_days < dim as i64 {
            month = i;
            break;
        }
        remaining_days -= dim as i64;
    }
    let day = remaining_days + 1;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month + 1,
        day,
        hours,
        minutes,
        seconds
    )
}

// ─── Settings ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppData {
    #[serde(default = "default_theme")]
    theme: String,
    #[serde(default = "default_accent")]
    accent_color: String,
    #[serde(default = "default_surface_opacity")]
    surface_opacity: f32,
    #[serde(default = "default_glass_blur")]
    glass_blur: f32,
    #[serde(default = "default_shortcut")]
    shortcut: String,
    #[serde(default = "default_download_mode")]
    download_mode: String, // "single" | "byType"
    #[serde(default)]
    download_dir: String, // empty => <Downloads>/Siphon
}

fn default_theme() -> String {
    "dark".into()
}
fn default_accent() -> String {
    "#4FB286".into()
}
fn default_surface_opacity() -> f32 {
    0.82
}
fn default_glass_blur() -> f32 {
    14.0
}
fn default_shortcut() -> String {
    "Alt+S".into()
}
fn default_download_mode() -> String {
    "single".into()
}

impl Default for AppData {
    fn default() -> Self {
        AppData {
            theme: default_theme(),
            accent_color: default_accent(),
            surface_opacity: default_surface_opacity(),
            glass_blur: default_glass_blur(),
            shortcut: default_shortcut(),
            download_mode: default_download_mode(),
            download_dir: String::new(),
        }
    }
}

fn default_base_dir(custom: &str) -> PathBuf {
    if custom.trim().is_empty() {
        dirs::download_dir()
            .map(|d| d.join("Siphon"))
            .unwrap_or_else(|| PathBuf::from("Siphon"))
    } else {
        PathBuf::from(custom)
    }
}

fn type_dir(preset: &str, fallback: PathBuf) -> PathBuf {
    let base = if preset == "mp4" {
        dirs::video_dir()
    } else {
        dirs::audio_dir()
    };
    base.map(|d| d.join("Siphon")).unwrap_or(fallback)
}

fn normalize_accent_color(value: &str) -> String {
    let trimmed = value.trim();
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        format!("#{}", hex.to_uppercase())
    } else {
        default_accent()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    theme: String,
    accent_color: String,
    surface_opacity: f32,
    glass_blur: f32,
    shortcut: String,
    download_mode: String,
    download_dir: String,
    video_dir: String,
    audio_dir: String,
}

pub struct SettingsState {
    data: Mutex<AppData>,
    path: PathBuf,
}

impl SettingsState {
    fn load(path: PathBuf) -> Self {
        let data = if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|content| serde_json::from_str(&content).ok())
                .unwrap_or_default()
        } else {
            AppData::default()
        };
        SettingsState {
            data: Mutex::new(data),
            path,
        }
    }

    fn save(&self) {
        let data = self.data.lock().unwrap();
        if let Ok(json) = serde_json::to_string_pretty(&*data) {
            let _ = fs::write(&self.path, json);
        }
    }

    fn snapshot(&self) -> Settings {
        let (theme, accent, opacity, blur, shortcut, mode, dir) = {
            let d = self.data.lock().unwrap();
            (
                d.theme.clone(),
                d.accent_color.clone(),
                d.surface_opacity,
                d.glass_blur,
                d.shortcut.clone(),
                d.download_mode.clone(),
                d.download_dir.clone(),
            )
        };
        let base = default_base_dir(&dir);
        Settings {
            theme,
            accent_color: accent,
            surface_opacity: opacity,
            glass_blur: blur,
            shortcut,
            download_mode: mode,
            download_dir: base.to_string_lossy().to_string(),
            video_dir: type_dir("mp4", base.clone()).to_string_lossy().to_string(),
            audio_dir: type_dir("mp3", base).to_string_lossy().to_string(),
        }
    }

    /// The folder a download with `preset` should be written to.
    pub fn target_dir(&self, preset: &str) -> PathBuf {
        let (mode, dir) = {
            let d = self.data.lock().unwrap();
            (d.download_mode.clone(), d.download_dir.clone())
        };
        let base = default_base_dir(&dir);
        if mode == "byType" {
            type_dir(preset, base)
        } else {
            base
        }
    }
}

// ─── Settings commands ──────────────────────────────────────────

#[tauri::command]
fn get_settings(settings: State<'_, Arc<SettingsState>>) -> Settings {
    settings.snapshot()
}

#[tauri::command]
fn set_theme(settings: State<'_, Arc<SettingsState>>, theme: String) -> String {
    settings.data.lock().unwrap().theme = theme.clone();
    settings.save();
    theme
}

#[tauri::command]
fn set_appearance(
    settings: State<'_, Arc<SettingsState>>,
    accent_color: String,
    surface_opacity: f32,
    glass_blur: f32,
) -> Settings {
    {
        let mut data = settings.data.lock().unwrap();
        data.accent_color = normalize_accent_color(&accent_color);
        data.surface_opacity = surface_opacity.clamp(0.4, 1.0);
        data.glass_blur = glass_blur.clamp(0.0, 30.0);
    }
    settings.save();
    settings.snapshot()
}

#[tauri::command]
fn set_download_mode(settings: State<'_, Arc<SettingsState>>, mode: String) -> Settings {
    {
        let mut data = settings.data.lock().unwrap();
        data.download_mode = if mode == "byType" {
            "byType".into()
        } else {
            "single".into()
        };
    }
    settings.save();
    settings.snapshot()
}

#[tauri::command]
fn pick_download_dir(app: AppHandle, settings: State<'_, Arc<SettingsState>>) -> Settings {
    use tauri_plugin_dialog::DialogExt;
    if let Some(folder) = app.dialog().file().blocking_pick_folder() {
        if let Ok(path) = folder.into_path() {
            {
                let mut data = settings.data.lock().unwrap();
                data.download_dir = path.to_string_lossy().to_string();
                data.download_mode = "single".into();
            }
            settings.save();
        }
    }
    settings.snapshot()
}

#[tauri::command]
fn set_shortcut(
    settings: State<'_, Arc<SettingsState>>,
    app: AppHandle,
    shortcut: String,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let _ = app.global_shortcut().unregister_all();

    let app_handle = app.clone();
    let parsed: tauri_plugin_global_shortcut::Shortcut =
        shortcut.parse().map_err(|e| format!("{:?}", e))?;
    app.global_shortcut()
        .on_shortcut(parsed, move |_app, _shortcut, event| {
            if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                let _ = app_handle.emit("open-quicksave", ());
            }
        })
        .map_err(|e| e.to_string())?;

    settings.data.lock().unwrap().shortcut = shortcut;
    settings.save();
    Ok(())
}

// ─── Startup (autostart) commands ───────────────────────────────
// The OS (Windows registry / macOS LaunchAgent / Linux .desktop) is the source
// of truth, so we don't mirror this into data.json — we just read/write the
// plugin's state. A boot launch passes `--autostart` (see plugin init in `run`)
// which starts the app hidden in the tray.

#[tauri::command]
fn get_autostart(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| e.to_string())?;
    } else {
        manager.disable().map_err(|e| e.to_string())?;
    }
    Ok(manager.is_enabled().unwrap_or(enabled))
}

// ─── Engine commands ────────────────────────────────────────────

#[tauri::command]
fn get_engine_status(engine: State<'_, Arc<EngineState>>) -> engine::EngineSnapshot {
    engine.snapshot()
}

#[tauri::command]
fn prepare_engine(app: AppHandle, engine: State<'_, Arc<EngineState>>) {
    let engine = engine.inner().clone();
    std::thread::spawn(move || engine::prepare(engine, app));
}

#[tauri::command]
fn get_downloads_dir(engine: State<'_, Arc<EngineState>>) -> String {
    engine.downloads_dir.to_string_lossy().to_string()
}

// ─── File commands ──────────────────────────────────────────────

#[tauri::command]
fn open_file(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal_in_folder(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_downloads_dir(app: AppHandle, settings: State<'_, Arc<SettingsState>>) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let dir = default_base_dir(&settings.data.lock().unwrap().download_dir.clone());
    let _ = fs::create_dir_all(&dir);
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

// ─── Window controls ────────────────────────────────────────────

#[tauri::command]
fn window_minimize(app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.minimize();
    }
}

#[tauri::command]
fn window_maximize(app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_maximized().unwrap_or(false) {
            let _ = window.unmaximize();
        } else {
            let _ = window.maximize();
        }
    }
}

#[tauri::command]
fn window_close(app: AppHandle) {
    // Hide to tray instead of quitting so downloads keep running.
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

#[tauri::command]
fn app_quit(app: AppHandle) {
    app.exit(0);
}

// ─── Quick-paste window ─────────────────────────────────────────

#[tauri::command]
fn close_quicksave(app: AppHandle) {
    let _ = app.emit("jobs-changed", ());
    if let Some(window) = app.get_webview_window("quicksave") {
        let _ = window.close();
    }
}

fn open_quicksave_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("quicksave") {
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let build = || {
        WebviewWindowBuilder::new(app, "quicksave", WebviewUrl::App("quicksave.html".into()))
            .title("Quick Download")
            .inner_size(480.0, 352.0)
            .resizable(false)
            .decorations(false)
            .skip_taskbar(true)
            .always_on_top(true)
            .center()
    };

    let builder = if let Some(icon) = app.default_window_icon().cloned() {
        match build().icon(icon) {
            Ok(b) => b,
            Err(_) => build(),
        }
    } else {
        build()
    };

    if let Ok(window) = builder.build() {
        apply_accent_border(&window);
    }
}

/// Colour the window's thin Windows 11 border with the Siphon accent green.
/// No-op on other platforms or pre-Windows-11 (the DWM call simply fails).
fn apply_accent_border(window: &tauri::WebviewWindow) {
    #[cfg(windows)]
    {
        #[link(name = "dwmapi")]
        extern "system" {
            fn DwmSetWindowAttribute(
                hwnd: isize,
                attr: u32,
                pv: *const core::ffi::c_void,
                cb: u32,
            ) -> i32;
        }
        const DWMWA_BORDER_COLOR: u32 = 34;
        if let Ok(hwnd) = window.hwnd() {
            // COLORREF is 0x00BBGGRR; Siphon green #4FB286 -> R=4F G=B2 B=86.
            let color: u32 = 0x0086_B24F;
            unsafe {
                DwmSetWindowAttribute(
                    hwnd.0 as isize,
                    DWMWA_BORDER_COLOR,
                    &color as *const u32 as *const core::ffi::c_void,
                    4,
                );
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = window;
    }
}

// ─── App setup ──────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // single-instance must be registered FIRST. When the browser opens a
        // `siphon://` link and Siphon is already running, the OS spawns a second
        // process; this routes its argv to the running instance and exits, so we
        // pop the quick window in the existing app (no duplicate instance).
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            if argv.iter().any(|a| a.starts_with("siphon://")) {
                let _ = app.emit("open-quicksave", ());
            }
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir().expect("missing app data dir");
            let _ = fs::create_dir_all(&app_data_dir);

            let bin_dir = app_data_dir.join("bin");
            let downloads_dir = dirs::download_dir()
                .map(|d| d.join("Siphon"))
                .unwrap_or_else(|| app_data_dir.join("downloads"));
            let _ = fs::create_dir_all(&bin_dir);
            let _ = fs::create_dir_all(&downloads_dir);

            // State
            let engine = Arc::new(EngineState::new(bin_dir, downloads_dir));
            app.manage(engine.clone());

            let store = Arc::new(JobStore::new(app_data_dir.join("history.json")));
            app.manage(store);

            app.manage(Arc::new(sniffer::SnifferState::default()));

            let settings = Arc::new(SettingsState::load(app_data_dir.join("data.json")));
            // Migrate the old default shortcut to the new one.
            {
                let mut data = settings.data.lock().unwrap();
                if data.shortcut == "CommandOrControl+Shift+S" {
                    data.shortcut = default_shortcut();
                }
            }
            settings.save();
            let shortcut_str = settings.data.lock().unwrap().shortcut.clone();
            app.manage(settings);

            if let Some(window) = app.get_webview_window("main") {
                if let Some(icon) = app.default_window_icon().cloned() {
                    let _ = window.set_icon(icon);
                }
                apply_accent_border(&window);
            }

            // System tray
            let show_item = MenuItem::with_id(app, "show", "Show Siphon", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .tooltip("Siphon")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            // Global quick-paste shortcut
            use tauri_plugin_global_shortcut::GlobalShortcutExt;
            let shortcut_handle = app.handle().clone();
            if let Ok(shortcut) = shortcut_str.parse::<tauri_plugin_global_shortcut::Shortcut>() {
                let _ = app.global_shortcut().on_shortcut(
                    shortcut,
                    move |_app, _shortcut, event| {
                        if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                            let _ = shortcut_handle.emit("open-quicksave", ());
                        }
                    },
                );
            }

            let quicksave_handle = app.handle().clone();
            app.listen("open-quicksave", move |_event| {
                open_quicksave_window(&quicksave_handle);
            });

            // `siphon://…` deep link (from the browser extension's "Send to Siphon").
            // Register the scheme so it works in dev too, and on open just pop the
            // quick window — it auto-fills from the clipboard the extension set.
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let _ = app.deep_link().register("siphon");
                let deep_link_handle = app.handle().clone();
                app.deep_link().on_open_url(move |_event| {
                    let _ = deep_link_handle.emit("open-quicksave", ());
                });
            }

            // Kick off binary bootstrap immediately.
            let prepare_engine = engine.clone();
            let prepare_handle = app.handle().clone();
            std::thread::spawn(move || engine::prepare(prepare_engine, prepare_handle));

            // Launched on boot with --autostart: start hidden in the tray.
            let args: Vec<String> = std::env::args().collect();
            if args.iter().any(|a| a == "--autostart") {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // engine
            get_engine_status,
            prepare_engine,
            get_downloads_dir,
            // downloads
            downloader::probe_url,
            downloader::start_download,
            downloader::get_jobs,
            downloader::cancel_download,
            downloader::remove_job,
            downloader::clear_finished,
            downloader::list_downloads,
            // advanced search (stream sniffer)
            sniffer::open_sniffer,
            sniffer::report_stream,
            sniffer::close_sniffer,
            sniffer::clear_finder_data,
            // files
            open_file,
            reveal_in_folder,
            open_downloads_dir,
            // settings
            get_settings,
            set_theme,
            set_appearance,
            set_shortcut,
            set_download_mode,
            pick_download_dir,
            get_autostart,
            set_autostart,
            // quicksave + window
            close_quicksave,
            window_minimize,
            window_maximize,
            window_close,
            app_quit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Siphon");
}
