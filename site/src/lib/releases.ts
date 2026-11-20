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
  zipName: string;
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
  const zip =
    assets.find((asset) => asset.name.toLowerCase().endsWith('-windows-x64.zip')) ??
    assets.find((asset) => asset.name.toLowerCase().endsWith('.zip'));

  if (!zip) return null;

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
    url: zip.browser_download_url,
    size: zip.size,
    prerelease: chosen.prerelease,
    zipName: zip.name,
    label,
    meta: `${releaseName} · ${formatBytes(zip.size)}`,
  };
}
