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

const MOTIONS: Motion[] = ['slow', 'normal', 'fast'];
const BOUNCES: Bounce[] = ['low', 'medium', 'high'];
const ACCENT_KEYS: Accent[] = ['ice', 'amber', 'mint'];
const DOCKS: Dock[] = ['left', 'center', 'right'];

/**
 * Coerces arbitrary parsed data (JSON embedded in markup, stored state) into a
 * safe settings object: every key is validated and unknown or invalid values
 * fall back to the defaults instead of producing undefined CSS.
 */
export function normalizeSettings(value: Partial<IslandSettings> | null | undefined): IslandSettings {
  const raw = value ?? {};
  return {
    motion: MOTIONS.includes(raw.motion as Motion) ? (raw.motion as Motion) : DEFAULT_SETTINGS.motion,
    bounce: BOUNCES.includes(raw.bounce as Bounce) ? (raw.bounce as Bounce) : DEFAULT_SETTINGS.bounce,
    accent: ACCENT_KEYS.includes(raw.accent as Accent) ? (raw.accent as Accent) : DEFAULT_SETTINGS.accent,
    hour12: typeof raw.hour12 === 'boolean' ? raw.hour12 : DEFAULT_SETTINGS.hour12,
    showSeconds: typeof raw.showSeconds === 'boolean' ? raw.showSeconds : DEFAULT_SETTINGS.showSeconds,
    gap: raw.gap === 0 || raw.gap === 8 || raw.gap === 16 ? raw.gap : DEFAULT_SETTINGS.gap,
    shadow: typeof raw.shadow === 'boolean' ? raw.shadow : DEFAULT_SETTINGS.shadow,
    dock: DOCKS.includes(raw.dock as Dock) ? (raw.dock as Dock) : DEFAULT_SETTINGS.dock,
    compactLyrics:
      typeof raw.compactLyrics === 'boolean' ? raw.compactLyrics : DEFAULT_SETTINGS.compactLyrics,
  };
}

export function resetSettings(): IslandSettings {
  return { ...DEFAULT_SETTINGS };
}
