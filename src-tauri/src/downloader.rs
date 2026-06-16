// ─── Downloader ─────────────────────────────────────────────────
// A faithful Rust port of download-music's server.js: URL validation,
// metadata probing, direct-media downloads, yt-dlp downloads with live
// progress, ffmpeg conversion, output resolution and error normalization.
//
// Progress is pushed to the webview via the `download://update` event
// (replacing the original's 1-second polling loop).

use crate::engine::{new_command, EngineState};
use crate::{now_iso, now_ms};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};

const DIRECT_MEDIA_EXTENSIONS: [&str; 8] =
    ["mp4", "mp3", "wav", "m4a", "webm", "ogg", "mov", "avi"];
const IDLE_TIMEOUT_MS: u128 = 60_000;
const CANCELED_SENTINEL: &str = "__SIPHON_CANCELED__";

// ─── Data models ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub id: Option<String>,
    pub title: String,
    pub thumbnail: Option<String>,
    pub duration: Option<f64>,
    pub duration_label: String,
    pub uploader: String,
    pub extractor: String,
    pub webpage_url: Option<String>,
    pub description: String,
    pub direct: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub url: String,
    pub preset: String,
    pub status: String, // queued | running | complete | error | canceled
    pub title: String,
    #[serde(default)]
    pub thumbnail: Option<String>,
    /// Coarse stage within a running job: "downloading" | "converting" | "merging".
    /// Drives the card's stage label and the indeterminate progress bar. None when
    /// not running.
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub percent: Option<String>,
    #[serde(default)]
    pub total_size: Option<String>,
    #[serde(default)]
    pub speed: Option<String>,
    #[serde(default)]
    pub eta: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub output_name: Option<String>,
    #[serde(default)]
    pub output_path: Option<String>,
    #[serde(default)]
    pub target_dir: String,
    /// Sent to yt-dlp as `--referer` for streams discovered via Advanced Search,
    /// so CDNs that check the Referer header don't reject the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referer: Option<String>,
    #[serde(default)]
    pub auto_open: bool,
    #[serde(default)]
    pub auto_reveal: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub path: String,
    pub modified_ms: u128,
}

// ─── Job store ──────────────────────────────────────────────────

pub struct JobStore {
    pub jobs: Mutex<HashMap<String, Job>>,
    pub order: Mutex<Vec<String>>, // newest first
    children: Mutex<HashMap<String, Arc<Mutex<Option<Child>>>>>,
    cancel: Mutex<HashMap<String, Arc<AtomicBool>>>,
    history_path: PathBuf,
}

impl JobStore {
    pub fn new(history_path: PathBuf) -> Self {
        let mut jobs = HashMap::new();
        let mut order = Vec::new();

        if let Ok(content) = std::fs::read_to_string(&history_path) {
            if let Ok(list) = serde_json::from_str::<Vec<Job>>(&content) {
                for job in list {
                    // Only completed/failed jobs survive a restart.
                    if matches!(job.status.as_str(), "complete" | "error" | "canceled") {
                        order.push(job.id.clone());
                        jobs.insert(job.id.clone(), job);
                    }
                }
            }
        }

        JobStore {
            jobs: Mutex::new(jobs),
            order: Mutex::new(order),
            children: Mutex::new(HashMap::new()),
            cancel: Mutex::new(HashMap::new()),
            history_path,
        }
    }

    pub fn ordered(&self) -> Vec<Job> {
        let jobs = self.jobs.lock().unwrap();
        let order = self.order.lock().unwrap();
        order.iter().filter_map(|id| jobs.get(id).cloned()).collect()
    }

    fn insert(&self, job: Job) {
        let id = job.id.clone();
        self.jobs.lock().unwrap().insert(id.clone(), job);
        self.order.lock().unwrap().insert(0, id);
    }

    fn mutate<F: FnOnce(&mut Job)>(&self, app: &AppHandle, id: &str, f: F) {
        let snapshot = {
            let mut jobs = self.jobs.lock().unwrap();
            if let Some(job) = jobs.get_mut(id) {
                f(job);
                job.updated_at = now_iso();
                Some(job.clone())
            } else {
                None
            }
        };
        if let Some(job) = snapshot {
            let _ = app.emit("download://update", &job);
        }
    }

    pub fn save(&self) {
        let list = self.ordered();
        if let Ok(json) = serde_json::to_string_pretty(&list) {
            let _ = std::fs::write(&self.history_path, json);
        }
    }
}

// ─── URL helpers (mirror of server.js) ──────────────────────────

