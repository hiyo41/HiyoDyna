import { describe, expect, it } from 'vitest';
import { COMPACT, EXPANDED } from './superellipse';
import { ShapeSpring, tuningFor } from './spring';

function run(spring: ShapeSpring, seconds: number, dt = 1 / 60) {
  let peak = 0;
  for (let time = 0; time < seconds; time += dt) {
    spring.advance(dt);
    peak = Math.max(peak, spring.state.width);
  }
  return peak;
}

describe('ShapeSpring', () => {
  it('overshoots then settles at the expanded panel', () => {
    const spring = new ShapeSpring(COMPACT, tuningFor('normal', 'medium'));
    spring.setTarget(EXPANDED);
    const peak = run(spring, 1);
    expect(peak).toBeGreaterThan(EXPANDED.width);
    expect(spring.state.width).toBeCloseTo(EXPANDED.width, 1);
    expect(spring.advance(1 / 60)).toBe(false);
  });

  it('never lets the radius exceed half the height', () => {
    const spring = new ShapeSpring(COMPACT, tuningFor('normal', 'high'));
    spring.setTarget(EXPANDED);
    run(spring, 2);
    expect(spring.state.radius).toBeLessThanOrEqual(spring.state.height / 2);
  });

  it('maps presets to the app values', () => {
    expect(tuningFor('slow', 'low')).toMatchObject({
      expandResponse: 0.38,
      expandDamping: 0.78,
    });
    expect(tuningFor('fast', 'high')).toMatchObject({
      collapseResponse: 0.24,
      collapseDamping: 0.64,
    });
  });
});
