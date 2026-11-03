// Thin bridge between the Rust client and browser APIs that have no good
// Rust equivalent: pdf.js page rendering, the in-memory prefetch cache,
// the screen wake lock and fullscreen.
//
// Nothing here persists: downloaded sheets live in memory until the tab
// is closed (the client must not store data on the device for now).

const PDFJS = '/vendor/pdfjs/pdf.min.mjs';
const PDFJS_WORKER = '/vendor/pdfjs/pdf.worker.min.mjs';

/** url -> Promise<Uint8Array>: the raw files of tonight's programme. */
const bytes = new Map();
/** url -> Promise<PDFDocumentProxy> */
const docs = new Map();
/** canvas id -> running RenderTask */
const tasks = new Map();
let pdfjsLib = null;

function pdfjs() {
  if (!pdfjsLib) {
    pdfjsLib = import(PDFJS).then((lib) => {
      lib.GlobalWorkerOptions.workerSrc = PDFJS_WORKER;
      return lib;
    });
  }
  return pdfjsLib;
}

async function download(url) {
  const response = await fetch(url, { credentials: 'same-origin' });
  if (!response.ok) {
    throw new Error(`download failed: HTTP ${response.status}`);
  }
  return new Uint8Array(await response.arrayBuffer());
}

/** Downloads a file into memory unless it is already there or underway. */
export function prefetch(url) {
  if (!bytes.has(url)) {
    const p = download(url);
    bytes.set(url, p);
    p.catch(() => bytes.delete(url));
  }
  return bytes.get(url).then(() => undefined);
}

export function isCached(url) {
  return bytes.has(url);
}

function open(url) {
  if (!docs.has(url)) {
    const p = (async () => {
      const lib = await pdfjs();
      await prefetch(url);
      const data = await bytes.get(url);
      // pdf.js hands the buffer to its worker; give it a copy so the
      // in-memory cache keeps its bytes.
      return lib.getDocument({ data: data.slice() }).promise;
    })();
    docs.set(url, p);
    p.catch(() => docs.delete(url));
  }
  return docs.get(url);
}

export async function pageCount(url) {
  return (await open(url)).numPages;
}

/**
 * Renders page `index` (0-based) as large as fits into boxWidth x boxHeight
 * CSS pixels, sharp on high-density screens. Returns [cssWidth, cssHeight]
 * of the drawn page, or null if a newer render replaced this one.
 */
export async function renderPage(url, index, canvasId, boxWidth, boxHeight) {
  const pdf = await open(url);
  const page = await pdf.getPage(index + 1);
  const base = page.getViewport({ scale: 1 });
  const scale = Math.max(0.1, Math.min(boxWidth / base.width, boxHeight / base.height));
  const viewport = page.getViewport({ scale });
  const dpr = window.devicePixelRatio || 1;

  // Render off-screen first so the visible page never flashes blank.
  const off = document.createElement('canvas');
  off.width = Math.floor(viewport.width * dpr);
  off.height = Math.floor(viewport.height * dpr);
  tasks.get(canvasId)?.cancel();
  const task = page.render({
    canvas: off,
    canvasContext: off.getContext('2d'),
    viewport,
    transform: dpr === 1 ? undefined : [dpr, 0, 0, dpr, 0, 0],
  });
  tasks.set(canvasId, task);
  try {
    await task.promise;
  } catch (e) {
    if (e && e.name === 'RenderingCancelledException') {
      return null;
    }
    throw e;
  } finally {
    if (tasks.get(canvasId) === task) {
      tasks.delete(canvasId);
    }
  }
  const canvas = document.getElementById(canvasId);
  if (!canvas) {
    return null;
  }
  canvas.width = off.width;
  canvas.height = off.height;
  canvas.style.width = `${viewport.width}px`;
  canvas.style.height = `${viewport.height}px`;
  canvas.getContext('2d').drawImage(off, 0, 0);
  return [viewport.width, viewport.height];
}

/** Drops everything not in `keep` (array of urls) from memory. */
export function retainOnly(keep) {
  const wanted = new Set(keep);
  for (const url of [...bytes.keys()]) {
    if (!wanted.has(url)) {
      bytes.delete(url);
      const doc = docs.get(url);
      docs.delete(url);
      doc?.then((d) => d.destroy()).catch(() => {});
    }
  }
}

/**
 * key -> { url, revision, bytes }: the latest unsaved annotation pages. The
 * reader saves them 1.5 s after the last stroke; if the tab is closed or
 * reloaded before that, they are sent with a keepalive request (the browser
 * completes those even while the page unloads).
 */
const pendingPages = new Map();

export function setPending(key, url, revision, bytes) {
  // `bytes` is a view into WebAssembly memory, valid only during this call.
  pendingPages.set(key, { url, revision, bytes: bytes.slice() });
}

export function clearPending(key) {
  pendingPages.delete(key);
}

window.addEventListener('pagehide', () => {
  for (const { url, revision, bytes } of pendingPages.values()) {
    fetch(url, {
      method: 'PUT',
      keepalive: true,
      credentials: 'same-origin',
      headers: {
        'content-type': 'application/octet-stream',
        'if-match': `"${revision}"`,
        'x-musicus-csrf': '1',
      },
      body: bytes,
    }).catch(() => {});
  }
  pendingPages.clear();
});

let wakeLock = null;

/** Keeps the screen on while reading (re-acquired after tab switches). */
export async function keepAwake(on) {
  try {
    if (on && !wakeLock && 'wakeLock' in navigator) {
      wakeLock = await navigator.wakeLock.request('screen');
      wakeLock.addEventListener('release', () => { wakeLock = null; });
    } else if (!on && wakeLock) {
      await wakeLock.release();
      wakeLock = null;
    }
  } catch (_) {
    // Not allowed (e.g. no user gesture yet or insecure context): ignore.
  }
}

document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible' && document.body.dataset.keepAwake === 'on') {
    keepAwake(true);
  }
});

export function toggleFullscreen() {
  if (document.fullscreenElement) {
    document.exitFullscreen?.();
  } else {
    document.documentElement.requestFullscreen?.().catch(() => {});
  }
}

/** Copies text to the clipboard; resolves to false if not allowed. */
export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (_) {
    return false;
  }
}
