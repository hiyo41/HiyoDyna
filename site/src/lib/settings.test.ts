import { describe, expect, it } from 'vitest';
import { ACCENTS, DEFAULT_SETTINGS, resetSettings } from './settings';

describe('settings', () => {
  it('starts from the app defaults', () => {
    expect(DEFAULT_SETTINGS).toEqual({
      motion: 'normal',
      bounce: 'medium',
      accent: 'ice',
      hour12: false,
      showSeconds: true,
      gap: 0,
      shadow: true,
      dock: 'center',
      compactLyrics: true,
    });
  });

  it('returns a fresh default object on reset', () => {
    const reset = resetSettings();
    expect(reset).toEqual(DEFAULT_SETTINGS);
    expect(reset).not.toBe(DEFAULT_SETTINGS);
  });

  it('restores dock and compact lyrics defaults on reset', () => {
    const changed = { ...DEFAULT_SETTINGS, dock: 'right' as const, compactLyrics: false };
    const reset = resetSettings();
    expect(changed.dock).toBe('right');
    expect(changed.compactLyrics).toBe(false);
    expect(reset.dock).toBe('center');
    expect(reset.compactLyrics).toBe(true);
  });

  it('maps the app accents', () => {
    expect(ACCENTS).toEqual({
      ice: '#8FC7FF',
      amber: '#FFC478',
      mint: '#96F0C8',
    });
  });
});