pub(crate) fn parse_url(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("Paste a media URL first.".into());
    }
    match reqwest::Url::parse(trimmed) {
        Ok(url) => Ok(url.to_string()),
        Err(_) => Err("That URL is not valid.".into()),
    }
}

pub fn unsupported_url_message(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.trim_start_matches("www.").to_lowercase();
    let path = parsed.path().to_lowercase();

    let is_yt = host == "youtube.com" || host == "m.youtube.com";
    if is_yt && path == "/results" {
        return Some("Please paste a direct YouTube video link, not a search page.".into());
    }
    if is_yt
        && (path.starts_with("/@")
            || path.starts_with("/channel/")
            || path.starts_with("/c/")
            || path.starts_with("/user/"))
    {
        return Some("Please paste a direct YouTube video link, not a channel page.".into());
    }
    None
}

fn last_segment(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    parsed
        .path_segments()
        .and_then(|segments| segments.filter(|s| !s.is_empty()).last())
        .map(|s| s.to_string())
}

fn url_extension(url: &str) -> Option<String> {
    let seg = last_segment(url)?;
    Path::new(&seg)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
}

fn is_direct_media_url(url: &str) -> bool {
    match url_extension(url) {
        Some(ext) => DIRECT_MEDIA_EXTENSIONS.contains(&ext.as_str()),
        None => false,
    }
}

fn sanitize_file_name(value: &str) -> String {
    let replaced: String = value
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 0x20 {
                ' '
            } else {
                c
            }
        })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    let clipped: String = trimmed.chars().take(180).collect();
    if clipped.is_empty() {
        "download".to_string()
    } else {
        clipped
    }
}

fn infer_title_from_url(url: &str) -> String {
    if let Some(seg) = last_segment(url) {
        let decoded = seg.replace("%20", " ");
        let stem = Path::new(&decoded)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&decoded);
        let spaced = stem.replace(['-', '_'], " ");
        return sanitize_file_name(&spaced);
    }
    "download".to_string()
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

fn fmt_eta(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn format_duration(seconds: Option<f64>) -> String {
    let seconds = match seconds {
        Some(s) if s.is_finite() => s,
        _ => return "Unknown".to_string(),
    };
    let total = seconds.max(0.0).round() as i64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let secs = total % 60;
    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{}:{:02}", minutes, secs)
    }
}

fn normalize_error(message: &str) -> String {
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = normalized.to_lowercase();
    if lower.contains("drm") {
        "This source is DRM-protected, so it can't be downloaded.".to_string()
    } else if lower.contains("not a bot") || lower.contains("cookies-from-browser") {
        "This source is blocking the request without browser cookies. Try enabling browser cookies in the settings to download this file.".to_string()
    } else if lower.contains("this video is unavailable") {
        "This video is currently unavailable.".to_string()
    } else if normalized.is_empty() {
        "Download failed.".to_string()
    } else {
        normalized
    }
}

fn clean_field(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("na") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Read `reader`, invoking `on_line` for every segment terminated by `\n` OR `\r`.
/// yt-dlp prints live progress using carriage returns; `BufRead::lines()` only
/// breaks on `\n`, so those updates pool up and surface all at once at the end —
/// the visible "bar jumps 0% → 100%" bug. Splitting on both delimiters lets each
/// progress tick through. Pass a buffered reader so this isn't a syscall per byte.
fn read_crlf_lines<R: Read>(mut reader: R, mut on_line: impl FnMut(&str)) {
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                if byte[0] == b'\n' || byte[0] == b'\r' {
                    if !buf.is_empty() {
                        on_line(&String::from_utf8_lossy(&buf));
                        buf.clear();
                    }
                } else {
                    buf.push(byte[0]);
                }
            }
            Err(_) => break,
        }
    }
    if !buf.is_empty() {
        on_line(&String::from_utf8_lossy(&buf));
    }
}

// ─── Probe ──────────────────────────────────────────────────────

fn serialize_info(value: &serde_json::Value) -> MediaInfo {
    let thumbnail = value["thumbnail"].as_str().map(String::from).or_else(|| {
        value["thumbnails"]
            .as_array()
            .and_then(|arr| arr.last())
            .and_then(|t| t["url"].as_str())
            .map(String::from)
    });
    let duration = value["duration"].as_f64();
    let uploader = value["uploader"]
        .as_str()
        .or_else(|| value["channel"].as_str())
        .or_else(|| value["extractor_key"].as_str())
        .unwrap_or("Unknown source")
        .to_string();
    let extractor = value["extractor_key"]
        .as_str()
        .or_else(|| value["extractor"].as_str())
        .unwrap_or("Web")
        .to_string();
    let description = value["description"]
        .as_str()
        .map(|d| d.chars().take(220).collect::<String>())
        .unwrap_or_default();

    MediaInfo {
        id: value["id"].as_str().map(String::from),
        title: value["title"].as_str().unwrap_or("Untitled media").to_string(),
        thumbnail,
        duration,
        duration_label: format_duration(duration),
        uploader,
        extractor,
        webpage_url: value["webpage_url"].as_str().map(String::from),
        description,
        direct: false,
    }
}

