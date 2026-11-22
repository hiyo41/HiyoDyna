const WEEKDAYS = [
  'Sunday',
  'Monday',
  'Tuesday',
  'Wednesday',
  'Thursday',
  'Friday',
  'Saturday',
] as const;

const MONTHS = [
  'January',
  'February',
  'March',
  'April',
  'May',
  'June',
  'July',
  'August',
  'September',
  'October',
  'November',
  'December',
] as const;

export interface ClockSnapshot {
  time: string;
  seconds: string;
  meridiem: string;
  date: string;
}

export function clockSnapshot(date: Date, hour12: boolean): ClockSnapshot {
  const hours24 = date.getHours();
  const minutes = String(date.getMinutes()).padStart(2, '0');
  const hours = hour12
    ? String(hours24 % 12 === 0 ? 12 : hours24 % 12)
    : String(hours24).padStart(2, '0');

  return {
    time: `${hours}:${minutes}`,
    seconds: String(Math.min(date.getSeconds(), 59)).padStart(2, '0'),
    meridiem: hour12 ? (hours24 < 12 ? 'AM' : 'PM') : '',
    date: `${WEEKDAYS[date.getDay()]} · ${MONTHS[date.getMonth()]} ${date.getDate()}`,
  };
}
