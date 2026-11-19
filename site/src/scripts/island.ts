import { clockSnapshot } from '../lib/clock';
import { ACCENTS, DEFAULT_SETTINGS, type IslandSettings } from '../lib/settings';
import { ShapeSpring, tuningFor } from '../lib/spring';
import { COMPACT, EXPANDED, superellipsePath, type ShapeState } from '../lib/superellipse';

interface IslandOptions {
  settings: IslandSettings;
  reducedMotion?: boolean;
}

export interface IslandController {
  open(): void;
  close(): void;
  previewMotion(): void;
  setSettings(next: IslandSettings): void;
  destroy(): void;
}

const live = new Set<Island>();
const byId = new Map<string, IslandController>();
let frame = 0;
let lastFrame = 0;

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

const smoothstep = (edge0: number, edge1: number, value: number) => {
  const t = clamp((value - edge0) / (edge1 - edge0), 0, 1);
  return t * t * (3 - 2 * t);
};

class Island implements IslandController {
  private root: HTMLElement;
  private control: HTMLButtonElement;
  private settings: IslandSettings;
  private reducedMotion: boolean;
  private forceMotion = false;
  private spring: ShapeSpring;
  private target: ShapeState = COMPACT;
  private display: ShapeState = COMPACT;
  private progress = 0;
  private pinned = false;
  private dismissed = false;
  private pointerInside = false;
  private focused = false;
  private closeTimer = 0;
  private clockTimer = 0;
  private resizeObserver?: ResizeObserver;
  private dirty = true;
  private destroyed = false;

  constructor(root: HTMLElement, options: IslandOptions) {
    this.root = root;
    const control = root.querySelector<HTMLButtonElement>('.island__button');
    if (!control) throw new Error('Island is missing its button');
    this.control = control;
    this.settings = options.settings;
    this.reducedMotion = options.reducedMotion ?? prefersReducedMotion();
    this.spring = new ShapeSpring(COMPACT, tuningFor(this.settings.motion, this.settings.bounce));

    this.root.dataset.expanded = 'false';
    this.control.setAttribute('aria-expanded', 'false');
    this.control.addEventListener('pointerenter', this.onPointerEnter);
    this.control.addEventListener('pointerleave', this.onPointerLeave);
    this.control.addEventListener('focus', this.onFocus);
    this.control.addEventListener('blur', this.onBlur);
    this.control.addEventListener('click', this.onClick);
    this.control.addEventListener('keydown', this.onKeyDown);

    this.observeSize();
    this.applySettings();
    this.setOpen(false);

    this.clockTimer = window.setInterval(this.updateClock, 1000);
    this.updateClock();
    live.add(this);
  }

  open(): void {
    this.pinned = true;
    this.dismissed = false;
    this.setOpen(true);
  }

  close(): void {
    this.pinned = false;
    this.forceMotion = false;
    this.root.classList.remove('is-previewing');
    this.setOpen(false);
  }

  previewMotion(): void {
    this.pinned = true;
    this.dismissed = false;
    this.forceMotion = this.reducedMotion;
    this.root.classList.add('is-previewing');
    this.display = { ...COMPACT };
    this.progress = 0;
    this.spring = new ShapeSpring(COMPACT, tuningFor(this.settings.motion, this.settings.bounce));
    this.setOpen(true);
  }

  setSettings(next: IslandSettings): void {
    this.settings = next;
    this.spring.setTuning(tuningFor(next.motion, next.bounce));
    this.applySettings();
    this.updateClock();
    this.dirty = true;
    this.render();
  }

  destroy(): void {
    this.destroyed = true;
    window.clearInterval(this.clockTimer);
    window.clearTimeout(this.closeTimer);
    this.resizeObserver?.disconnect();
    this.control.removeEventListener('pointerenter', this.onPointerEnter);
    this.control.removeEventListener('pointerleave', this.onPointerLeave);
    this.control.removeEventListener('focus', this.onFocus);
    this.control.removeEventListener('blur', this.onBlur);
    this.control.removeEventListener('click', this.onClick);
    this.control.removeEventListener('keydown', this.onKeyDown);
    live.delete(this);
    this.root.removeAttribute('data-ready');
  }