fn probe_direct(url: &str) -> MediaInfo {
    let host = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_else(|| "Direct source".to_string());
    MediaInfo {
        id: None,
        title: infer_title_from_url(url),
        thumbnail: None,
        duration: None,
        duration_label: "Direct file".to_string(),
        uploader: host,
        extractor: "Direct".to_string(),
        webpage_url: Some(url.to_string()),
        description: "Direct media file".to_string(),
        direct: true,
    }
}

fn probe(engine: &EngineState, raw_url: &str) -> Result<MediaInfo, String> {
    let url = parse_url(raw_url)?;
    if let Some(message) = unsupported_url_message(&url) {
        return Err(message);
    }
    if is_direct_media_url(&url) {
        return Ok(probe_direct(&url));
    }

    if engine.yt_dlp.lock().unwrap().status != "ready" {
        return Err("The download engine is still preparing. Please wait a moment and try again.".into());
    }

    let output = new_command(engine.yt_dlp_path())
        .args(["-J", "--no-warnings", "--no-playlist", &url])
        .output()
        .map_err(|e| format!("Failed to run yt-dlp: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(normalize_error(&stderr));
    }

    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Could not read media info.".to_string())?;
    Ok(serialize_info(&value))
}

// ─── Output resolution ──────────────────────────────────────────

fn list_files(dir: &Path) -> Vec<FileEntry> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".part") {
                continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let modified_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis())
                .unwrap_or(0);
            files.push(FileEntry {
                name,
                size: meta.len(),
                path: path.to_string_lossy().to_string(),
                modified_ms,
            });
        }
    }
    files.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    files
}

/// Most-recent modification time (ms since epoch) of any file under `dir`,
/// recursing into subdirectories. Used as a watchdog heartbeat: while ffmpeg is
/// converting/merging, yt-dlp emits no stdout but the output file keeps growing,
/// so a fresh mtime here means the job is still alive.
fn newest_mtime(dir: &Path) -> Option<u128> {
    let mut newest: Option<u128> = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let candidate = if path.is_dir() {
                newest_mtime(&path)
            } else {
                entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis())
            };
            if let Some(ms) = candidate {
                newest = Some(newest.map_or(ms, |cur| cur.max(ms)));
            }
        }
    }
    newest
}

/// True for yt-dlp intermediate streams like `video.f137.mp4`.
fn looks_like_intermediate(name: &str) -> bool {
    let lower = name.to_lowercase();
    let bytes = lower.as_bytes();
    let mut search_from = 0;
    while let Some(pos) = lower[search_from..].find(".f") {
        let digits_start = search_from + pos + 2;
        let mut cursor = digits_start;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor > digits_start && cursor < bytes.len() && bytes[cursor] == b'.' {
            return true;
        }
        search_from = digits_start;
        if search_from >= lower.len() {
            break;
        }
    }
    false
}

fn resolve_completed_output(
    dir: &Path,
    preset: &str,
    started_ms: u128,
    reported_path: Option<&str>,
) -> Result<String, String> {
    if let Some(path) = reported_path {
        if Path::new(path).exists() {
            return Ok(basename(path));
        }
    }

    let files = list_files(dir);
    let started_after = started_ms.saturating_sub(2000);
    let final_ext = format!(".{}", preset.to_lowercase());

    if let Some(file) = files.iter().find(|f| {
        f.modified_ms >= started_after
            && f.name.to_lowercase().ends_with(&final_ext)
            && !looks_like_intermediate(&f.name)
    }) {
        return Ok(file.name.clone());
    }

    if files.iter().any(|f| f.modified_ms >= started_after) {
        if preset == "mp4" {
            return Err("The video was downloaded, but it was not merged correctly. ffmpeg or ffprobe was not resolved correctly.".into());
        }
        return Err("The audio was downloaded, but it was not converted correctly. ffmpeg or ffprobe was not resolved correctly.".into());
    }

    Err("No final output file was found.".into())
}

