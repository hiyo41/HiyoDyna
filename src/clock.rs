use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClockFormat {
    TwentyFourHour,
    TwelveHour,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClockSnapshot {
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub weekday: u16,
    pub month: u16,
    pub day: u16,
}

impl ClockSnapshot {
    pub fn now() -> Self {
        let mut time = SYSTEMTIME::default();
        // SAFETY: time is a valid writable SYSTEMTIME.
        unsafe { GetLocalTime(&mut time) };
        Self {
            hour: time.wHour,
            minute: time.wMinute,
            second: time.wSecond,
            weekday: time.wDayOfWeek,
            month: time.wMonth,
            day: time.wDay,
        }
    }

    /// The clock digits, without the meridiem suffix.
    pub fn time_text(&self, format: ClockFormat) -> String {
        match format {
            ClockFormat::TwentyFourHour => format!("{:02}:{:02}", self.hour, self.minute),
            ClockFormat::TwelveHour => {
                let hour = match self.hour % 12 {
                    0 => 12,
                    hour => hour,
                };
                format!("{hour}:{:02}", self.minute)
            }
        }
    }

    pub fn meridiem(&self, format: ClockFormat) -> Option<&'static str> {
        match format {
            ClockFormat::TwentyFourHour => None,
            ClockFormat::TwelveHour => Some(if self.hour < 12 { "AM" } else { "PM" }),
        }
    }

    pub fn seconds_text(&self) -> String {
        format!("{:02}", self.second.min(59))
    }

    pub fn date_text(&self) -> String {
        let weekday = WEEKDAYS[self.weekday as usize % WEEKDAYS.len()];
        let month = MONTHS[(self.month.clamp(1, 12) - 1) as usize];
        format!("{weekday} · {month} {}", self.day)
    }
}

#[cfg(test)]
mod tests {
    use super::{ClockFormat, ClockSnapshot};

    fn snapshot(hour: u16, minute: u16, weekday: u16, month: u16, day: u16) -> ClockSnapshot {
        ClockSnapshot {
            hour,
            minute,
            second: 0,
            weekday,
            month,
            day,
        }
    }

    fn snapshot_with_second(
        hour: u16,
        minute: u16,
        second: u16,
        weekday: u16,
        month: u16,
        day: u16,
    ) -> ClockSnapshot {
        ClockSnapshot {
            hour,
            minute,
            second,
            weekday,
            month,
            day,
        }
    }

    #[test]
    fn formats_twenty_four_hour_time() {
        assert_eq!(
            snapshot(0, 5, 0, 11, 18).time_text(ClockFormat::TwentyFourHour),
            "00:05"
        );
        assert_eq!(
            snapshot(21, 41, 0, 11, 18).time_text(ClockFormat::TwentyFourHour),
            "21:41"
        );
        assert_eq!(
            snapshot(21, 41, 0, 11, 18).meridiem(ClockFormat::TwentyFourHour),
            None
        );
    }

    #[test]
    fn formats_twelve_hour_time_and_meridiem() {
        let midnight = snapshot(0, 7, 0, 11, 18);
        let noon = snapshot(12, 0, 0, 11, 18);
        let evening = snapshot(21, 41, 0, 11, 18);
        let morning = snapshot(9, 5, 0, 11, 18);

        assert_eq!(midnight.time_text(ClockFormat::TwelveHour), "12:07");
        assert_eq!(midnight.meridiem(ClockFormat::TwelveHour), Some("AM"));
        assert_eq!(noon.time_text(ClockFormat::TwelveHour), "12:00");
        assert_eq!(noon.meridiem(ClockFormat::TwelveHour), Some("PM"));
        assert_eq!(evening.time_text(ClockFormat::TwelveHour), "9:41");
        assert_eq!(evening.meridiem(ClockFormat::TwelveHour), Some("PM"));
        assert_eq!(morning.time_text(ClockFormat::TwelveHour), "9:05");
        assert_eq!(morning.meridiem(ClockFormat::TwelveHour), Some("AM"));
    }

    #[test]
    fn formats_a_modern_date_line() {
        assert_eq!(
            snapshot(9, 41, 3, 11, 18).date_text(),
            "Wednesday · November 18"
        );
        assert_eq!(snapshot(9, 41, 1, 1, 1).date_text(), "Monday · January 1");
    }

    #[test]
    fn clamps_impossible_month_values() {
        assert_eq!(
            snapshot(9, 41, 9, 99, 18).date_text(),
            "Tuesday · December 18"
        );
    }

    #[test]
    fn formats_seconds_with_two_digits() {
        let midnight = snapshot_with_second(0, 0, 0, 0, 11, 18);
        let nine = snapshot_with_second(0, 0, 9, 0, 11, 18);
        let last = snapshot_with_second(0, 0, 59, 0, 11, 18);

        assert_eq!(midnight.seconds_text(), "00");
        assert_eq!(nine.seconds_text(), "09");
        assert_eq!(last.seconds_text(), "59");
    }

    #[test]
    fn snapshots_differ_when_only_the_second_changes() {
        let first = snapshot_with_second(21, 41, 17, 3, 11, 18);
        let next = snapshot_with_second(21, 41, 18, 3, 11, 18);

        assert_ne!(first, next);
        assert_eq!(first.time_text(ClockFormat::TwentyFourHour), "21:41");
        assert_eq!(next.time_text(ClockFormat::TwentyFourHour), "21:41");
    }
}
