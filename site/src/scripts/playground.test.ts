import { describe, expect, it } from 'vitest';
import { DEFAULT_SETTINGS } from '../lib/settings';
import { applySetting } from './playground';

describe('applySetting', () => {
  it('applies every control', () => {
    let settings = applySetting(DEFAULT_SETTINGS, 'motion', 'fast');
    settings = applySetting(settings, 'bounce', 'high');
    settings = applySetting(settings, 'accent', 'mint');
    settings = applySetting(settings, 'hour12', 'true');
    settings = applySetting(settings, 'showSeconds', 'false');
    settings = applySetting(settings, 'gap', '16');
    settings = applySetting(settings, 'shadow', 'false');

    expect(settings).toEqual({
      motion: 'fast',
      bounce: 'high',
      accent: 'mint',
      hour12: true,
      showSeconds: false,
      gap: 16,
      shadow: false,
      dock: 'center',
      compactLyrics: true,
    });
  });

  it('ignores unknown and out-of-range values', () => {
    expect(applySetting(DEFAULT_SETTINGS, 'motion', 'turbo')).toEqual(DEFAULT_SETTINGS);
    expect(applySetting(DEFAULT_SETTINGS, 'gap', '12')).toEqual(DEFAULT_SETTINGS);
    expect(applySetting(DEFAULT_SETTINGS, 'nonsense', '1')).toEqual(DEFAULT_SETTINGS);
  });
});