  tick(dtSeconds: number): boolean {
    if (this.destroyed) return false;
    if (this.reducedMotion && !this.forceMotion) {
      if (!this.dirty) return false;
      this.render();
      this.dirty = false;
      return false;
    }

    const active = this.spring.advance(dtSeconds);
    this.display = this.spring.state;
    this.progress = clamp((this.display.height - COMPACT.height) / (EXPANDED.height - COMPACT.height), 0, 1);

    if (active || this.dirty) {
      this.render();
      this.dirty = false;
    }

    if (!active && this.forceMotion) {
      this.forceMotion = false;
      this.root.classList.remove('is-previewing');
    }

    return active;
  }

  private setOpen(open: boolean): void {
    this.target = open ? EXPANDED : COMPACT;
    this.root.dataset.expanded = String(open);
    this.control.setAttribute('aria-expanded', String(open));
    this.setLayerAccessibility(open);

    if (this.reducedMotion && !this.forceMotion) {
      this.display = { ...this.target };
      this.progress = open ? 1 : 0;
      this.dirty = true;
      this.render();
      this.dirty = false;
      return;
    }

    this.spring.setTarget(this.target);
    this.dirty = true;
    startLoop();
  }

  private setLayerAccessibility(expanded: boolean): void {
    this.root
      .querySelector('.island__text--compact')
      ?.setAttribute('aria-hidden', String(expanded));
    this.root
      .querySelector('.island__text--expanded')
      ?.setAttribute('aria-hidden', String(!expanded));
  }

  private render(): void {
    const d = superellipsePath(this.display);
    this.root.querySelectorAll<SVGPathElement>('[data-shape], [data-rim], [data-halo]').forEach((path) => {
      path.setAttribute('d', d);
    });

    const compactOpacity = this.reducedMotion
      ? Number(this.progress < 0.5)
      : 1 - smoothstep(0.1, 0.42, this.progress);
    const expandedOpacity = this.reducedMotion
      ? Number(this.progress >= 0.5)
      : smoothstep(0.25, 0.7, this.progress);

    this.root.style.setProperty('--p', this.progress.toFixed(3));
    this.root.style.setProperty('--compact-opacity', compactOpacity.toFixed(3));
    this.root.style.setProperty('--expanded-opacity', expandedOpacity.toFixed(3));
    setLayerVisibility(this.root, '.island__text--compact', compactOpacity);
    setLayerVisibility(this.root, '.island__text--expanded', expandedOpacity);
  }

  private applySettings(): void {
    this.root.style.setProperty('--island-accent', ACCENTS[this.settings.accent]);
    this.root.style.setProperty('--island-gap', `${this.settings.gap}px`);
    this.root.dataset.shadow = this.settings.shadow ? 'soft' : 'off';
    this.root.dataset.seconds = this.settings.showSeconds ? 'on' : 'off';
    this.root.dataset.hour12 = this.settings.hour12 ? 'true' : 'false';
  }

  private updateClock = (): void => {
    if (document.hidden) return;
    const snapshot = clockSnapshot(new Date(), this.settings.hour12);

    setText(this.root, '[data-time]', snapshot.time);
    setText(this.root, '[data-time-compact]', snapshot.time);
    setText(this.root, '[data-seconds]', snapshot.seconds);
    setText(this.root, '[data-seconds-compact]', snapshot.seconds);
    setText(this.root, '[data-date]', snapshot.date);
    setText(this.root, '[data-meridiem]', snapshot.meridiem);
    setText(this.root, '[data-meridiem-compact]', snapshot.meridiem);
  };

  private observeSize(): void {
    const shell = this.root.parentElement;
    if (!shell) return;

    const resize = () => {
      const width = shell.getBoundingClientRect().width || 460;
      this.root.style.setProperty('--island-scale', String(clamp(width / 460, 0.4, 1)));
    };

    resize();
    if (typeof ResizeObserver !== 'undefined') {
      this.resizeObserver = new ResizeObserver(resize);
      this.resizeObserver.observe(shell);
    } else {
      window.addEventListener('resize', resize);
    }
  }

