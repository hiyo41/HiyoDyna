export type Motion = 'slow' | 'normal' | 'fast';
export type Bounce = 'low' | 'medium' | 'high';
export type Accent = 'ice' | 'amber' | 'mint';
export type Dock = 'left' | 'center' | 'right';

export interface IslandSettings {
  motion: Motion;
  bounce: Bounce;
  accent: Accent;
  hour12: boolean;
  showSeconds: boolean;
  gap: 0 | 8 | 16;
  shadow: boolean;
  dock: Dock;
  compactLyrics: boolean;
}

export const DEFAULT_SETTINGS: IslandSettings = {
  motion: 'normal',
  bounce: 'medium',
  accent: 'ice',
  hour12: false,
  showSeconds: true,
  gap: 0,
  shadow: true,
  dock: 'center',
  compactLyrics: true,
};

export const ACCENTS: Record<Accent, string> = {
  ice: '#8FC7FF',
  amber: '#FFC478',
  mint: '#96F0C8',
};

export function resetSettings(): IslandSettings {
  return { ...DEFAULT_SETTINGS };
}
