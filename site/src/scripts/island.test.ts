// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { createIsland } from './island';
import { DEFAULT_SETTINGS } from '../lib/settings';

function island() {
  document.body.innerHTML =
    '<div data-island="test"><svg><path data-shape></path><path data-rim></path><path data-halo></path></svg><div class="island__text island__text--compact"></div><div class="island__text island__text--expanded"></div><button class="island__button" type="button"></button></div>';
  const root = document.querySelector<HTMLElement>('[data-island]')!;
  const button = root.querySelector<HTMLButtonElement>('.island__button')!;
  return { root, button };
}

describe('island interaction', () => {
  it('opens on Enter and exposes aria-expanded', () => {
    const { root, button } = island();
    const controller = createIsland(root, { settings: DEFAULT_SETTINGS });
    button.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    expect(button.getAttribute('aria-expanded')).toBe('true');
    expect(root.dataset.expanded).toBe('true');
    controller.destroy();
  });

  it('toggles on tap', () => {
    const { root, button } = island();
    const controller = createIsland(root, { settings: DEFAULT_SETTINGS });
    button.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(button.getAttribute('aria-expanded')).toBe('true');
    controller.destroy();
  });

  it('collapses on Escape and reopens on Enter', () => {
    const { root, button } = island();
    const controller = createIsland(root, { settings: DEFAULT_SETTINGS });
    button.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    button.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(button.getAttribute('aria-expanded')).toBe('false');
    expect(root.dataset.expanded).toBe('false');
    button.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    expect(button.getAttribute('aria-expanded')).toBe('true');
    controller.destroy();
  });

  it('opens immediately under reduced motion', () => {
    const { root, button } = island();
    const controller = createIsland(root, { settings: DEFAULT_SETTINGS, reducedMotion: true });
    button.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    expect(root.dataset.expanded).toBe('true');
    controller.destroy();
  });

  it('starts a one-shot motion preview under reduced motion', () => {
    const { root, button } = island();
    const controller = createIsland(root, { settings: DEFAULT_SETTINGS, reducedMotion: true });
    controller.previewMotion();
    expect(button.getAttribute('aria-expanded')).toBe('true');
    expect(root.classList.contains('is-previewing')).toBe(true);
    controller.destroy();
  });
});
