export interface ReleaseAsset {
  name: string;
  size: number;
  browser_download_url: string;
}

export interface Release {
  tag_name: string;
  prerelease: boolean;
  draft: boolean;
  assets: ReleaseAsset[];
}

export interface DownloadInfo {
  tag: string;
  url: string;
  size: number;
  prerelease: boolean;
  fileName: string;
  label: string;
  meta: string;
}

export function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  if (bytes >= 1024) {
    return `${Math.round(bytes / 1024)} KB`;
  }

  return `${bytes} B`;
}

export function selectRelease(releases: Release[]): DownloadInfo | null {
  const available = releases.filter((release) => !release.draft);
  const chosen = available.find((release) => !release.prerelease) ?? available[0];
  if (!chosen) return null;

  const assets = chosen.assets ?? [];
  const executable =
    assets.find((asset) => asset.name.toLowerCase() === 'hiyodyna.exe') ??
    assets.find((asset) => asset.name.toLowerCase().endsWith('.exe'));

  if (!executable) return null;

  const build = /^pre-(\d+)$/.exec(chosen.tag_name);
  const label = chosen.prerelease
    ? build
      ? `Download build ${build[1]}`
      : 'Download pre-release'
    : `Download ${chosen.tag_name}`;
  const releaseName = chosen.prerelease
    ? build
      ? `Pre-release build ${build[1]}`
      : 'Pre-release'
    : `Stable ${chosen.tag_name}`;

  return {
    tag: chosen.tag_name,
    url: executable.browser_download_url,
    size: executable.size,
    prerelease: chosen.prerelease,
    fileName: executable.name,
    label,
    meta: `${releaseName} · ${formatBytes(executable.size)}`,
  };
}
