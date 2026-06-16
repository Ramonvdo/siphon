/* ═══════════════════════════════════════════════════════════════
   Siphon — application logic (Tauri v2)
   ═══════════════════════════════════════════════════════════════ */

// ─── Platform tag ───────────────────────────────────────────────
(function detectPlatform() {
  const ua = (navigator.userAgent || '').toLowerCase();
  let platform = 'windows';
  if (ua.includes('mac')) platform = 'macos';
  else if (ua.includes('linux')) platform = 'linux';
  document.documentElement.setAttribute('data-platform', platform);
})();

// ─── Tauri bridge ───────────────────────────────────────────────
async function invoke(cmd, args) {
  for (let i = 0; i < 30; i++) {
    if (window.__TAURI__ && window.__TAURI__.core) {
      return window.__TAURI__.core.invoke(cmd, args);
    }
    await new Promise((r) => setTimeout(r, 80));
  }
  console.error('Tauri API unavailable for command:', cmd);
  throw new Error('Tauri API unavailable.');
}

function convertFileSrc(path) {
  if (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.convertFileSrc) {
    return window.__TAURI__.core.convertFileSrc(path);
  }
  return path;
}

async function listen(event, handler) {
  for (let i = 0; i < 30; i++) {
    if (window.__TAURI__ && window.__TAURI__.event) {
      return window.__TAURI__.event.listen(event, handler);
    }
    await new Promise((r) => setTimeout(r, 80));
  }
}

// ─── State ──────────────────────────────────────────────────────
const state = {
  jobs: [],
  filter: 'all',
  preset: 'mp4',
  info: null,
  engine: null,
  settings: null,
  // Advanced Search (stream sniffer)
  view: 'library',
  advStreams: [],
  advPage: null,
};

const PRESETS = ['mp4', 'mp3', 'wav'];

// ─── DOM ────────────────────────────────────────────────────────
const $ = (s) => document.querySelector(s);
const $$ = (s) => Array.from(document.querySelectorAll(s));

const urlInput = $('#urlInput');
const urlField = urlInput.closest('.url-field');
const urlClear = $('#urlClear');
const presetGroup = $('#presetGroup');
const previewBtn = $('#previewBtn');
const downloadBtn = $('#downloadBtn');
const downloadForm = $('#downloadForm');
const intakeError = $('#intakeError');
const intakeErrorMsg = $('#intakeErrorMsg');
const intakeAdvBtn = $('#intakeAdvBtn');
const preview = $('#preview');
const previewSkeleton = $('#previewSkeleton');
const previewThumb = $('#previewThumb');
const previewTitle = $('#previewTitle');
const previewMeta = $('#previewMeta');
const previewDesc = $('#previewDesc');
const grid = $('#cardGrid');
const emptyState = $('#emptyState');
const queueCount = $('#queueCount');
const toastEl = $('#toast');
const toastMsg = $('#toastMessage');
const engineDot = $('#engineDot');
const engineStateEl = $('#engineState');
const engineDetail = $('#engineDetail');

// Advanced Search
const advUrl = $('#advUrl');
const advFind = $('#advFind');
const advClose = $('#advClose');
const advStatus = $('#advStatus');
const advResults = $('#advResults');
const advEmpty = $('#advEmpty');
const advResultsHead = $('#advResultsHead');
const advCount = $('#advCount');
const advClearBtn = $('#advClearBtn');
const advClearData = $('#advClearData');

// ─── Icons ──────────────────────────────────────────────────────
const I = {
  open: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 11l3 3 8-8"/><path d="M20 12v7a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h9"/></svg>',
  folder: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.7-.9l-.8-1.2A2 2 0 0 0 8 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>',
  link: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7"/><path d="M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7"/></svg>',
  trash: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>',
  x: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>',
  retry: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 2v6h6"/><path d="M3 13a9 9 0 1 0 3-7.7L3 8"/></svg>',
  search: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>',
};

const GLYPH = {
  video: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="m22 8-6 4 6 4V8Z"/><rect x="2" y="6" width="14" height="12" rx="2"/></svg>',
  audio: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>',
};

