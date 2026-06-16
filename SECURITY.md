# Security Policy

## Reporting a vulnerability

If you discover a security vulnerability in Siphon, please report it **privately** rather
than opening a public issue. Use GitHub's **["Report a vulnerability"](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)**
flow (Security tab → "Report a vulnerability"), which creates a private security advisory.

Please include:

- A description of the issue and its potential impact.
- Steps to reproduce (a minimal proof of concept if possible).
- The affected version / commit.

You can expect an initial acknowledgement within a reasonable time frame. Please give a
reasonable window for a fix before any public disclosure.

## Scope and design notes

Siphon is a local, offline-first desktop application. It has **no backend, no user
accounts, no database, and collects no telemetry**. Notable security-relevant design
choices:

- External tools (`yt-dlp`, `ffmpeg`) are invoked via `std::process::Command` with
  arguments passed as a separate argument vector — **never** through a shell — so user
  input (URLs, filenames) cannot be interpreted as shell commands.
- Submitted URLs are validated before use; output filenames are sanitized of path and
  shell-significant characters.
- `yt-dlp` and a static `ffmpeg`/`ffprobe` build are downloaded on first launch from their
  official upstream sources into the app-data folder.

If you are reviewing the threat model, the most relevant surfaces are the URL/manifest
handling in `src-tauri/src/downloader.rs` and `src-tauri/src/sniffer.rs`, and the Tauri
capability grants in `src-tauri/capabilities/`.
