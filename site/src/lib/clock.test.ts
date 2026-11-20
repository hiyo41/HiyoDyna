import { describe, expect, it } from 'vitest';
import { clockSnapshot } from './clock';

const at = (hour: number, minute: number, second = 0) =>
  new Date(2026, 10, 19, hour, minute, second);

describe('clockSnapshot', () => {
  it('formats 24-hour time with a leading zero', () => {
    expect(clockSnapshot(at(9, 5, 7), false)).toMatchObject({
      time: '09:05',
      seconds: '07',
      meridiem: '',
    });
  });

  it('formats 12-hour time without a leading hour zero', () => {
    expect(clockSnapshot(at(13, 4), true)).toMatchObject({ time: '1:04', meridiem: 'PM' });
  });

  it('maps midnight and noon to 12', () => {
    expect(clockSnapshot(at(0, 0), true)).toMatchObject({ time: '12:00', meridiem: 'AM' });
    expect(clockSnapshot(at(12, 0), true)).toMatchObject({ time: '12:00', meridiem: 'PM' });
  });

  it('formats the app date line', () => {
    expect(clockSnapshot(at(14, 3), false).date).toBe('Thursday · November 19');
  });
});