// ─── Helpers ────────────────────────────────────────────────────
function esc(value) {
  return (value ?? '').toString().replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

function errMessage(e) {
  if (typeof e === 'string') return e;
  if (e && e.message) return e.message;
  return 'Something went wrong.';
}

function clampPct(job) {
  if (job.status === 'complete') return 100;
  const n = parseFloat(job.percent);
  if (!Number.isFinite(n)) return 0;
  return Math.max(0, Math.min(100, n));
}

function sourceLabel(job) {
  if (job.status === 'complete' && job.outputName) return job.outputName;
  try {
    return new URL(job.url).hostname.replace(/^www\./, '');
  } catch {
    return job.url;
  }
}

function statusLabel(status) {
  return {
    queued: 'Queued', running: 'Downloading', complete: 'Completed',
    error: 'Failed', canceled: 'Canceled',
  }[status] || status;
}

// Stage within a running job, shown on the card so the user sees what it's doing.
function phaseLabel(job) {
  if (job.phase === 'converting') return `Converting to ${(job.preset || '').toUpperCase()}…`;
  if (job.phase === 'merging') return 'Merging…';
  return 'Downloading';
}

function cardStatusLabel(job) {
  return job.status === 'running' ? phaseLabel(job) : statusLabel(job.status);
}

// The bar can't track ffmpeg, and some sources report no size — show a moving band
// instead of a frozen 0%/number in those cases.
function isIndeterminate(job) {
  if (job.status !== 'running') return false;
  if (job.phase === 'converting' || job.phase === 'merging') return true;
  return !Number.isFinite(parseFloat(job.percent));
}

function matchesFilter(job, filter) {
  switch (filter) {
    case 'active': return job.status === 'queued' || job.status === 'running';
    case 'completed': return job.status === 'complete';
    case 'audio': return job.preset === 'mp3' || job.preset === 'wav';
    case 'video': return job.preset === 'mp4';
    default: return true;
  }
}

function fmtShortcut(s) {
  return (s || '')
    .replace('CommandOrControl', 'Ctrl')
    .replace('CmdOrCtrl', 'Ctrl')
    .replace('Control', 'Ctrl');
}

let toastTimer = null;
function toast(message, isError = false) {
  toastMsg.textContent = message;
  toastEl.classList.toggle('error', isError);
  toastEl.classList.add('visible');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toastEl.classList.remove('visible'), 2300);
}

async function copyText(text) {
  try {
    if (window.__TAURI__ && window.__TAURI__.clipboardManager) {
      await window.__TAURI__.clipboardManager.writeText(text);
    } else {
      await navigator.clipboard.writeText(text);
    }
    toast('Link copied');
  } catch {
    toast('Could not copy link', true);
  }
}

// ─── Intake error ───────────────────────────────────────────────
function showError(message) {
  intakeErrorMsg.textContent = message;
  // Offer "Try advanced search" only when there's a URL to hand off.
  intakeAdvBtn.hidden = urlInput.value.trim().length === 0;
  intakeError.hidden = false;
}
function hideError() {
  intakeError.hidden = true;
}

function clientUnsupported(value) {
  try {
    const url = new URL(value);
    const host = url.hostname.replace(/^www\./, '').toLowerCase();
    const path = url.pathname.toLowerCase();
    const yt = host === 'youtube.com' || host === 'm.youtube.com';
    if (yt && path === '/results') return 'Please paste a direct YouTube video link, not a search page.';
    if (yt && (path.startsWith('/@') || path.startsWith('/channel/') || path.startsWith('/c/') || path.startsWith('/user/'))) {
      return 'Please paste a direct YouTube video link, not a channel page.';
    }
  } catch {
    return null;
  }
  return null;
}

// ─── Preset segmented control ───────────────────────────────────
function setPreset(preset) {
  state.preset = preset;
  const idx = Math.max(0, PRESETS.indexOf(preset));
  presetGroup.style.setProperty('--seg-i', idx);
  $$('.seg').forEach((b) => b.classList.toggle('active', b.dataset.preset === preset));
}

// ─── Preview ────────────────────────────────────────────────────
function hidePreview() {
  preview.hidden = true;
}

function renderPreview(info) {
  if (info.thumbnail) {
    previewThumb.innerHTML = `<img src="${esc(info.thumbnail)}" alt="">`;
  } else {
    previewThumb.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.1-3.1a2 2 0 0 0-2.8 0L6 21"/></svg>';
  }
  previewTitle.textContent = info.title || 'Preview ready';
  const parts = [];
  if (info.uploader) parts.push(`<span>${esc(info.uploader)}</span>`);
  if (info.durationLabel) parts.push('<span class="dot"></span>', `<span>${esc(info.durationLabel)}</span>`);
  if (info.extractor) parts.push('<span class="dot"></span>', `<span class="src-tag">${esc(info.extractor)}</span>`);
  previewMeta.innerHTML = parts.join('');
  previewDesc.textContent = info.description || '';
  previewDesc.style.display = info.description ? '' : 'none';
  preview.hidden = false;
}