/// Return `dir/file_name`, or `dir/{stem} (1).{ext}`, `(2)`, … if it already
/// exists — so a new download never overwrites a file with the same name.
fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let as_path = Path::new(file_name);
    let stem = as_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.to_string());
    let ext = as_path.extension().map(|e| e.to_string_lossy().to_string());
    let mut n: u32 = 1;
    loop {
        let next = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = dir.join(next);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

// ─── ffmpeg ─────────────────────────────────────────────────────

fn run_ffmpeg(engine: &EngineState, args: &[&str]) -> Result<(), String> {
    if engine.ffmpeg.lock().unwrap().status != "ready" {
        return Err("ffmpeg is not available yet. Please wait for the engine to finish preparing.".into());
    }
    let output = new_command(engine.ffmpeg_exe())
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(if stderr.trim().is_empty() {
            "ffmpeg conversion failed.".to_string()
        } else {
            stderr
        })
    }
}

// ─── Direct media download ──────────────────────────────────────

fn download_direct(
    app: &AppHandle,
    engine: &EngineState,
    store: &JobStore,
    id: &str,
    dir: &Path,
    url: &str,
    preset: &str,
    title: &str,
    canceled: Arc<AtomicBool>,
) -> Result<(), String> {
    let source_ext = url_extension(url)
        .map(|e| format!(".{e}"))
        .unwrap_or_else(|| ".bin".to_string());
    let chosen_title = if title.is_empty() {
        infer_title_from_url(url)
    } else {
        title.to_string()
    };
    let base = sanitize_file_name(&chosen_title);
    let temp_input = unique_path(dir, &format!("{base} [source]{source_ext}"));
    // Resolve a non-colliding final name up front so we never overwrite.
    let final_output = unique_path(dir, &format!("{base} [direct].{preset}"));

    store.mutate(app, id, |job| {
        job.status = "running".to_string();
        job.phase = Some("downloading".to_string());
    });

    let response = reqwest::blocking::Client::builder()
        .user_agent("Siphon/1.0")
        .build()
        .map_err(|e| e.to_string())?
        .get(url)
        .send()
        .map_err(|e| format!("Source request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Source returned {} {}.",
            response.status().as_u16(),
            response.status().canonical_reason().unwrap_or("error")
        ));
    }

    let total_bytes = response.content_length().unwrap_or(0);
    let mut reader = response;
    let mut file = std::fs::File::create(&temp_input).map_err(|e| e.to_string())?;
    let mut buffer = [0u8; 65536];
    let mut written: u64 = 0;
    let mut last_emit = Instant::now();
    let mut last_written: u64 = 0;

    loop {
        if canceled.load(Ordering::SeqCst) {
            drop(file);
            let _ = std::fs::remove_file(&temp_input);
            return Err(CANCELED_SENTINEL.to_string());
        }
        let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
        written += read as u64;

        if last_emit.elapsed() >= Duration::from_millis(120) {
            // Live transfer rate over the window since the last emit, so the card
            // shows a real "2.1 MiB/s" instead of a static "Receiving".
            let elapsed = last_emit.elapsed().as_secs_f64();
            last_emit = Instant::now();
            let delta = written.saturating_sub(last_written);
            last_written = written;
            let rate = if elapsed > 0.0 { delta as f64 / elapsed } else { 0.0 };
            let speed = if rate >= 1.0 {
                format!("{}/s", human_bytes(rate as u64))
            } else {
                "Receiving".to_string()
            };

            let percent = if total_bytes > 0 {
                Some(format!("{:.1}%", (written as f64 / total_bytes as f64) * 100.0))
            } else {
                None
            };
            // Known total → show it; unknown → show bytes received so far so the
            // card isn't stuck at a blank 0% for a Content-Length-less source.
            let size = if total_bytes > 0 {
                Some(human_bytes(total_bytes))
            } else {
                Some(human_bytes(written))
            };
            let eta = if total_bytes > 0 && rate >= 1.0 {
                let remaining = total_bytes.saturating_sub(written) as f64;
                Some(fmt_eta((remaining / rate) as u64))
            } else if total_bytes > 0 {
                Some("Calculating".to_string())
            } else {
                None
            };
            store.mutate(app, id, |job| {
                job.phase = Some("downloading".to_string());
                job.percent = percent.clone();
                job.total_size = size.clone();
                job.speed = Some(speed.clone());
                job.eta = eta.clone();
            });
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    let source_matches_preset = (preset == "mp4" && source_ext == ".mp4")
        || (preset == "mp3" && source_ext == ".mp3")
        || (preset == "wav" && source_ext == ".wav");

    if source_matches_preset {
        std::fs::rename(&temp_input, &final_output).map_err(|e| e.to_string())?;
    } else {
        store.mutate(app, id, |job| {
            job.phase = Some("converting".to_string());
            job.percent = None;
            job.speed = Some("Converting".to_string());
            job.eta = None;
        });

        let input = temp_input.to_string_lossy().to_string();
        let output = final_output.to_string_lossy().to_string();
        let args: Vec<&str> = match preset {
            "mp4" => vec!["-y", "-i", &input, "-c:v", "libx264", "-c:a", "aac", &output],
            "mp3" => vec!["-y", "-i", &input, "-vn", "-codec:a", "libmp3lame", "-q:a", "2", &output],
            _ => vec!["-y", "-i", &input, "-vn", &output],
        };
        run_ffmpeg(engine, &args)?;
        let _ = std::fs::remove_file(&temp_input);
    }

    let output_name = basename(&final_output.to_string_lossy());
    let output_path = final_output.to_string_lossy().to_string();
    store.mutate(app, id, |job| {
        job.output_name = Some(output_name.clone());
        job.output_path = Some(output_path.clone());
        job.status = "complete".to_string();
        job.phase = None;
        job.percent = Some("100.0%".to_string());
        job.speed = Some("Done".to_string());
        job.eta = Some("0s".to_string());
    });
    Ok(())
}

// ─── yt-dlp download ────────────────────────────────────────────

fn run_ytdlp(
    app: &AppHandle,
    engine: &EngineState,
    store: &JobStore,
    id: &str,
    dir: &Path,
    url: &str,
    preset: &str,
    referer: Option<&str>,
    started_ms: u128,
    canceled: Arc<AtomicBool>,
) -> Result<(), String> {
    // Download into a private temp dir so yt-dlp can't see (and overwrite or
    // skip) an existing same-named file. On success the finished file is moved
    // into `dir` under a non-colliding name; the temp dir is always cleaned up.
    let work_dir = dir.join(format!(".siphon-tmp-{id}"));
    let _ = std::fs::create_dir_all(&work_dir);
    let result = ytdlp_download(
        app, engine, store, id, &work_dir, dir, url, preset, referer, started_ms, canceled,
    );
    let _ = std::fs::remove_dir_all(&work_dir);
    result
}

#[allow(clippy::too_many_arguments)]
fn ytdlp_download(
    app: &AppHandle,
    engine: &EngineState,
    store: &JobStore,
    id: &str,
    work_dir: &Path,
    final_dir: &Path,
    url: &str,
    preset: &str,
    referer: Option<&str>,
    started_ms: u128,
    canceled: Arc<AtomicBool>,
) -> Result<(), String> {
    if engine.yt_dlp.lock().unwrap().status != "ready" {
        return Err("The download engine is still preparing. Please wait a moment and try again.".into());
    }

    let downloads = work_dir.to_string_lossy().to_string();
    let mut args: Vec<String> = vec![
        "--no-playlist",
        "--no-warnings",
        "--newline",
        "--socket-timeout",
        "15",
        "--extractor-retries",
        "1",
        "--retries",
        "2",
        "--windows-filenames",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    if let Some(location) = engine.ffmpeg_location() {
        args.push("--ffmpeg-location".to_string());
        args.push(location.to_string_lossy().to_string());
    }

    args.push("-P".to_string());
    args.push(downloads);
    args.push("-o".to_string());
    args.push("%(title).180B [%(id)s].%(ext)s".to_string());
    args.push("--print".to_string());
    args.push("after_move:filepath".to_string());
    args.push("--progress-template".to_string());
    args.push(
        "download:@@SIPHON@@%(progress._percent_str)s@@%(progress._speed_str)s@@%(progress._total_bytes_str)s@@%(progress._eta_str)s"
            .to_string(),
    );

    if preset == "mp4" {
        args.extend(
            ["-f", "bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/b", "--merge-output-format", "mp4"]
                .iter()
                .map(|s| s.to_string()),
        );
    } else {
        args.extend(
            ["-x", "--audio-format", preset, "--audio-quality", "0"]
                .iter()
                .map(|s| s.to_string()),
        );
    }

    // Streams discovered via Advanced Search often live behind a CDN that
    // checks the Referer (and sometimes rejects non-browser User-Agents).
    if let Some(referer) = referer {
        if !referer.is_empty() {
            args.push("--referer".to_string());
            args.push(referer.to_string());
            args.push("--user-agent".to_string());
            args.push(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
                    .to_string(),
            );
        }
    }

    args.push(url.to_string());

    store.mutate(app, id, |job| {
        job.status = "running".to_string();
        job.phase = Some("downloading".to_string());
    });

    let mut child = new_command(engine.yt_dlp_path())
        .args(&args)
        // yt-dlp is a frozen Python binary; without this its stdout is block-buffered
        // when piped, so progress lines pool up and arrive all at once at the end
        // (the bar appears to jump straight from 0% to 100%). Force it unbuffered.
        .env("PYTHONUNBUFFERED", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start yt-dlp: {e}"))?;

    let stdout = child.stdout.take().ok_or("Could not read yt-dlp output.")?;
    let stderr = child.stderr.take().ok_or("Could not read yt-dlp output.")?;
    let child_arc = Arc::new(Mutex::new(Some(child)));
    store
        .children
        .lock()
        .unwrap()
        .insert(id.to_string(), child_arc.clone());

    // Collect stderr for error reporting.
    let stderr_buf = Arc::new(Mutex::new(String::new()));
    {
        let stderr_buf = stderr_buf.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                let mut buf = stderr_buf.lock().unwrap();
                buf.push_str(&line);
                buf.push('\n');
            }
        });
    }

    // Idle watchdog. yt-dlp goes silent on stdout while ffmpeg post-processes
    // (extract-audio / merge), which on a 1 hr+ file can far exceed the idle
    // timeout — so a stdout-only watchdog kills healthy long conversions. We also
    // watch the work dir: ffmpeg writes the output progressively, so a fresh file
    // mtime counts as activity. Only kill when BOTH stdout and disk have been quiet.
    let last_activity = Arc::new(Mutex::new(Instant::now()));
    let timed_out = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    {
        let last_activity = last_activity.clone();
        let timed_out = timed_out.clone();
        let done = done.clone();
        let canceled = canceled.clone();
        let child_arc = child_arc.clone();
        let work_dir = work_dir.to_path_buf();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(2));
            if done.load(Ordering::SeqCst) {
                break;
            }
            if canceled.load(Ordering::SeqCst) {
                if let Some(child) = child_arc.lock().unwrap().as_mut() {
                    let _ = child.kill();
                }
                break;
            }
            let idle_ms = last_activity.lock().unwrap().elapsed().as_millis();
            if idle_ms <= IDLE_TIMEOUT_MS {
                continue;
            }
            // stdout has been quiet — is ffmpeg still writing the output file?
            let disk_active = newest_mtime(&work_dir)
                .is_some_and(|mtime| now_ms().saturating_sub(mtime) <= IDLE_TIMEOUT_MS);
            if disk_active {
                continue;
            }
            timed_out.store(true, Ordering::SeqCst);
            if let Some(child) = child_arc.lock().unwrap().as_mut() {
                let _ = child.kill();
            }
            break;
        });
    }

    let mut reported_path: Option<String> = None;
    read_crlf_lines(BufReader::new(stdout), |line| {
        *last_activity.lock().unwrap() = Instant::now();

        if let Some(rest) = line.strip_prefix("@@SIPHON@@") {
            let parts: Vec<&str> = rest.split("@@").collect();
            let percent = clean_field(parts.first().copied());
            let speed = clean_field(parts.get(1).copied());
            let size = clean_field(parts.get(2).copied());
            let eta = clean_field(parts.get(3).copied());
            store.mutate(app, id, |job| {
                // Don't let a late stray download tick revert a started conversion.
                if !matches!(job.phase.as_deref(), Some("converting") | Some("merging")) {
                    job.phase = Some("downloading".to_string());
                }
                job.percent = percent.clone();
                job.speed = speed.clone();
                job.total_size = size.clone();
                job.eta = eta.clone();
            });
            return;
        }

        // yt-dlp emits post-processing markers on stdout. The download bar can't
        // track ffmpeg, so switch the card to an indeterminate "Converting…" /
        // "Merging…" stage. These lines start with "[" and are never the
        // `after_move:filepath` print, so they must not be taken as the output path.
        let trimmed = line.trim();
        if trimmed.starts_with("[ExtractAudio]")
            || trimmed.starts_with("[VideoConvertor]")
            || trimmed.starts_with("[VideoRemuxer]")
        {
            store.mutate(app, id, |job| {
                job.phase = Some("converting".to_string());
                job.percent = None;
                job.eta = None;
                job.speed = None;
            });
        } else if trimmed.starts_with("[Merger]") {
            store.mutate(app, id, |job| {
                job.phase = Some("merging".to_string());
                job.percent = None;
                job.eta = None;
                job.speed = None;
            });
        } else if !trimmed.is_empty() && !trimmed.starts_with('[') {
            reported_path = Some(trimmed.to_string());
        }
    });

    done.store(true, Ordering::SeqCst);

    let exit_status = {
        let mut guard = child_arc.lock().unwrap();
        guard.take().map(|mut child| child.wait())
    };

    if canceled.load(Ordering::SeqCst) {
        return Err(CANCELED_SENTINEL.to_string());
    }
    if timed_out.load(Ordering::SeqCst) {
        return Err("The source stopped responding. Try a different URL or try again.".into());
    }

    let status = match exit_status {
        Some(Ok(status)) => status,
        _ => return Err("yt-dlp process error.".into()),
    };

    if !status.success() {
        let stderr = stderr_buf.lock().unwrap().clone();
        let message = if stderr.trim().is_empty() {
            format!("yt-dlp exited with code {}.", status.code().unwrap_or(-1))
        } else {
            stderr
        };
        return Err(normalize_error(&message));
    }

    let produced_name = resolve_completed_output(work_dir, preset, started_ms, reported_path.as_deref())?;

    // Move the finished file out of the temp dir into the real folder, picking a
    // non-colliding name so we never overwrite an existing download.
    let dest = unique_path(final_dir, &produced_name);
    std::fs::rename(work_dir.join(&produced_name), &dest)
        .map_err(|e| format!("Could not save the finished file: {e}"))?;
    let output_path = dest.to_string_lossy().to_string();
    let output_name = dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or(produced_name);

    store.mutate(app, id, |job| {
        job.output_name = Some(output_name.clone());
        job.output_path = Some(output_path.clone());
        job.status = "complete".to_string();
        job.phase = None;
        job.percent = Some("100.0%".to_string());
        job.eta = Some("0s".to_string());
    });

    Ok(())
}