  private onPointerEnter = (): void => {
    this.pointerInside = true;
    window.clearTimeout(this.closeTimer);
    this.setOpen(true);
  };

  private onPointerLeave = (): void => {
    this.pointerInside = false;
    this.scheduleClose();
  };

  private onFocus = (): void => {
    this.focused = true;
    window.clearTimeout(this.closeTimer);
    if (!this.dismissed) this.setOpen(true);
  };

  private onBlur = (): void => {
    this.focused = false;
    this.dismissed = false;
    this.scheduleClose();
  };

  private togglePinned = (): void => {
    if (this.dismissed) {
      this.dismissed = false;
      this.pinned = true;
      this.setOpen(true);
      return;
    }

    this.pinned = !this.pinned;
    this.setOpen(this.pinned || this.pointerInside || this.focused);
  };

  private onClick = (): void => {
    this.togglePinned();
  };

  private onKeyDown = (event: KeyboardEvent): void => {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      this.togglePinned();
      return;
    }

    if (event.key === 'Escape') {
      this.pinned = false;
      this.dismissed = true;
      this.forceMotion = false;
      this.root.classList.remove('is-previewing');
      this.setOpen(false);
    }
  };

  private scheduleClose(): void {
    if (this.pinned) return;
    window.clearTimeout(this.closeTimer);
    this.closeTimer = window.setTimeout(() => {
      if (!this.pointerInside && !this.focused && !this.pinned) {
        this.setOpen(false);
      }
    }, 160);
  }
}

function setText(root: HTMLElement, selector: string, text: string): void {
  root.querySelectorAll<HTMLElement>(selector).forEach((element) => {
    element.textContent = text;
  });
}

function setLayerVisibility(root: HTMLElement, selector: string, opacity: number): void {
  const layer = root.querySelector<HTMLElement>(selector);
  if (!layer) return;

  layer.style.visibility = opacity > 0.01 ? 'visible' : 'hidden';
}

function prefersReducedMotion(): boolean {
  return typeof window !== 'undefined' && window.matchMedia
    ? window.matchMedia('(prefers-reduced-motion: reduce)').matches
    : false;
}

function startLoop(): void {
  if (frame || typeof requestAnimationFrame === 'undefined') return;
  lastFrame = performance.now();
  frame = requestAnimationFrame(tick);
}

function tick(now: number): void {
  frame = 0;
  const dt = Math.min((now - lastFrame) / 1000, 0.1);
  lastFrame = now;

  let active = false;
  for (const island of live) {
    active = island.tick(dt) || active;
  }

  if (active) {
    startLoop();
  }
}

export function createIsland(root: HTMLElement, options: IslandOptions): IslandController {
  return new Island(root, options);
}

export function initIslands(): void {
  document.querySelectorAll<HTMLElement>('[data-island]').forEach((root) => {
    if (root.dataset.ready === 'true') return;

    const id = root.dataset.island ?? `island-${byId.size}`;
    const settings = parseSettings(root.dataset.settings);
    const controller = createIsland(root, {
      settings,
      reducedMotion: prefersReducedMotion(),
    });

    root.dataset.ready = 'true';
    root.dataset.islandId = id;
    byId.set(id, controller);
  });
}

export function getIsland(id: string): IslandController | undefined {
  return byId.get(id);
}

function parseSettings(raw: string | undefined): IslandSettings {
  if (!raw) return { ...DEFAULT_SETTINGS };

  try {
    return { ...DEFAULT_SETTINGS, ...(JSON.parse(raw) as Partial<IslandSettings>) };
  } catch {
    return { ...DEFAULT_SETTINGS };
  }
}

if (typeof document !== 'undefined') {
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
      if (frame) cancelAnimationFrame(frame);
      frame = 0;
      return;
    }

    lastFrame = performance.now();
    startLoop();
  });
}