async function fetchPreview(url) {
  hideError();
  hidePreview();
  previewSkeleton.hidden = false;
  try {
    const info = await invoke('probe_url', { url });
    info._url = url;
    state.info = info;
    renderPreview(info);
    return info;
  } finally {
    previewSkeleton.hidden = true;
  }
}

async function doPreview() {
  const url = urlInput.value.trim();
  if (!url) return showError('Please paste a link.');
  const unsupported = clientUnsupported(url);
  if (unsupported) return showError(unsupported);

  previewBtn.disabled = true;
  try {
    await fetchPreview(url);
  } catch (e) {
    state.info = null;
    showError(errMessage(e));
  } finally {
    previewBtn.disabled = false;
  }
}

// ─── Downloads ──────────────────────────────────────────────────
async function startDownloadWith(url, preset, title, thumbnail) {
  try {
    const job = await invoke('start_download', {
      url,
      preset,
      title: title || null,
      thumbnail: thumbnail || null,
    });
    if (job) {
      upsertJob(job);
      toast('Download started');
    }
  } catch (e) {
    showError(errMessage(e));
  }
}

async function submitDownload(event) {
  event.preventDefault();
  const url = urlInput.value.trim();
  if (!url) return showError('Please paste a link.');
  const unsupported = clientUnsupported(url);
  if (unsupported) return showError(unsupported);

  hideError();

  // Always show a preview for the download (probe if we don't already have it).
  let info = state.info && state.info._url === url ? state.info : null;
  if (!info) {
    downloadBtn.disabled = true;
    try {
      info = await fetchPreview(url);
    } catch (e) {
      showError(errMessage(e));
      return;
    } finally {
      downloadBtn.disabled = !(state.engine && state.engine.ready);
    }
  }

  await startDownloadWith(url, state.preset, info && info.title, info && info.thumbnail);
}

// ─── Card rendering ─────────────────────────────────────────────
function metricsHTML(job) {
  if (job.status === 'complete') {
    return `<span class="m-pct">100%</span><span class="m-speed">${esc(job.totalSize || '')}</span><span class="m-eta">Done</span>`;
  }
  if (job.status === 'queued') {
    return '<span class="m-pct">Queued…</span><span class="m-speed"></span><span class="m-eta"></span>';
  }
  if (job.status === 'canceled') {
    return '<span class="m-pct">Canceled</span><span class="m-speed"></span><span class="m-eta"></span>';
  }
  // Conversion / merge: no trackable %, just say what's happening.
  if (job.phase === 'converting' || job.phase === 'merging') {
    const label = job.phase === 'merging' ? 'Merging…' : 'Converting…';
    return `<span class="m-pct">${label}</span><span class="m-speed"></span><span class="m-eta"></span>`;
  }
  // Downloading: show % when known, else bytes received so far.
  const known = Number.isFinite(parseFloat(job.percent));
  const left = known ? `${Math.round(clampPct(job))}%` : esc(job.totalSize || 'Receiving…');
  const eta = job.eta ? `ETA ${esc(job.eta)}` : '';
  return `<span class="m-pct">${left}</span><span class="m-speed">${esc(job.speed || '')}</span><span class="m-eta">${eta}</span>`;
}

function actionsHTML(job) {
  const id = esc(job.id);
  if (job.status === 'running' || job.status === 'queued') {
    return `<button class="act danger" data-act="cancel" data-id="${id}">${I.x} Cancel</button>`;
  }
  if (job.status === 'complete') {
    return `
      <button class="act primary" data-act="open" data-id="${id}">${I.open} Open</button>
      <button class="act act-icon" data-act="reveal" data-id="${id}" title="Reveal in folder">${I.folder}</button>
      <button class="act act-icon" data-act="copy" data-id="${id}" title="Copy link">${I.link}</button>
      <button class="act act-icon" data-act="remove" data-id="${id}" title="Remove">${I.trash}</button>`;
  }
  if (job.status === 'error') {
    return `
      <button class="act primary" data-act="retry" data-id="${id}">${I.retry} Retry</button>
      <button class="act act-icon" data-act="copy" data-id="${id}" title="Copy link">${I.link}</button>
      <button class="act act-icon danger" data-act="remove" data-id="${id}" title="Remove">${I.trash}</button>`;
  }
  // canceled
  return `
    <button class="act primary" data-act="retry" data-id="${id}">${I.retry} Retry</button>
    <button class="act act-icon danger" data-act="remove" data-id="${id}" title="Remove">${I.trash}</button>`;
}

