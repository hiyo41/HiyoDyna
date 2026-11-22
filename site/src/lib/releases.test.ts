import { describe, expect, it } from 'vitest';
import { formatBytes, selectRelease, type Release } from './releases';

const asset = (name: string, size = 867371) => ({
  name,
  size,
  browser_download_url: `https://example.test/${name}`,
});

const executable = (size = 1719296) => asset('HiyoDyna.exe', size);

const release = (
  tag: string,
  prerelease: boolean,
  assets = [executable()],
): Release => ({
  tag_name: tag,
  prerelease,
  draft: false,
  assets,
});

describe('selectRelease', () => {
  it('prefers an official release over a newer pre-release', () => {
    const official = release('v0.0.1-9', false);
    const info = selectRelease([release('pre-5', true), official]);
    expect(info?.tag).toBe('v0.0.1-9');
    expect(info?.prerelease).toBe(false);
    expect(info?.label).toBe('Download v0.0.1-9');
    expect(info?.meta).toBe('Stable v0.0.1-9 · 1.6 MB');
  });

  it('falls back to the newest pre-release', () => {
    const info = selectRelease([release('pre-5', true)]);
    expect(info?.label).toBe('Download build 5');
    expect(info?.meta).toBe('Pre-release build 5 · 1.6 MB');
  });

  it('prefers HiyoDyna.exe over the portable zip', () => {
    const info = selectRelease([
      release('pre-5', true, [
        asset('HiyoDyna-pre-5-windows-x64.zip', 867371),
        executable(),
        asset('HiyoDyna.exe.sha256', 66),
      ]),
    ]);
    expect(info?.url).toBe('https://example.test/HiyoDyna.exe');
    expect(info?.fileName).toBe('HiyoDyna.exe');
    expect(info?.size).toBe(1719296);
  });

  it('ignores drafts and releases without an executable asset', () => {
    expect(selectRelease([{ ...release('pre-5', true), draft: true }])).toBeNull();
    expect(selectRelease([release('pre-5', true, [])])).toBeNull();
    expect(
      selectRelease([release('pre-5', true, [asset('HiyoDyna-pre-5-windows-x64.zip')])]),
    ).toBeNull();
  });

  it('formats sizes', () => {
    expect(formatBytes(867371)).toBe('847 KB');
    expect(formatBytes(1719296)).toBe('1.6 MB');
    expect(formatBytes(512)).toBe('512 B');
  });
});
