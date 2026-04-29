// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TimeOfDay {
    seconds_since_midnight: u64,
}

impl TimeOfDay {
    #[must_use]
    pub const fn from_hm(hour: u64, minute: u64) -> Self {
        Self {
            seconds_since_midnight: hour * 3600 + minute * 60,
        }
    }

    #[must_use]
    pub const fn seconds_since_midnight(self) -> u64 {
        self.seconds_since_midnight
    }

    #[must_use]
    pub fn add_ticks(self, ticks: u64) -> Self {
        let seconds = (self.seconds_since_midnight + ticks) % (24 * 3600);
        Self {
            seconds_since_midnight: seconds,
        }
    }

    #[must_use]
    pub fn round_to_nearest_quarter_hour(self) -> Self {
        let quarter_hour = 15 * 60;
        let rounded =
            ((self.seconds_since_midnight + quarter_hour / 2) / quarter_hour) * quarter_hour;
        Self {
            seconds_since_midnight: rounded % (24 * 3600),
        }
    }
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total_minutes = self.seconds_since_midnight / 60;
        let hour = total_minutes / 60;
        let minute = total_minutes % 60;
        write!(f, "{hour:02}:{minute:02}")
    }
}

impl FromStr for TimeOfDay {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut parts = value.split(':');
        let hour = parts
            .next()
            .ok_or_else(|| "missing hour".to_string())?
            .parse::<u64>()
            .map_err(|_| format!("invalid hour in '{value}'"))?;
        let minute = parts
            .next()
            .ok_or_else(|| "missing minutes".to_string())?
            .parse::<u64>()
            .map_err(|_| format!("invalid minutes in '{value}'"))?;

        if parts.next().is_some() {
            return Err(format!(
                "invalid time '{value}', expected HH:MM in 24-hour format"
            ));
        }
        if hour >= 24 || minute >= 60 {
            return Err(format!(
                "invalid time '{value}', expected HH:MM in 24-hour format"
            ));
        }

        Ok(Self::from_hm(hour, minute))
    }
}