function thumbHTML(job) {
  if (job.thumbnail) {
    return `<div class="card-thumb"><img src="${esc(job.thumbnail)}" alt="" loading="lazy"></div>`;
  }
  const glyph = job.preset === 'mp4' ? GLYPH.video : GLYPH.audio;
  return `<div class="card-thumb card-thumb-empty">${glyph}</div>`;
}

function progressHTML(job) {
  const ind = isIndeterminate(job);
  const fill = ind
    ? '<div class="bar-fill indeterminate"></div>'
    : `<div class="bar-fill" style="width:${clampPct(job)}%"></div>`;
  return `<div class="progress"><div class="bar">${fill}</div><div class="metrics">${metricsHTML(job)}</div></div>`;
}

function cardInnerHTML(job) {
  const showProgress = job.status !== 'error';
  return `
    <div class="card-top">
      <span class="fmt-badge">${esc(job.preset.toUpperCase())}</span>
      <span class="card-status"><span class="s-dot"></span><span class="s-label">${esc(cardStatusLabel(job))}</span></span>
    </div>
    <div class="card-head">
      ${thumbHTML(job)}
      <div class="card-headings">
        <div class="card-title">${esc(job.title || 'Untitled')}</div>
        <div class="card-sub">${esc(sourceLabel(job))}</div>
      </div>
    </div>
    ${showProgress ? progressHTML(job) : ''}
    ${job.status === 'error' ? `<div class="card-error"><span class="card-error-msg">${esc(job.error || 'Download failed.')}</span><button class="error-action" data-act="advanced" data-id="${esc(job.id)}">${I.search} Try advanced search</button></div>` : ''}
    <div class="card-foot">${actionsHTML(job)}</div>`;
}

function createCard(job, index) {
  const el = document.createElement('div');
  el.className = `card status-${job.status} fmt-${job.preset}`;
  el.dataset.jobId = job.id;
  el.dataset.status = job.status;
  el.style.setProperty('--i', index % 12);
  el.innerHTML = cardInnerHTML(job);
  return el;
}

function updateCardEl(el, job) {
  if (el.dataset.status !== job.status) {
    el.dataset.status = job.status;
    el.className = `card status-${job.status} fmt-${job.preset}`;
    el.innerHTML = cardInnerHTML(job);
    return;
  }
  const sLabel = el.querySelector('.s-label');
  if (sLabel) sLabel.textContent = cardStatusLabel(job);
  const fill = el.querySelector('.bar-fill');
  if (fill) {
    const ind = isIndeterminate(job);
    fill.classList.toggle('indeterminate', ind);
    fill.style.width = ind ? '' : `${clampPct(job)}%`;
  }
  const metrics = el.querySelector('.metrics');
  if (metrics) metrics.innerHTML = metricsHTML(job);
}

function renderCounts() {
  const counts = {
    all: state.jobs.length,
    active: state.jobs.filter((j) => j.status === 'queued' || j.status === 'running').length,
    completed: state.jobs.filter((j) => j.status === 'complete').length,
    audio: state.jobs.filter((j) => j.preset === 'mp3' || j.preset === 'wav').length,
    video: state.jobs.filter((j) => j.preset === 'mp4').length,
  };
  $$('[data-count]').forEach((el) => {
    el.textContent = counts[el.dataset.count] ?? 0;
  });
  queueCount.textContent = state.jobs.filter((j) => matchesFilter(j, state.filter)).length;
}

function renderAll() {
  renderCounts();
  const list = state.jobs.filter((j) => matchesFilter(j, state.filter));
  grid.innerHTML = '';
  list.forEach((job, i) => grid.appendChild(createCard(job, i)));
  emptyState.style.display = list.length ? 'none' : 'flex';
  grid.style.display = list.length ? 'grid' : 'none';
}

function upsertJob(job) {
  const idx = state.jobs.findIndex((j) => j.id === job.id);
  if (idx >= 0) state.jobs[idx] = job;
  else state.jobs.unshift(job);

  const el = grid.querySelector(`[data-job-id="${job.id}"]`);
  if (el && matchesFilter(job, state.filter)) {
    updateCardEl(el, job);
    renderCounts();
  } else {
    renderAll();
  }
}

async function refreshJobs() {
  try {
    const jobs = await invoke('get_jobs');
    state.jobs = jobs || [];
    renderAll();
  } catch {
    /* ignore */
  }
}