// ─── Worker entry point ─────────────────────────────────────────

fn run_download(app: AppHandle, engine: Arc<EngineState>, store: Arc<JobStore>, id: String) {
    let (url, preset, title, target_dir, referer) = {
        let jobs = store.jobs.lock().unwrap();
        match jobs.get(&id) {
            Some(job) => (
                job.url.clone(),
                job.preset.clone(),
                job.title.clone(),
                job.target_dir.clone(),
                job.referer.clone(),
            ),
            None => return,
        }
    };

    let dir = if target_dir.trim().is_empty() {
        engine.downloads_dir.clone()
    } else {
        PathBuf::from(&target_dir)
    };
    let _ = std::fs::create_dir_all(&dir);

    let started_ms = now_ms();
    let canceled = Arc::new(AtomicBool::new(false));
    store
        .cancel
        .lock()
        .unwrap()
        .insert(id.clone(), canceled.clone());

    let result = if is_direct_media_url(&url) {
        download_direct(&app, &engine, &store, &id, &dir, &url, &preset, &title, canceled.clone())
    } else {
        run_ytdlp(
            &app,
            &engine,
            &store,
            &id,
            &dir,
            &url,
            &preset,
            referer.as_deref(),
            started_ms,
            canceled.clone(),
        )
    };

    store.cancel.lock().unwrap().remove(&id);
    store.children.lock().unwrap().remove(&id);

    match result {
        Ok(()) => {
            use tauri_plugin_opener::OpenerExt;
            let (auto_open, auto_reveal, output_path) = {
                let jobs = store.jobs.lock().unwrap();
                match jobs.get(&id) {
                    Some(job) => (job.auto_open, job.auto_reveal, job.output_path.clone()),
                    None => (false, false, None),
                }
            };
            if let Some(path) = output_path {
                if auto_open {
                    let _ = app.opener().open_path(path.clone(), None::<&str>);
                }
                if auto_reveal {
                    let _ = app.opener().reveal_item_in_dir(path);
                }
            }
        }
        Err(message) if message == CANCELED_SENTINEL || canceled.load(Ordering::SeqCst) => {
            store.mutate(&app, &id, |job| {
                job.status = "canceled".to_string();
                job.error = Some("Canceled.".to_string());
            });
        }
        Err(message) => {
            store.mutate(&app, &id, |job| {
                job.status = "error".to_string();
                job.error = Some(message.clone());
            });
        }
    }

    store.save();
}

