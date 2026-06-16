/* ═══════════════════════════════════════════════════════════════
   Siphon — quick-paste popup logic
   ═══════════════════════════════════════════════════════════════ */

const PRESETS = ['mp4', 'mp3', 'wav'];
let preset = 'mp4';

async function invoke(cmd, args) {
  for (let i = 0; i < 30; i++) {
    if (window.__TAURI__ && window.__TAURI__.core) {
      return window.__TAURI__.core.invoke(cmd, args);
    }
    await new Promise((r) => setTimeout(r, 80));
  }
  throw new Error('Tauri API unavailable.');
}

const $ = (s) => document.querySelector(s);
const urlInput = $('#qsUrl');
const presetGroup = $('#qsPresetGroup');
const errorEl = $('#qsError');
const openFileToggle = $('#qsOpenFile');
const openFolderToggle = $('#qsOpenFolder');

function showError(message) {
  errorEl.textContent = message;
  errorEl.hidden = false;
}

function hideError() {
  errorEl.hidden = true;
}

function clientUnsupported(value) {
  try {
    const url = new URL(value);
    const host = url.hostname.replace(/^www\./, '').toLowerCase();
    const path = url.pathname.toLowerCase();
    const yt = host === 'youtube.com' || host === 'm.youtube.com';
    if (yt && path === '/results') return 'Paste a direct video link, not a search page.';
    if (yt && (path.startsWith('/@') || path.startsWith('/channel/') || path.startsWith('/c/') || path.startsWith('/user/'))) {
      return 'Paste a direct video link, not a channel page.';
    }
  } catch {
    return null;
  }
  return null;
}

function setPreset(value) {
  preset = value;
  const idx = Math.max(0, PRESETS.indexOf(value));
  presetGroup.style.setProperty('--seg-i', idx);
  document.querySelectorAll('.seg').forEach((b) => b.classList.toggle('active', b.dataset.preset === value));
}

async function applyTheme() {
  try {
    const settings = await invoke('get_settings');
    document.documentElement.dataset.theme = settings.theme || 'dark';
    document.documentElement.style.setProperty('--accent', settings.accentColor || '#4FB286');
    document.documentElement.style.setProperty('--surface-opacity', settings.surfaceOpacity ?? 0.82);
    document.documentElement.style.setProperty('--glass-blur', `${settings.glassBlur ?? 14}px`);
  } catch {
    /* ignore */
  }
}

async function submit(event) {
  event.preventDefault();
  const url = urlInput.value.trim();
  if (!url) return showError('Paste a link first.');
  const unsupported = clientUnsupported(url);
  if (unsupported) return showError(unsupported);

  hideError();
  try {
    await invoke('start_download', {
      url,
      preset,
      title: null,
      thumbnail: null,
      autoOpen: openFileToggle.checked,
      autoReveal: openFolderToggle.checked,
    });
    await invoke('close_quicksave');
  } catch (e) {
    showError(typeof e === 'string' ? e : (e?.message || 'Could not start download.'));
  }
}

function init() {
  applyTheme();
  setPreset('mp4');

  // Remember the "when done" toggles between sessions.
  openFileToggle.checked = localStorage.getItem('qsOpenFile') === '1';
  openFolderToggle.checked = localStorage.getItem('qsOpenFolder') === '1';
  openFileToggle.addEventListener('change', () => localStorage.setItem('qsOpenFile', openFileToggle.checked ? '1' : '0'));
  openFolderToggle.addEventListener('change', () => localStorage.setItem('qsOpenFolder', openFolderToggle.checked ? '1' : '0'));

  document.querySelectorAll('.seg').forEach((btn) => {
    btn.addEventListener('click', () => setPreset(btn.dataset.preset));
  });
  $('#qsForm').addEventListener('submit', submit);
  $('#qsClose').addEventListener('click', () => invoke('close_quicksave').catch(() => {}));
  $('#qsCancel').addEventListener('click', () => invoke('close_quicksave').catch(() => {}));
  urlInput.addEventListener('input', hideError);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') invoke('close_quicksave').catch(() => {});
  });

  // Pre-fill from clipboard if it looks like a URL (via the Tauri clipboard
  // plugin so the webview never shows a permission prompt).
  readClipboard().then((text) => {
    if (text && /^https?:\/\//i.test(text.trim())) {
      urlInput.value = text.trim();
    }
    urlInput.focus();
  });
}

async function readClipboard() {
  try {
    if (window.__TAURI__ && window.__TAURI__.clipboardManager) {
      return (await window.__TAURI__.clipboardManager.readText()) || '';
    }
  } catch {
    /* ignore */
  }
  return '';
}

init();