// ─── Engine ─────────────────────────────────────────────────────
function applyEngine(snapshot) {
  state.engine = snapshot;
  const yt = snapshot.yt_dlp || snapshot.ytDlp || {};
  const ff = snapshot.ffmpeg || {};
  const ready = snapshot.ready;

  engineDot.className = 'engine-dot';
  if (ready) {
    engineDot.classList.add('is-ready');
    engineStateEl.textContent = 'Engine ready';
  } else if (yt.status === 'downloading' || ff.status === 'downloading') {
    engineDot.classList.add('is-busy');
    engineStateEl.textContent = 'Preparing engine…';
  } else if (yt.status === 'error' || ff.status === 'error') {
    engineDot.classList.add('is-error');
    engineStateEl.textContent = 'Engine error';
  } else {
    engineDot.classList.add('is-busy');
    engineStateEl.textContent = 'Checking engine…';
  }
  engineDetail.textContent = `yt-dlp ${yt.status || '—'} · ffmpeg ${ff.status || '—'}`;

  downloadBtn.disabled = !ready;
  downloadBtn.title = ready ? '' : 'The download engine is still preparing.';

  // Settings tags
  const ytTag = $('#ytTag');
  const ffTag = $('#ffTag');
  const ytVersion = $('#ytVersion');
  const ffVersion = $('#ffVersion');
  if (ytTag) { ytTag.textContent = yt.status || 'pending'; ytTag.className = `status-tag ${yt.status || ''}`; }
  if (ffTag) { ffTag.textContent = ff.status || 'pending'; ffTag.className = `status-tag ${ff.status || ''}`; }
  if (ytVersion) ytVersion.textContent = yt.version || (yt.error ? esc(yt.error) : '—');
  if (ffVersion) ffVersion.textContent = ff.version || (ff.error ? esc(ff.error) : '—');
}

async function refreshEngine() {
  try {
    applyEngine(await invoke('get_engine_status'));
  } catch {
    /* ignore */
  }
}

// ─── Settings ───────────────────────────────────────────────────
function applySettings(settings) {
  state.settings = settings;
  const root = document.documentElement;
  // Brand identity is locked: always dark, always Siphon green.
  root.dataset.theme = 'dark';
  root.style.setProperty('--accent', '#4FB286');
  root.style.setProperty('--surface-opacity', settings.surfaceOpacity ?? 0.82);
  root.style.setProperty('--glass-blur', `${settings.glassBlur ?? 14}px`);

  const opacitySlider = $('#opacitySlider');
  const opacityValue = $('#opacityValue');
  const blurSlider = $('#blurSlider');
  const blurValue = $('#blurValue');
  const shortcutDisplay = $('#shortcutDisplay');

  if (opacitySlider) opacitySlider.value = Math.round((settings.surfaceOpacity ?? 0.82) * 100);
  if (opacityValue) opacityValue.textContent = `${Math.round((settings.surfaceOpacity ?? 0.82) * 100)}%`;
  if (blurSlider) blurSlider.value = settings.glassBlur ?? 14;
  if (blurValue) blurValue.textContent = `${settings.glassBlur ?? 14}px`;
  if (shortcutDisplay) shortcutDisplay.textContent = fmtShortcut(settings.shortcut);

  applyDownloadSettings(settings);
}

function applyDownloadSettings(settings) {
  const mode = settings.downloadMode || 'single';
  $$('#dlModeGroup .dl-opt').forEach((b) => b.classList.toggle('active', b.dataset.mode === mode));
  const singleRow = $('#dlSingleRow');
  const typeRow = $('#dlTypeRow');
  if (singleRow) singleRow.hidden = mode !== 'single';
  if (typeRow) typeRow.hidden = mode !== 'byType';
  const fp = $('#dlFolderPath');
  if (fp) fp.textContent = settings.downloadDir || '—';
  const vp = $('#dlVideoPath');
  if (vp) vp.textContent = settings.videoDir || '—';
  const ap = $('#dlAudioPath');
  if (ap) ap.textContent = settings.audioDir || '—';
}

async function loadSettings() {
  try {
    applySettings(await invoke('get_settings'));
  } catch {
    applySettings({ theme: 'dark', accentColor: '#4FB286', surfaceOpacity: 0.82, glassBlur: 14, shortcut: 'CommandOrControl+Shift+S' });
  }
}

// Reflect the OS-level autostart state onto the On/Off segmented toggle.
function syncAutostartUI(enabled) {
  $$('#autostartGroup .dl-opt').forEach((b) => b.classList.toggle('active', (b.dataset.on === 'true') === enabled));
}

async function loadAutostart() {
  try {
    syncAutostartUI(await invoke('get_autostart'));
  } catch {
    syncAutostartUI(false);
  }
}

