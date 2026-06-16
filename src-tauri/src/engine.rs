// ─── Engine ─────────────────────────────────────────────────────
// Bootstraps and resolves the external binaries Siphon depends on:
//   • yt-dlp           – downloaded from the official GitHub release
//   • ffmpeg / ffprobe – a static build, or whatever is already on PATH
//
// Mirrors `ensureYtDlpBinary` / `resolveFfmpegState` from the original
// download-music server.js, but in native Rust.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const YT_DLP_WIN: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
const YT_DLP_MAC: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos";
const YT_DLP_LINUX: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp";

const FFMPEG_WIN_ZIP: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";
const FFMPEG_MAC_ZIP: &str = "https://evermeet.cx/ffmpeg/getrelease/ffmpeg/zip";
const FFPROBE_MAC_ZIP: &str = "https://evermeet.cx/ffmpeg/getrelease/ffprobe/zip";

// Published checksums, fetched and used to verify each download before it is
// written to disk or executed (defence against tampered/corrupted downloads — CWE-494).
const YT_DLP_SUMS: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS";
const FFMPEG_WIN_SHA: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip.sha256";

#[derive(Debug, Clone, Serialize)]
pub struct BinaryStatus {
    pub status: String, // "pending" | "downloading" | "ready" | "error"
    pub version: Option<String>,
    pub error: Option<String>,
}

impl BinaryStatus {
    fn pending() -> Self {
        BinaryStatus { status: "pending".into(), version: None, error: None }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineSnapshot {
    pub yt_dlp: BinaryStatus,
    pub ffmpeg: BinaryStatus,
    pub ready: bool,
}

pub struct EngineState {
    pub bin_dir: PathBuf,
    pub downloads_dir: PathBuf,
    pub yt_dlp: Mutex<BinaryStatus>,
    pub ffmpeg: Mutex<BinaryStatus>,
    pub preparing: Mutex<bool>,
}

impl EngineState {
    pub fn new(bin_dir: PathBuf, downloads_dir: PathBuf) -> Self {
        EngineState {
            bin_dir,
            downloads_dir,
            yt_dlp: Mutex::new(BinaryStatus::pending()),
            ffmpeg: Mutex::new(BinaryStatus::pending()),
            preparing: Mutex::new(false),
        }
    }

    pub fn snapshot(&self) -> EngineSnapshot {
        let yt = self.yt_dlp.lock().unwrap().clone();
        let ff = self.ffmpeg.lock().unwrap().clone();
        let ready = yt.status == "ready" && ff.status == "ready";
        EngineSnapshot { yt_dlp: yt, ffmpeg: ff, ready }
    }

    // ── Path helpers ──
    pub fn yt_dlp_path(&self) -> PathBuf {
        self.bin_dir.join(yt_dlp_filename())
    }

    fn bundled_ffmpeg(&self) -> PathBuf {
        self.bin_dir.join(exe_name("ffmpeg"))
    }

    fn bundled_ffprobe(&self) -> PathBuf {
        self.bin_dir.join(exe_name("ffprobe"))
    }

    /// Executable path to invoke ffmpeg with (bundled build preferred, else PATH).
    pub fn ffmpeg_exe(&self) -> String {
        if self.bundled_ffmpeg().exists() {
            self.bundled_ffmpeg().to_string_lossy().to_string()
        } else {
            "ffmpeg".to_string()
        }
    }

    /// Directory passed to yt-dlp via `--ffmpeg-location` (Some only when bundled).
    pub fn ffmpeg_location(&self) -> Option<PathBuf> {
        if self.bundled_ffmpeg().exists() {
            Some(self.bin_dir.clone())
        } else {
            None
        }
    }
}

// ── Cross-platform helpers ──

/// Build a Command that never flashes a console window on Windows.
pub fn new_command<S: AsRef<std::ffi::OsStr>>(program: S) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

fn yt_dlp_filename() -> String {
    exe_name("yt-dlp")
}

fn run_version(exe: &str, args: &[&str]) -> Option<String> {
    let out = new_command(exe).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().next().map(|l| l.trim().to_string())
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent("Siphon/1.0")
        .timeout(Duration::from_secs(900))
        .build()
        .map_err(|e| e.to_string())
}

fn download_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = http_client()?
        .get(url)
        .send()
        .map_err(|e| format!("Download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}", resp.status().as_u16()));
    }
    resp.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string())
}