// ─── Commands ───────────────────────────────────────────────────

#[tauri::command]
pub async fn probe_url(
    engine: State<'_, Arc<EngineState>>,
    url: String,
) -> Result<MediaInfo, String> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || probe(&engine, &url))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn start_download(
    app: AppHandle,
    engine: State<'_, Arc<EngineState>>,
    store: State<'_, Arc<JobStore>>,
    settings: State<'_, Arc<crate::SettingsState>>,
    url: String,
    preset: String,
    title: Option<String>,
    thumbnail: Option<String>,
    auto_open: Option<bool>,
    auto_reveal: Option<bool>,
    referer: Option<String>,
) -> Result<Job, String> {
    let url = parse_url(&url)?;
    if let Some(message) = unsupported_url_message(&url) {
        return Err(message);
    }
    if !["mp4", "mp3", "wav"].contains(&preset.as_str()) {
        return Err("Choose MP4, MP3, or WAV.".into());
    }

    let requested_title = title.unwrap_or_default();
    let title = if !requested_title.trim().is_empty() {
        requested_title.trim().to_string()
    } else if is_direct_media_url(&url) {
        infer_title_from_url(&url)
    } else {
        "Download".to_string()
    };

    let target = settings.target_dir(&preset);
    let _ = std::fs::create_dir_all(&target);

    let now = now_iso();
    let job = Job {
        id: uuid::Uuid::new_v4().to_string()[..12].to_string(),
        url,
        preset,
        status: "queued".to_string(),
        title,
        thumbnail,
        phase: None,
        percent: None,
        total_size: None,
        speed: None,
        eta: None,
        error: None,
        output_name: None,
        output_path: None,
        target_dir: target.to_string_lossy().to_string(),
        referer: referer.and_then(|r| {
            let r = r.trim().to_string();
            if r.is_empty() {
                None
            } else {
                Some(r)
            }
        }),
        auto_open: auto_open.unwrap_or(false),
        auto_reveal: auto_reveal.unwrap_or(false),
        created_at: now.clone(),
        updated_at: now,
    };

    store.insert(job.clone());

    let app_handle = app.clone();
    let engine = engine.inner().clone();
    let store_arc = store.inner().clone();
    let id = job.id.clone();
    std::thread::spawn(move || run_download(app_handle, engine, store_arc, id));

    Ok(job)
}