let appearanceTimer = null;
function persistAppearance() {
  clearTimeout(appearanceTimer);
  appearanceTimer = setTimeout(async () => {
    const accent = '#4FB286';
    const surfaceOpacity = Number($('#opacitySlider').value) / 100;
    const glassBlur = Number($('#blurSlider').value);
    try {
      const settings = await invoke('set_appearance', { accentColor: accent, surfaceOpacity, glassBlur });
      if (settings) state.settings = settings;
    } catch {
      /* ignore */
    }
  }, 250);
}

function openSettings() { $('#settingsOverlay').classList.add('active'); }
function closeSettings() { $('#settingsOverlay').classList.remove('active'); }

// ─── View switching (Library ↔ Advanced Search) ─────────────────
function setView(view) {
  state.view = view;
  const advanced = view === 'advanced';
  $('#intake').hidden = advanced;
  $('#queue').hidden = advanced;
  $('#advanced').hidden = !advanced;
}

// Jump to Advanced Search with `url` pre-filled and start sniffing — used by the
// "Try advanced search" button on a failed download's error card.
function goAdvancedWith(url) {
  const advNav = $('#advNav');
  $$('.filter').forEach((b) => b.classList.toggle('active', b === advNav));
  setView('advanced');
  advUrl.value = url;
  startSniff();
}

// ─── Advanced Search (stream sniffer) ───────────────────────────
function setAdvStatus(message) {
  if (!message) {
    advStatus.hidden = true;
    advStatus.textContent = '';
  } else {
    advStatus.textContent = message;
    advStatus.hidden = false;
  }
}

function advTitleFromPage(pageUrl) {
  try {
    return `${new URL(pageUrl).hostname.replace(/^www\./, '')} stream`;
  } catch {
    return 'Stream';
  }
}

function advRowHTML(stream) {
  const url = esc(stream.streamUrl);
  const kind = stream.kind === 'dash' ? 'dash' : 'hls';
  const kindLabel = kind === 'dash' ? 'DASH' : 'HLS';
  return `
    <div class="adv-row" data-stream="${url}">
      <div class="adv-row-main">
        <span class="adv-kind ${kind}">${kindLabel}</span>
        <span class="adv-url mono" title="${url}">${url}</span>
      </div>
      <div class="adv-row-actions">
        <div class="adv-seg" data-preset="mp4" role="group" aria-label="Output format">
          <button type="button" class="adv-seg-btn active" data-p="mp4">MP4</button>
          <button type="button" class="adv-seg-btn" data-p="mp3">MP3</button>
          <button type="button" class="adv-seg-btn" data-p="wav">WAV</button>
        </div>
        <button type="button" class="btn btn-primary btn-sm adv-dl">Download</button>
      </div>
    </div>`;
}

function renderAdvResults() {
  const n = state.advStreams.length;
  advResults.innerHTML = state.advStreams.map(advRowHTML).join('');
  advEmpty.style.display = n ? 'none' : 'flex';
  advResultsHead.hidden = n === 0;
  advCount.textContent = n;
}

function clearStreams() {
  state.advStreams = [];
  renderAdvResults();
  setAdvStatus(null);
}

function addAdvStream(payload) {
  if (!payload || !payload.streamUrl) return;
  if (state.advStreams.some((s) => s.streamUrl === payload.streamUrl)) return;
  state.advStreams.push(payload);
  renderAdvResults();
  const n = state.advStreams.length;
  setAdvStatus(`${n} stream${n > 1 ? 's' : ''} found — pick a format and download.`);
}

async function startSniff() {
  const url = advUrl.value.trim();
  if (!url) { setAdvStatus('Paste a page URL first.'); return; }

  state.advStreams = [];
  state.advPage = url;
  renderAdvResults();
  advFind.disabled = true;
  try {
    await invoke('open_sniffer', { url });
    setAdvStatus('Finder opening… sign in / press Play there. Detected streams appear here automatically — the finder stays open so you can grab more, then hit Stop.');
    advClose.hidden = false;
  } catch (e) {
    setAdvStatus(errMessage(e));
  } finally {
    advFind.disabled = false;
  }
}

async function stopSniff() {
  try { await invoke('close_sniffer'); } catch { /* ignore */ }
  advClose.hidden = true;
  setAdvStatus(state.advStreams.length ? `${state.advStreams.length} stream(s) found.` : 'Stopped. No streams found yet.');
}

