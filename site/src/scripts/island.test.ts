// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { createIsland } from './island';
import { DEFAULT_SETTINGS } from '../lib/settings';

function island() {
  document.body.innerHTML =
    '<div data-island="test" role="button" tabindex="0" aria-expanded="false"><svg><path data-shape></path><path data-rim></path><path data-halo></path></svg><span data-compact></span><span data-expanded></span></div>';
  return document.querySelector<HTMLElement>('[data-island]')!;
}

describe('island interaction', () => {
  it('opens on Enter and exposes aria-expanded', () => {
    const root = island();
    const controller = createIsland(root, { settings: DEFAULT_SETTINGS });
    root.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    expect(root.getAttribute('aria-expanded')).toBe('true');
    expect(root.dataset.expanded).toBe('true');
    controller.destroy();
  });

  it('toggles on tap', () => {
    const root = island();
    const controller = createIsland(root, { settings: DEFAULT_SETTINGS });
    root.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(root.getAttribute('aria-expanded')).toBe('true');
    controller.destroy();
  });

  it('opens immediately under reduced motion', () => {
    const root = island();
    const controller = createIsland(root, { settings: DEFAULT_SETTINGS, reducedMotion: true });
    root.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    expect(root.dataset.expanded).toBe('true');
    controller.destroy();
  });
});
