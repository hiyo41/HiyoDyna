import { COMPACT, EXPANDED, type ShapeState } from './superellipse';
import type { Bounce, Motion } from './settings';

export interface SpringTuning {
  expandResponse: number;
  expandDamping: number;
  collapseResponse: number;
  collapseDamping: number;
}

const RESPONSE: Record<Motion, { expand: number; collapse: number }> = {
  slow: { expand: 0.38, collapse: 0.42 },
  normal: { expand: 0.30, collapse: 0.32 },
  fast: { expand: 0.22, collapse: 0.24 },
};

const DAMPING: Record<Bounce, { expand: number; collapse: number }> = {
  low: { expand: 0.78, collapse: 0.85 },
  medium: { expand: 0.62, collapse: 0.76 },
  high: { expand: 0.50, collapse: 0.64 },
};

export function tuningFor(motion: Motion, bounce: Bounce): SpringTuning {
  return {
    expandResponse: RESPONSE[motion].expand,
    expandDamping: DAMPING[bounce].expand,
    collapseResponse: RESPONSE[motion].collapse,
    collapseDamping: DAMPING[bounce].collapse,
  };
}

const OVERSHOOT_MIN = 0.92;
const OVERSHOOT_MAX = 1.08;
const MAX_FRAME_SECONDS = 0.1;
const SUBSTEP_SECONDS = 1 / 240;
const POSITION_EPSILON = 0.03;
const VELOCITY_EPSILON = 0.05;

export class ShapeSpring {
  state: ShapeState;
  private target: ShapeState = COMPACT;
  private velocity = { width: 0, height: 0, radius: 0 };
  private tuning: SpringTuning;

  constructor(initial: ShapeState = COMPACT, tuning: SpringTuning = tuningFor('normal', 'medium')) {
    this.state = { ...initial };
    this.tuning = tuning;
  }

  setTarget(target: ShapeState): void {
    this.target = { ...target };
  }

  setTuning(tuning: SpringTuning): void {
    this.tuning = tuning;
  }

  advance(dtSeconds: number): boolean {
    if (!Number.isFinite(dtSeconds) || dtSeconds <= 0) {
      return !this.isSettled();
    }

    // Match the app: the tuning pair is chosen by the height axis.
    const expanding = this.target.height >= EXPANDED.height;
    const response = expanding ? this.tuning.expandResponse : this.tuning.collapseResponse;
    const damping = expanding ? this.tuning.expandDamping : this.tuning.collapseDamping;
    const omega = (2 * Math.PI) / response;
    // Collapsing to the compact capsule must approach its destination without
    // dipping below the compact baseline or rebounding, exactly like the
    // native spring in animation.rs.
    const collapseHeight = this.target.height === COMPACT.height;

    let remaining = Math.min(dtSeconds, MAX_FRAME_SECONDS);
    while (remaining > 0) {
      const step = Math.min(remaining, SUBSTEP_SECONDS);
      this.integrateAxis('width', step, omega, damping, false);
      this.integrateAxis('height', step, omega, damping, collapseHeight);
      this.integrateAxis('radius', step, omega, damping, false);
      remaining -= step;
    }

    if (this.isSettled()) {
      this.state = { ...this.target };
      this.velocity = { width: 0, height: 0, radius: 0 };
      return false;
    }

    return true;
  }

  private integrateAxis(
    axis: keyof ShapeState,
    dt: number,
    omega: number,
    damping: number,
    monotonic: boolean,
  ): void {
    const target = this.target[axis];
    const previous = this.state[axis];
    const min = COMPACT[axis] * OVERSHOOT_MIN;
    const max = EXPANDED[axis] * OVERSHOOT_MAX;
    const effectiveDamping = monotonic ? Math.max(damping, 1) : damping;
    const acceleration =
      -2 * effectiveDamping * omega * this.velocity[axis] - omega * omega * (previous - target);

    this.velocity[axis] += acceleration * dt;
    this.state[axis] += this.velocity[axis] * dt;

    if (monotonic && previous >= target) {
      const clamped = Math.min(Math.max(this.state[axis], target), previous);
      if (this.state[axis] !== clamped) {
        this.state[axis] = clamped;
        this.velocity[axis] = 0;
      }
    }

    if (this.state[axis] < min) {
      this.state[axis] = min;
      if (this.velocity[axis] < 0) this.velocity[axis] = 0;
    } else if (this.state[axis] > max) {
      this.state[axis] = max;
      if (this.velocity[axis] > 0) this.velocity[axis] = 0;
    }

    if (axis === 'radius') {
      const maxRadius = Math.min(this.state.height / 2, max);
      if (this.state.radius > maxRadius) {
        this.state.radius = maxRadius;
        if (this.velocity.radius > 0) this.velocity.radius = 0;
      }
    }
  }

  private isSettled(): boolean {
    return (
      Math.abs(this.state.width - this.target.width) < POSITION_EPSILON &&
      Math.abs(this.state.height - this.target.height) < POSITION_EPSILON &&
      Math.abs(this.state.radius - this.target.radius) < POSITION_EPSILON &&
      Math.abs(this.velocity.width) < VELOCITY_EPSILON &&
      Math.abs(this.velocity.height) < VELOCITY_EPSILON &&
      Math.abs(this.velocity.radius) < VELOCITY_EPSILON
    );
  }
}