async function clearFinderData() {
  advClearData.disabled = true;
  advClose.hidden = true; // the finder is closed as part of clearing
  setAdvStatus('Clearing finder data…');
  try {
    // Completion arrives via the 'sniffer://cleared' event.
    await invoke('clear_finder_data');
  } catch (e) {
    advClearData.disabled = false;
    setAdvStatus(errMessage(e));
    toast(errMessage(e), true);
  }
}

async function downloadStream(url, preset) {
  try {
    const job = await invoke('start_download', {
      url,
      preset,
      title: advTitleFromPage(state.advPage),
      thumbnail: null,
      referer: state.advPage || null,
    });
    if (job) {
      upsertJob(job);
      toast('Download started — see it under Library.');
    }
  } catch (e) {
    setAdvStatus(errMessage(e));
  }
}

// ─── Wiring ─────────────────────────────────────────────────────
function wireUI() {
  // Preset segmented control
  $$('.seg').forEach((btn) => btn.addEventListener('click', () => setPreset(btn.dataset.preset)));

  // URL field
  urlInput.addEventListener('input', () => {
    urlField.classList.toggle('has-value', urlInput.value.trim().length > 0);
    if (!intakeError.hidden) hideError();
  });
  urlClear.addEventListener('click', () => {
    urlInput.value = '';
    urlField.classList.remove('has-value');
    urlInput.focus();
    hidePreview();
    state.info = null;
  });

  previewBtn.addEventListener('click', doPreview);
  downloadForm.addEventListener('submit', submitDownload);
  intakeAdvBtn.addEventListener('click', () => goAdvancedWith(urlInput.value.trim()));

  // Download sub-filter disclosure (Active / Completed / Audio / Video fold
  // under the main "Download" item; collapsed by default).
  const downloadToggle = $('#downloadToggle');
  const downloadSub = $('#downloadSub');
  function setDownloadGroup(open) {
    downloadSub.classList.toggle('open', open);
    downloadToggle.setAttribute('aria-expanded', open ? 'true' : 'false');
  }
  downloadToggle.addEventListener('click', () => {
    setDownloadGroup(!downloadSub.classList.contains('open'));
  });

  // Filters + view switch (Library filters and the Advanced Search tab
  // are all `.filter` buttons; `data-view` marks a full view switch).
  $$('.filter').forEach((btn) => btn.addEventListener('click', () => {
    $$('.filter').forEach((b) => b.classList.toggle('active', b === btn));
    if (btn.dataset.view === 'advanced') {
      setView('advanced');
      advUrl.focus();
      return;
    }
    // Selecting a folded filter keeps the group open so the active one stays visible.
    if (btn.classList.contains('sub')) setDownloadGroup(true);
    state.filter = btn.dataset.filter;
    setView('library');
    renderAll();
  }));

  // Advanced Search
  advFind.addEventListener('click', startSniff);
  advUrl.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { e.preventDefault(); startSniff(); }
  });
  advClose.addEventListener('click', stopSniff);
  advClearBtn.addEventListener('click', clearStreams);
  advClearData.addEventListener('click', clearFinderData);
  advResults.addEventListener('click', (e) => {
    const segBtn = e.target.closest('.adv-seg-btn');
    if (segBtn) {
      const group = segBtn.closest('.adv-seg');
      group.querySelectorAll('.adv-seg-btn').forEach((b) => b.classList.toggle('active', b === segBtn));
      group.dataset.preset = segBtn.dataset.p;
      return;
    }
    const dl = e.target.closest('.adv-dl');
    if (dl) {
      const row = dl.closest('.adv-row');
      const streamUrl = row.dataset.stream;
      const preset = row.querySelector('.adv-seg').dataset.preset || 'mp4';
      downloadStream(streamUrl, preset);
    }
  });

  // Card actions (delegated)
  grid.addEventListener('click', async (e) => {
    const btn = e.target.closest('[data-act]');
    if (!btn) return;
    const { act, id } = btn.dataset;
    const job = state.jobs.find((j) => j.id === id);
    if (!job) return;

    if (act === 'open' && job.outputPath) {
      try { await invoke('open_file', { path: job.outputPath }); } catch (err) { toast(errMessage(err), true); }
    } else if (act === 'reveal' && job.outputPath) {
      try { await invoke('reveal_in_folder', { path: job.outputPath }); } catch (err) { toast(errMessage(err), true); }
    } else if (act === 'copy') {
      copyText(job.url);
    } else if (act === 'cancel') {
      try { await invoke('cancel_download', { id }); } catch { /* ignore */ }
    } else if (act === 'remove') {
      try {
        const jobs = await invoke('remove_job', { id });
        state.jobs = jobs || [];
        renderAll();
      } catch { /* ignore */ }
    } else if (act === 'retry') {
      await startDownloadWith(job.url, job.preset, job.title, job.thumbnail);
    } else if (act === 'advanced') {
      goAdvancedWith(job.url);
    }
  });

  // Clear finished
  $('#clearFinishedBtn').addEventListener('click', async () => {
    try {
      const jobs = await invoke('clear_finished');
      state.jobs = jobs || [];
      renderAll();
      toast('Cleared finished');
    } catch { /* ignore */ }
  });

  // Open folder
  $('#openFolderBtn').addEventListener('click', () => invoke('open_downloads_dir').catch(() => {}));

  // Download location
  $$('#dlModeGroup .dl-opt').forEach((btn) => btn.addEventListener('click', async () => {
    try {
      const s = await invoke('set_download_mode', { mode: btn.dataset.mode });
      if (s) { state.settings = s; applyDownloadSettings(s); }
    } catch { /* ignore */ }
  }));
  $('#dlChangeBtn').addEventListener('click', async () => {
    try {
      const s = await invoke('pick_download_dir');
      if (s) { state.settings = s; applyDownloadSettings(s); toast('Download folder updated'); }
    } catch { /* ignore */ }
  });

  // Startup (autostart)
  $$('#autostartGroup .dl-opt').forEach((btn) => btn.addEventListener('click', async () => {
    const enabled = btn.dataset.on === 'true';
    try {
      const result = await invoke('set_autostart', { enabled });
      syncAutostartUI(result);
      toast(result ? 'Siphon will start on login' : 'Start on login disabled');
    } catch {
      // Revert UI to the real OS state on failure.
      loadAutostart();
      toast('Could not change startup setting', true);
    }
  }));

  // Settings
  $('#settingsBtn').addEventListener('click', openSettings);
  $('#settingsClose').addEventListener('click', closeSettings);
  $('#settingsOverlay').addEventListener('click', (e) => {
    if (e.target.id === 'settingsOverlay') closeSettings();
  });
  $('#retryEngineBtn').addEventListener('click', async () => {
    try { await invoke('prepare_engine'); toast('Re-checking engine…'); } catch { /* ignore */ }
  });

  // Appearance controls
  const opacitySlider = $('#opacitySlider');
  opacitySlider.addEventListener('input', () => {
    document.documentElement.style.setProperty('--surface-opacity', Number(opacitySlider.value) / 100);
    $('#opacityValue').textContent = `${opacitySlider.value}%`;
    persistAppearance();
  });
  const blurSlider = $('#blurSlider');
  blurSlider.addEventListener('input', () => {
    document.documentElement.style.setProperty('--glass-blur', `${blurSlider.value}px`);
    $('#blurValue').textContent = `${blurSlider.value}px`;
    persistAppearance();
  });

  // Window controls
  $('#winMinimize').addEventListener('click', () => invoke('window_minimize').catch(() => {}));
  $('#winMaximize').addEventListener('click', () => invoke('window_maximize').catch(() => {}));
  $('#winClose').addEventListener('click', () => invoke('window_close').catch(() => {}));

  // Keyboard
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') closeSettings();
  });
}

