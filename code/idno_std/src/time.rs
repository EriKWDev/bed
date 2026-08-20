pub type Micros = i64;
pub const MICROS_PER_SECOND: Micros = 1_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeStamp(pub Micros);

impl TimeStamp {
    pub const ZERO: Self = Self(0);
    pub const MIN: Self = Self(Micros::MIN);
    pub const MAX: Self = Self(Micros::MAX);

    pub fn from_duration(duration: std::time::Duration) -> Self {
        Self(duration.as_micros().min(i64::MAX as u128) as i64)
    }

    pub fn from_seconds_f32(seconds: f32) -> Self {
        Self((seconds as f64 * MICROS_PER_SECOND as f64).round() as i64)
    }

    pub fn from_micros(micros: Micros) -> Self {
        Self(micros)
    }

    pub fn as_micros(self) -> Micros {
        self.0
    }

    pub fn max(self, other: Self) -> Self {
        Self(self.0.max(other.0))
    }

    pub fn min(self, other: Self) -> Self {
        Self(self.0.min(other.0))
    }

    pub fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }

    pub fn duration_since(self, earlier: Self) -> std::time::Duration {
        std::time::Duration::from_micros(self.0.saturating_sub(earlier.0) as u64)
    }

    pub fn seconds_since(self, earlier: Self) -> f32 {
        self.0.saturating_sub(earlier.0) as f32 / MICROS_PER_SECOND as f32
    }

    pub fn seconds(self) -> f64 {
        let fractional_micros = self.0 % MICROS_PER_SECOND;
        (self.0 - fractional_micros) as f64 / MICROS_PER_SECOND as f64
            + fractional_micros as f64 / MICROS_PER_SECOND as f64
    }

    pub fn with_offset_seconds(self, offset_seconds: f32) -> Self {
        Self(
            self.0
                .saturating_add((offset_seconds as f64 * MICROS_PER_SECOND as f64).round() as i64),
        )
    }

    pub fn wrapping(self, period_seconds: f32) -> f32 {
        let period_micros = (period_seconds as f64 * MICROS_PER_SECOND as f64).round() as i64;
        if period_micros <= 0 {
            return 0.0;
        }
        self.0.rem_euclid(period_micros) as f32 / MICROS_PER_SECOND as f32
    }

    pub fn alpha(self, period_seconds: f32) -> f32 {
        if period_seconds > 0.0 {
            self.wrapping(period_seconds) / period_seconds
        } else {
            0.0
        }
    }

    pub fn sin(self, period_seconds: f32) -> f32 {
        (self.alpha(period_seconds) * core::f32::consts::TAU).sin()
    }

    pub fn cos(self, period_seconds: f32) -> f32 {
        (self.alpha(period_seconds) * core::f32::consts::TAU).cos()
    }

    pub fn sin_01(self, period_seconds: f32) -> f32 {
        self.sin(period_seconds) * 0.5 + 0.5
    }

    pub fn cos_01(self, period_seconds: f32) -> f32 {
        self.cos(period_seconds) * 0.5 + 0.5
    }

    pub fn tau(self, period_seconds: f32) -> f32 {
        self.alpha(period_seconds) * core::f32::consts::TAU
    }

    pub fn pi(self, period_seconds: f32) -> f32 {
        self.alpha(period_seconds) * core::f32::consts::PI
    }

    pub fn with_offset_micros(self, offset: Micros) -> Self {
        Self(self.0.saturating_add(offset))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Time {
    pub timestamp: TimeStamp,
    pub delta_micros: Micros,
}

impl Time {
    pub const ZERO: Self = Self {
        timestamp: TimeStamp::ZERO,
        delta_micros: 0,
    };
    pub const MIN: Self = Self {
        timestamp: TimeStamp::MIN,
        delta_micros: Micros::MIN,
    };
    pub const MAX: Self = Self {
        timestamp: TimeStamp::MAX,
        delta_micros: Micros::MAX,
    };

    pub fn from_durations(elapsed: std::time::Duration, delta: std::time::Duration) -> Self {
        Self {
            timestamp: TimeStamp::from_duration(elapsed),
            delta_micros: delta.as_micros().min(i64::MAX as u128) as i64,
        }
    }

    pub fn from_seconds_f32(seconds: f32) -> Self {
        Self {
            timestamp: TimeStamp::from_seconds_f32(seconds),
            delta_micros: 0,
        }
    }

    pub fn timestamp(self) -> TimeStamp {
        self.timestamp
    }

    pub fn max(self, other: Self) -> Self {
        Self {
            timestamp: self.timestamp.max(other.timestamp),
            delta_micros: self.delta_micros.max(other.delta_micros),
        }
    }

    pub fn min(self, other: Self) -> Self {
        Self {
            timestamp: self.timestamp.min(other.timestamp),
            delta_micros: self.delta_micros.min(other.delta_micros),
        }
    }

    pub fn delta_seconds(self) -> f32 {
        self.delta_micros as f32 / MICROS_PER_SECOND as f32
    }

    pub fn delta_seconds_f64(self) -> f64 {
        self.delta_micros as f64 / MICROS_PER_SECOND as f64
    }

    pub fn with_delta_seconds(self, delta_seconds: f32) -> Self {
        Self {
            delta_micros: (delta_seconds as f64 * MICROS_PER_SECOND as f64).round() as Micros,
            ..self
        }
    }

    pub fn with_delta_micros(self, delta_micros: Micros) -> Self {
        Self {
            delta_micros,
            ..self
        }
    }

    pub fn duration_since(self, earlier: impl Into<TimeStamp>) -> std::time::Duration {
        self.timestamp.duration_since(earlier.into())
    }

    pub fn wrapping(self, period_seconds: f32) -> f32 {
        self.timestamp.wrapping(period_seconds)
    }

    pub fn alpha(self, period_seconds: f32) -> f32 {
        self.timestamp.alpha(period_seconds)
    }

    pub fn sin(self, period_seconds: f32) -> f32 {
        self.timestamp.sin(period_seconds)
    }

    pub fn cos(self, period_seconds: f32) -> f32 {
        self.timestamp.cos(period_seconds)
    }

    pub fn sin_01(self, period_seconds: f32) -> f32 {
        self.timestamp.sin_01(period_seconds)
    }

    pub fn cos_01(self, period_seconds: f32) -> f32 {
        self.timestamp.cos_01(period_seconds)
    }

    pub fn tau(self, period_seconds: f32) -> f32 {
        self.timestamp.tau(period_seconds)
    }

    pub fn pi(self, period_seconds: f32) -> f32 {
        self.timestamp.pi(period_seconds)
    }

    pub fn with_offset_seconds(self, offset_seconds: f32) -> Self {
        Self {
            timestamp: self.timestamp.with_offset_seconds(offset_seconds),
            ..self
        }
    }

    pub fn with_offset_micros(self, offset: Micros) -> Self {
        Self {
            timestamp: self.timestamp.with_offset_micros(offset),
            ..self
        }
    }

    pub fn seconds(self) -> f64 {
        self.timestamp.seconds()
    }
}

impl From<TimeStamp> for Time {
    fn from(timestamp: TimeStamp) -> Self {
        Self {
            timestamp,
            ..Self::ZERO
        }
    }
}

impl From<Time> for TimeStamp {
    fn from(time: Time) -> Self {
        time.timestamp
    }
}

impl std::ops::Sub for TimeStamp {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl std::ops::Add for TimeStamp {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl std::ops::AddAssign for TimeStamp {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl std::ops::SubAssign for TimeStamp {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl std::ops::Add<std::time::Duration> for TimeStamp {
    type Output = Self;
    fn add(self, rhs: std::time::Duration) -> Self {
        self.with_offset_micros(rhs.as_micros().min(Micros::MAX as u128) as Micros)
    }
}

impl std::ops::Sub<std::time::Duration> for TimeStamp {
    type Output = Self;
    fn sub(self, rhs: std::time::Duration) -> Self {
        self.with_offset_micros(-(rhs.as_micros().min(Micros::MAX as u128) as Micros))
    }
}

impl std::ops::Sub<TimeStamp> for Time {
    type Output = Self;
    fn sub(self, rhs: TimeStamp) -> Self {
        Self {
            timestamp: self.timestamp - rhs,
            ..self
        }
    }
}

impl std::ops::Add for Time {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            timestamp: self.timestamp + rhs.timestamp,
            ..self
        }
    }
}

impl std::ops::Sub for Time {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            timestamp: self.timestamp - rhs.timestamp,
            ..self
        }
    }
}

impl std::ops::Add<std::time::Duration> for Time {
    type Output = Self;
    fn add(self, rhs: std::time::Duration) -> Self {
        Self {
            timestamp: self.timestamp + rhs,
            ..self
        }
    }
}

impl std::ops::Sub<std::time::Duration> for Time {
    type Output = Self;
    fn sub(self, rhs: std::time::Duration) -> Self {
        Self {
            timestamp: self.timestamp - rhs,
            ..self
        }
    }
}

impl std::ops::AddAssign<std::time::Duration> for Time {
    fn add_assign(&mut self, rhs: std::time::Duration) {
        self.timestamp = self.timestamp + rhs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_phase_keeps_microsecond_resolution_after_long_uptime() {
        let late = TimeStamp(20 * 365 * 24 * 60 * 60 * MICROS_PER_SECOND + 123_456);
        let one_microsecond_later = TimeStamp(late.0 + 1);

        assert_ne!(late.wrapping(1.0), one_microsecond_later.wrapping(1.0));
        assert!((late.wrapping(1.0) - 0.123_456).abs() < 0.000_002);
    }
}
