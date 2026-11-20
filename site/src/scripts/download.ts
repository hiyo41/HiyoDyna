import { selectRelease, type DownloadInfo, type Release } from '../lib/releases';

const CACHE_KEY = 'hiyodyna:release:v1';
const CACHE_TTL_MS = 30 * 60 * 1000;

interface CachedRelease {
  at: number;
  info: DownloadInfo;
}

export function applyDownloadInfo(info: DownloadInfo): void {
  document.querySelectorAll<HTMLAnchorElement>('[data-download]').forEach((link) => {
    link.href = info.url;
    link.textContent = info.label;
    link.title = `${info.fileName} · ${info.meta}`;
  });

  document.querySelectorAll<HTMLElement>('[data-download-meta]').forEach((meta) => {
    meta.textContent = info.meta;
  });
}

export function initDownload(): void {
  const cached = readCache();
  if (cached) applyDownloadInfo(cached);

  const load = () => {
    void fetchLatest();
  };

  if (typeof window.requestIdleCallback === 'function') {
    window.requestIdleCallback(load, { timeout: 2000 });
  } else {
    window.setTimeout(load, 0);
  }
}

async function fetchLatest(): Promise<void> {
  try {
    const response = await fetch(
      'https://api.github.com/repos/hiyo41/HiyoDyna/releases?per_page=20',
      { headers: { Accept: 'application/vnd.github+json' } },
    );

    if (!response.ok) return;

    const data = (await response.json()) as Release[];
    if (!Array.isArray(data)) return;

    const info = selectRelease(data);
    if (!info) return;

    applyDownloadInfo(info);
    writeCache(info);
  } catch {
    // Keep the static Releases link when the API is unavailable or rate-limited.
  }
}

function readCache(): DownloadInfo | null {
  try {
    const raw = sessionStorage.getItem(CACHE_KEY);
    if (!raw) return null;

    const cached = JSON.parse(raw) as CachedRelease;
    if (!cached?.info || Date.now() - cached.at > CACHE_TTL_MS) return null;

    return cached.info;
  } catch {
    return null;
  }
}

function writeCache(info: DownloadInfo): void {
  try {
    sessionStorage.setItem(CACHE_KEY, JSON.stringify({ at: Date.now(), info } satisfies CachedRelease));
  } catch {
    // Storage can be unavailable; the page still works without it.
  }
}
