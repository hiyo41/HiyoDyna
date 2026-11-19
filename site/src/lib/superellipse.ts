export interface ShapeState {
  width: number;
  height: number;
  radius: number;
}

export const SHAPE_BOX = { width: 420, height: 150 } as const;
export const COMPACT: ShapeState = { width: 184, height: 54, radius: 27 };
export const EXPANDED: ShapeState = { width: 420, height: 150, radius: 30 };

const CORNER_SEGMENTS = 16;
const EXPONENT = 2 / 3;

export function cornerOffset(radius: number, angle: number): { x: number; y: number } {
  return {
    x: radius * Math.pow(Math.cos(angle), EXPONENT),
    y: radius * Math.pow(Math.sin(angle), EXPONENT),
  };
}

export function superellipsePath(state: ShapeState): string {
  const cx = SHAPE_BOX.width / 2;
  const cy = SHAPE_BOX.height / 2;
  const halfWidth = Math.max(0, state.width) / 2;
  const halfHeight = Math.max(0, state.height) / 2;
  const radius = Math.max(0, Math.min(state.radius, halfWidth, halfHeight));
  const innerWidth = halfWidth - radius;
  const innerHeight = halfHeight - radius;

  const points: Array<[number, number]> = [[cx - innerWidth, cy - halfHeight]];

  points.push([cx + innerWidth, cy - halfHeight]);

  for (let i = 1; i <= CORNER_SEGMENTS; i += 1) {
    const offset = cornerOffset(radius, (i / CORNER_SEGMENTS) * (Math.PI / 2));
    points.push([cx + innerWidth + offset.y, cy - innerHeight - offset.x]);
  }

  points.push([cx + halfWidth, cy + innerHeight]);

  for (let i = 1; i <= CORNER_SEGMENTS; i += 1) {
    const offset = cornerOffset(radius, (i / CORNER_SEGMENTS) * (Math.PI / 2));
    points.push([cx + innerWidth + offset.x, cy + innerHeight + offset.y]);
  }

  points.push([cx - innerWidth, cy + halfHeight]);

  for (let i = 1; i <= CORNER_SEGMENTS; i += 1) {
    const offset = cornerOffset(radius, (i / CORNER_SEGMENTS) * (Math.PI / 2));
    points.push([cx - innerWidth - offset.y, cy + innerHeight + offset.x]);
  }

  points.push([cx - halfWidth, cy - innerHeight]);

  for (let i = 1; i <= CORNER_SEGMENTS; i += 1) {
    const offset = cornerOffset(radius, (i / CORNER_SEGMENTS) * (Math.PI / 2));
    points.push([cx - innerWidth - offset.x, cy - innerHeight - offset.y]);
  }

  const format = (value: number) => String(Number(value.toFixed(2)));
  const [first, ...rest] = points;

  return `M${format(first[0])} ${format(first[1])}${rest
    .map(([x, y]) => `L${format(x)} ${format(y)}`)
    .join('')}Z`;
}