// ─── Init ───────────────────────────────────────────────────────
async function init() {
  await loadSettings();
  loadAutostart();
  setPreset('mp4');
  wireUI();
  renderAdvResults();
  await refreshEngine();
  await refreshJobs();

  listen('engine://status', (e) => applyEngine(e.payload));
  listen('download://update', (e) => upsertJob(e.payload));
  listen('jobs-changed', () => refreshJobs());
  listen('sniffer://found', (e) => addAdvStream(e.payload));
  listen('sniffer://status', (e) => {
    const payload = e.payload;
    if (payload === 'interactive') {
      setAdvStatus('Opened the page in a browser window — sign in and press play, then come back here.');
    } else if (typeof payload === 'string' && payload.startsWith('error:')) {
      setAdvStatus(payload.slice(6).trim());
    }
  });
  listen('sniffer://cleared', (e) => {
    advClearData.disabled = false;
    const payload = e.payload;
    if (typeof payload === 'string' && payload.startsWith('error')) {
      setAdvStatus('Could not clear finder data.');
      toast('Could not clear finder data.', true);
    } else {
      setAdvStatus('Finder data cleared — you are signed out of every site.');
      toast('Finder data cleared.');
    }
  });

  requestAnimationFrame(() => document.body.classList.remove('app-booting'));
}

init();
