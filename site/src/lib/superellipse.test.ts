import { describe, expect, it } from 'vitest';
import { COMPACT, EXPANDED, SHAPE_BOX, cornerOffset, superellipsePath } from './superellipse';

describe('superellipse', () => {
  it('returns a closed path centered in the shape box', () => {
    const d = superellipsePath(COMPACT);
    expect(d.startsWith('M')).toBe(true);
    expect(d.trim().endsWith('Z')).toBe(true);
    expect(d).not.toContain('NaN');
  });

  it('keeps the compact capsule inside the expanded box', () => {
    const d = superellipsePath(COMPACT);
    expect(SHAPE_BOX.width - COMPACT.width).toBe(236);
    expect(SHAPE_BOX.height - COMPACT.height).toBe(96);
    expect(d).toContain('118');
    expect(d).toContain('302');
  });

  it('makes corners fuller than a circle', () => {
    const superellipse = cornerOffset(EXPANDED.radius, Math.PI / 4).x;
    const circle = EXPANDED.radius / Math.SQRT2;
    expect(superellipse).toBeGreaterThan(circle);
  });
});