#[tauri::command]
pub fn get_jobs(store: State<'_, Arc<JobStore>>) -> Vec<Job> {
    store.ordered()
}

#[tauri::command]
pub fn cancel_download(store: State<'_, Arc<JobStore>>, id: String) {
    if let Some(flag) = store.cancel.lock().unwrap().get(&id) {
        flag.store(true, Ordering::SeqCst);
    }
    if let Some(child) = store.children.lock().unwrap().get(&id) {
        if let Some(process) = child.lock().unwrap().as_mut() {
            let _ = process.kill();
        }
    }
}

#[tauri::command]
pub fn remove_job(store: State<'_, Arc<JobStore>>, id: String) -> Vec<Job> {
    store.jobs.lock().unwrap().remove(&id);
    store.order.lock().unwrap().retain(|existing| existing != &id);
    store.save();
    store.ordered()
}

#[tauri::command]
pub fn clear_finished(store: State<'_, Arc<JobStore>>) -> Vec<Job> {
    let removable: Vec<String> = {
        let jobs = store.jobs.lock().unwrap();
        jobs.values()
            .filter(|job| matches!(job.status.as_str(), "complete" | "error" | "canceled"))
            .map(|job| job.id.clone())
            .collect()
    };
    {
        let mut jobs = store.jobs.lock().unwrap();
        let mut order = store.order.lock().unwrap();
        for id in &removable {
            jobs.remove(id);
        }
        order.retain(|id| !removable.contains(id));
    }
    store.save();
    store.ordered()
}

#[tauri::command]
pub fn list_downloads(engine: State<'_, Arc<EngineState>>) -> Vec<FileEntry> {
    list_files(&engine.downloads_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_path_avoids_overwrite() {
        let dir = std::env::temp_dir().join(format!("siphon-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // Free name is returned unchanged.
        let p0 = unique_path(&dir, "song.mp3");
        assert_eq!(p0, dir.join("song.mp3"));
        std::fs::write(&p0, b"x").unwrap();

        // Taken names get ` (1)`, ` (2)`, … before the extension.
        let p1 = unique_path(&dir, "song.mp3");
        assert_eq!(p1, dir.join("song (1).mp3"));
        std::fs::write(&p1, b"x").unwrap();
        assert_eq!(unique_path(&dir, "song.mp3"), dir.join("song (2).mp3"));

        // Names with no extension still work.
        std::fs::write(dir.join("README"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "README"), dir.join("README (1)"));

        // Multi-dot names keep the full stem.
        std::fs::write(dir.join("My.Video.mp4"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "My.Video.mp4"), dir.join("My.Video (1).mp4"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
