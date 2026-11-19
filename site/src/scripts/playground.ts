import {
  DEFAULT_SETTINGS,
  resetSettings,
  type Accent,
  type Bounce,
  type IslandSettings,
  type Motion,
} from '../lib/settings';
import { getIsland, initIslands } from './island';

const MOTIONS = new Set<Motion>(['slow', 'normal', 'fast']);
const BOUNCES = new Set<Bounce>(['low', 'medium', 'high']);
const ACCENTS = new Set<Accent>(['ice', 'amber', 'mint']);

export function applySetting(settings: IslandSettings, name: string, value: string): IslandSettings {
  switch (name) {
    case 'motion':
      return MOTIONS.has(value as Motion) ? { ...settings, motion: value as Motion } : settings;
    case 'bounce':
      return BOUNCES.has(value as Bounce) ? { ...settings, bounce: value as Bounce } : settings;
    case 'accent':
      return ACCENTS.has(value as Accent) ? { ...settings, accent: value as Accent } : settings;
    case 'hour12':
      return value === 'true' || value === 'false' ? { ...settings, hour12: value === 'true' } : settings;
    case 'showSeconds':
      return value === 'true' || value === 'false'
        ? { ...settings, showSeconds: value === 'true' }
        : settings;
    case 'shadow':
      return value === 'true' || value === 'false' ? { ...settings, shadow: value === 'true' } : settings;
    case 'gap': {
      const gap = Number(value);
      return gap === 0 || gap === 8 || gap === 16 ? { ...settings, gap } : settings;
    }
    default:
      return settings;
  }
}

export function initPlayground(): void {
  initIslands();

  const form = document.querySelector<HTMLFormElement>('[data-settings-form]');
  const island = getIsland('playground');
  if (!form || !island) return;

  let current: IslandSettings = { ...DEFAULT_SETTINGS };
  const note = document.querySelector<HTMLElement>('[data-reduced-note]');
  if (note && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches) {
    note.hidden = false;
  }

  const preview = document.querySelector<HTMLButtonElement>('[data-preview]');
  const islandRoot = document.querySelector<HTMLElement>('[data-island="playground"]');
  const syncPreview = () => {
    if (preview) {
      preview.textContent = island.isOpen() ? 'Collapse preview' : 'Preview motion';
    }
  };

  if (islandRoot && typeof MutationObserver !== 'undefined') {
    new MutationObserver(syncPreview).observe(islandRoot, {
      attributes: true,
      attributeFilter: ['data-expanded'],
    });
  }

  const startPreview = () => {
    island.previewMotion();
    syncPreview();
  };
  syncPreview();

  form.addEventListener('change', (event) => {
    const input = event.target as HTMLInputElement;
    if (!input.name) return;

    const value = input.type === 'checkbox' ? String(input.checked) : input.value;
    current = applySetting(current, input.name, value);
    island.setSettings(current);

    if (input.name === 'motion' || input.name === 'bounce') {
      startPreview();
    }
  });

  form.addEventListener('submit', (event) => event.preventDefault());

  document.querySelector<HTMLButtonElement>('[data-reset]')?.addEventListener('click', () => {
    form.reset();
    current = resetSettings();
    island.setSettings(current);
  });

  preview?.addEventListener('click', () => {
    if (island.isOpen()) {
      island.close();
    } else {
      startPreview();
    }
    syncPreview();
  });
}