fn download_text(url: &str) -> Result<String, String> {
    let bytes = download_bytes(url)?;
    String::from_utf8(bytes).map_err(|_| "Checksum file was not valid UTF-8.".to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// Verify `bytes` against an expected (case-insensitive) hex SHA-256. On mismatch
/// this returns an error so the caller can abort — an unverified binary is never run.
fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<(), String> {
    let expected = expected_hex.trim().to_lowercase();
    if expected.len() != 64 {
        return Err("Checksum was missing or malformed; download discarded.".into());
    }
    let got = sha256_hex(bytes);
    if got == expected {
        Ok(())
    } else {
        Err(format!(
            "Integrity check failed (expected {expected}, got {got}); download discarded."
        ))
    }
}

/// Find the expected SHA-256 for `asset_name` inside a `SHA2-256SUMS`-style file
/// (`<hex>  <name>` per line; the name may be prefixed with `*`).
fn expected_sha_for(sums_text: &str, asset_name: &str) -> Option<String> {
    for line in sums_text.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(hash), Some(name)) = (parts.next(), parts.next()) {
            if name.trim_start_matches('*') == asset_name {
                return Some(hash.to_lowercase());
            }
        }
    }
    None
}

/// (download URL, asset name as listed in SHA2-256SUMS) for the current platform.
fn yt_dlp_source() -> (&'static str, &'static str) {
    if cfg!(windows) {
        (YT_DLP_WIN, "yt-dlp.exe")
    } else if cfg!(target_os = "macos") {
        (YT_DLP_MAC, "yt-dlp_macos")
    } else {
        (YT_DLP_LINUX, "yt-dlp")
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

// ── Status emit ──

fn emit(app: &AppHandle, engine: &EngineState) {
    let _ = app.emit("engine://status", engine.snapshot());
}

fn set_yt(engine: &EngineState, status: BinaryStatus) {
    *engine.yt_dlp.lock().unwrap() = status;
}

fn set_ff(engine: &EngineState, status: BinaryStatus) {
    *engine.ffmpeg.lock().unwrap() = status;
}

// ── Public entry point ──

/// Ensure both binaries exist. Runs on a dedicated OS thread (blocking I/O).
pub fn prepare(engine: Arc<EngineState>, app: AppHandle) {
    {
        let mut preparing = engine.preparing.lock().unwrap();
        if *preparing {
            return;
        }
        *preparing = true;
    }

    let _ = std::fs::create_dir_all(&engine.bin_dir);
    let _ = std::fs::create_dir_all(&engine.downloads_dir);

    ensure_yt_dlp(&engine, &app);
    ensure_ffmpeg(&engine, &app);

    *engine.preparing.lock().unwrap() = false;
    emit(&app, &engine);
}

fn ensure_yt_dlp(engine: &EngineState, app: &AppHandle) {
    let path = engine.yt_dlp_path();

    if !path.exists() {
        set_yt(engine, BinaryStatus { status: "downloading".into(), version: None, error: None });
        emit(app, engine);

        let (url, asset) = yt_dlp_source();

        // Download to memory, verify against yt-dlp's published SHA2-256SUMS, and
        // only then write to disk — an unverified binary is never persisted or run.
        let bytes = match download_bytes(url) {
            Ok(bytes) => bytes,
            Err(err) => {
                set_yt(engine, BinaryStatus { status: "error".into(), version: None, error: Some(err) });
                emit(app, engine);
                return;
            }
        };

        let expected = download_text(YT_DLP_SUMS)
            .ok()
            .and_then(|sums| expected_sha_for(&sums, asset));
        let verified = match expected {
            Some(expected) => verify_sha256(&bytes, &expected),
            None => Err("Could not verify yt-dlp: checksums were unavailable.".into()),
        };
        if let Err(err) = verified {
            set_yt(engine, BinaryStatus { status: "error".into(), version: None, error: Some(err) });
            emit(app, engine);
            return;
        }

        if let Err(err) = std::fs::write(&path, &bytes) {
            set_yt(engine, BinaryStatus { status: "error".into(), version: None, error: Some(err.to_string()) });
            emit(app, engine);
            return;
        }
        make_executable(&path);
    }

    match run_version(&path.to_string_lossy(), &["--version"]) {
        Some(v) => set_yt(engine, BinaryStatus { status: "ready".into(), version: Some(v), error: None }),
        None => set_yt(
            engine,
            BinaryStatus {
                status: "error".into(),
                version: None,
                error: Some("yt-dlp downloaded but failed to run.".into()),
            },
        ),
    }
    emit(app, engine);
}

fn ensure_ffmpeg(engine: &EngineState, app: &AppHandle) {
    // Already bundled?
    if engine.bundled_ffmpeg().exists() && engine.bundled_ffprobe().exists() {
        finish_ffmpeg(engine, app);
        return;
    }

    // Already on PATH? (mirrors the original resolveFfmpegState behaviour)
    if run_version("ffmpeg", &["-version"]).is_some() && run_version("ffprobe", &["-version"]).is_some() {
        finish_ffmpeg(engine, app);
        return;
    }

    set_ff(engine, BinaryStatus { status: "downloading".into(), version: None, error: None });
    emit(app, engine);

    let result = if cfg!(windows) {
        fetch_ffmpeg_windows(engine)
    } else if cfg!(target_os = "macos") {
        fetch_ffmpeg_macos(engine)
    } else {
        Err("ffmpeg/ffprobe were not found. Please install them with your package manager (e.g. `sudo apt install ffmpeg`).".into())
    };

    match result {
        Ok(()) => finish_ffmpeg(engine, app),
        Err(err) => {
            set_ff(engine, BinaryStatus { status: "error".into(), version: None, error: Some(err) });
            emit(app, engine);
        }
    }
}

fn finish_ffmpeg(engine: &EngineState, app: &AppHandle) {
    let exe = engine.ffmpeg_exe();
    match run_version(&exe, &["-version"]) {
        Some(v) => set_ff(engine, BinaryStatus { status: "ready".into(), version: Some(v), error: None }),
        None => set_ff(
            engine,
            BinaryStatus {
                status: "error".into(),
                version: None,
                error: Some("ffmpeg is present but failed to run.".into()),
            },
        ),
    }
    emit(app, engine);
}

fn fetch_ffmpeg_windows(engine: &EngineState) -> Result<(), String> {
    let zip_bytes = download_bytes(FFMPEG_WIN_ZIP)?;

    // Verify the archive against gyan.dev's published SHA-256 before extracting.
    let expected = download_text(FFMPEG_WIN_SHA)?;
    let expected = expected.split_whitespace().next().unwrap_or("");
    verify_sha256(&zip_bytes, expected)?;

    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| e.to_string())?;

    let mut got_ffmpeg = false;
    let mut got_ffprobe = false;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = file.name().to_lowercase();
        let dest = if name.ends_with("bin/ffmpeg.exe") {
            got_ffmpeg = true;
            engine.bundled_ffmpeg()
        } else if name.ends_with("bin/ffprobe.exe") {
            got_ffprobe = true;
            engine.bundled_ffprobe()
        } else {
            continue;
        };

        let mut out = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
        std::io::copy(&mut file, &mut out).map_err(|e| e.to_string())?;
    }

    if got_ffmpeg && got_ffprobe {
        Ok(())
    } else {
        Err("Could not extract ffmpeg/ffprobe from the downloaded archive.".into())
    }
}

fn fetch_ffmpeg_macos(engine: &EngineState) -> Result<(), String> {
    // evermeet.cx serves the binary over HTTPS but without a simple checksum
    // sidecar, so these downloads rely on TLS for integrity. (yt-dlp and the
    // Windows ffmpeg build are additionally verified against published hashes.)
    extract_single_binary(FFMPEG_MAC_ZIP, &engine.bundled_ffmpeg())?;
    extract_single_binary(FFPROBE_MAC_ZIP, &engine.bundled_ffprobe())?;
    make_executable(&engine.bundled_ffmpeg());
    make_executable(&engine.bundled_ffprobe());
    Ok(())
}

/// Extracts the first regular file from a zip (the evermeet builds ship a single binary).
fn extract_single_binary(url: &str, dest: &Path) -> Result<(), String> {
    let zip_bytes = download_bytes(url)?;
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| e.to_string())?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        if file.is_file() {
            let mut out = std::fs::File::create(dest).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut out).map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    Err("Downloaded archive contained no file.".into())
}
