#[cfg(test)]
use std::fmt;

use serde::{Deserialize, Serialize};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

pub const DEFAULT_MIN_AGE_SECONDS: u64 = 30 * 24 * 60 * 60;
pub const DEFAULT_MAX_AGE_SECONDS: u64 = 365 * 24 * 60 * 60;
pub const DEFAULT_MAX_SIZE_BYTES: u64 = 512 * 1024 * 1024;
pub const DEFAULT_POWER: f64 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i64)]
#[serde(rename_all = "lowercase")]
pub enum ExpiryMode {
    Relative = 1,
    Absolute = 2,
    Decay = 3,
}

impl TryFrom<i64> for ExpiryMode {
    type Error = ExpiryError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Relative),
            2 => Ok(Self::Absolute),
            3 => Ok(Self::Decay),
            _ => Err(ExpiryError::InvalidMode(value)),
        }
    }
}

impl From<ExpiryMode> for i64 {
    fn from(value: ExpiryMode) -> Self {
        value as Self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i64)]
#[serde(rename_all = "lowercase")]
pub enum ExpiryTargetKind {
    Site = 1,
    Folder = 2,
    File = 3,
}

impl TryFrom<i64> for ExpiryTargetKind {
    type Error = ExpiryError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Site),
            2 => Ok(Self::Folder),
            3 => Ok(Self::File),
            _ => Err(ExpiryError::InvalidTargetKind(value)),
        }
    }
}

impl From<ExpiryTargetKind> for i64 {
    fn from(value: ExpiryTargetKind) -> Self {
        value as Self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DecayPolicy {
    pub min_age_seconds: u64,
    pub max_age_seconds: u64,
    pub max_size_bytes: u64,
    pub power: f64,
}

impl Default for DecayPolicy {
    fn default() -> Self {
        Self {
            min_age_seconds: DEFAULT_MIN_AGE_SECONDS,
            max_age_seconds: DEFAULT_MAX_AGE_SECONDS,
            max_size_bytes: DEFAULT_MAX_SIZE_BYTES,
            power: DEFAULT_POWER,
        }
    }
}

impl DecayPolicy {
    pub fn validate(self) -> Result<Self, ExpiryError> {
        if self.min_age_seconds > self.max_age_seconds {
            return Err(ExpiryError::MinAgeExceedsMaxAge);
        }
        if self.max_size_bytes == 0 {
            return Err(ExpiryError::ZeroMaxSize);
        }
        if !self.power.is_finite() || self.power <= 0.0 {
            return Err(ExpiryError::InvalidPower);
        }
        Ok(self)
    }

    /// Computes the policy retention in seconds, rounded to the nearest second.
    ///
    /// The endpoint cases are handled exactly. Interior values use:
    /// `min_age + (max_age - min_age) * (1 - min(size, max_size) / max_size)^power`.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    pub fn retention_seconds(self, size_bytes: u64) -> Result<u64, ExpiryError> {
        let policy = self.validate()?;
        if size_bytes == 0 {
            return Ok(policy.max_age_seconds);
        }
        if size_bytes >= policy.max_size_bytes {
            return Ok(policy.min_age_seconds);
        }

        let size_fraction = size_bytes as f64 / policy.max_size_bytes as f64;
        let age_range = (policy.max_age_seconds - policy.min_age_seconds) as f64;
        let retention = age_range.mul_add(
            (1.0 - size_fraction).powf(policy.power),
            policy.min_age_seconds as f64,
        );
        Ok(retention.round() as u64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum ExpiryPolicy {
    Relative { duration_seconds: u64 },
    Absolute { deadline_unix_seconds: i64 },
    Decay(DecayPolicy),
}

impl ExpiryPolicy {
    pub const fn mode(self) -> ExpiryMode {
        match self {
            Self::Relative { .. } => ExpiryMode::Relative,
            Self::Absolute { .. } => ExpiryMode::Absolute,
            Self::Decay(_) => ExpiryMode::Decay,
        }
    }

    pub fn validate(self) -> Result<Self, ExpiryError> {
        match self {
            Self::Relative {
                duration_seconds: 0,
            } => Err(ExpiryError::ZeroDuration),
            Self::Relative { .. } | Self::Absolute { .. } => Ok(self),
            Self::Decay(policy) => policy.validate().map(Self::Decay),
        }
    }

    pub fn retention_seconds(self, size_bytes: u64) -> Result<Option<u64>, ExpiryError> {
        match self.validate()? {
            Self::Relative { duration_seconds } => Ok(Some(duration_seconds)),
            Self::Absolute { .. } => Ok(None),
            Self::Decay(policy) => policy.retention_seconds(size_bytes).map(Some),
        }
    }

    #[cfg(test)]
    pub fn own_deadline(
        self,
        refreshed_at_unix_seconds: i64,
        size_bytes: u64,
    ) -> Result<i64, ExpiryError> {
        match self.validate()? {
            Self::Relative { duration_seconds } => {
                checked_deadline(refreshed_at_unix_seconds, duration_seconds)
            }
            Self::Absolute {
                deadline_unix_seconds,
            } => Ok(deadline_unix_seconds),
            Self::Decay(policy) => checked_deadline(
                refreshed_at_unix_seconds,
                policy.retention_seconds(size_bytes)?,
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpiryTarget {
    pub site: String,
    pub path: Option<String>,
    pub kind: ExpiryTargetKind,
}

impl ExpiryTarget {
    #[cfg(test)]
    pub fn validate(self) -> Result<Self, ExpiryError> {
        if self.site.is_empty() {
            return Err(ExpiryError::EmptySite);
        }
        match (self.kind, self.path.as_deref()) {
            (ExpiryTargetKind::Site, None) => Ok(self),
            (ExpiryTargetKind::Folder | ExpiryTargetKind::File, Some(path)) if !path.is_empty() => {
                Ok(self)
            }
            _ => Err(ExpiryError::TargetKindPathMismatch),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnExpiryReport {
    pub mode: ExpiryMode,
    pub min_age_seconds: Option<u64>,
    pub max_age_seconds: Option<u64>,
    pub max_size_bytes: Option<u64>,
    pub power: Option<f64>,
    pub retention_seconds: Option<u64>,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InheritedExpiryCap {
    pub kind: ExpiryTargetKind,
    pub path: Option<String>,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpiryReport {
    pub target: ExpiryTarget,
    pub size: u64,
    pub refreshed_at: Option<String>,
    pub own_policy: Option<OwnExpiryReport>,
    pub inherited_caps: Vec<InheritedExpiryCap>,
    pub effective_expires_at: Option<String>,
    pub remaining_seconds: Option<u64>,
    pub limited_by: Option<ExpiryLimit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpiryLimit {
    pub kind: ExpiryTargetKind,
    pub path: Option<String>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanDuration {
    pub days: u64,
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
}

#[cfg(test)]
impl HumanDuration {
    pub const fn from_seconds(total_seconds: u64) -> Self {
        const DAY: u64 = 24 * 60 * 60;
        Self {
            days: total_seconds / DAY,
            hours: ((total_seconds % DAY) / (60 * 60)) as u8,
            minutes: ((total_seconds % (60 * 60)) / 60) as u8,
            seconds: (total_seconds % 60) as u8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExpiryError {
    #[error("invalid expiry mode value {0}")]
    InvalidMode(i64),
    #[error("Expiry-Mode must be relative, absolute, decay, or never")]
    InvalidModeHeader,
    #[error("{0} header is required")]
    MissingHeader(&'static str),
    #[error("invalid expiry target kind value {0}")]
    InvalidTargetKind(i64),
    #[error("duration must be a positive integer followed by s, m, h, d, or w")]
    InvalidDuration,
    #[error("duration must be greater than zero")]
    ZeroDuration,
    #[error("duration is too large")]
    DurationOverflow,
    #[error(
        "size must be an integer followed by B, KB, MB, GB, TB, PB, EB, KiB, MiB, GiB, TiB, PiB, or EiB"
    )]
    InvalidSize,
    #[error("size is too large")]
    SizeOverflow,
    #[error("min-age must not exceed max-age")]
    MinAgeExceedsMaxAge,
    #[error("max-size must be greater than zero")]
    ZeroMaxSize,
    #[error("power must be finite and greater than zero")]
    InvalidPower,
    #[error("site must not be empty")]
    EmptySite,
    #[error("target kind does not match path")]
    TargetKindPathMismatch,
    #[error("timestamp must be RFC3339 with an explicit offset")]
    InvalidTimestamp,
    #[error("deadline is outside the supported range")]
    DeadlineOverflow,
}

pub fn parse_duration_seconds(input: &str) -> Result<u64, ExpiryError> {
    let Some((&unit, digits)) = input.as_bytes().split_last() else {
        return Err(ExpiryError::InvalidDuration);
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(ExpiryError::InvalidDuration);
    }
    let value = std::str::from_utf8(digits)
        .expect("ASCII digits are valid UTF-8")
        .parse::<u64>()
        .map_err(|_| ExpiryError::DurationOverflow)?;
    if value == 0 {
        return Err(ExpiryError::ZeroDuration);
    }
    let multiplier = match unit {
        b's' => 1,
        b'm' => 60,
        b'h' => 60 * 60,
        b'd' => 24 * 60 * 60,
        b'w' => 7 * 24 * 60 * 60,
        _ => return Err(ExpiryError::InvalidDuration),
    };
    value
        .checked_mul(multiplier)
        .ok_or(ExpiryError::DurationOverflow)
}

pub fn parse_size_bytes(input: &str) -> Result<u64, ExpiryError> {
    let digit_count = input.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count == 0 {
        return Err(ExpiryError::InvalidSize);
    }
    let (digits, unit) = input.split_at(digit_count);
    let value = digits
        .parse::<u64>()
        .map_err(|_| ExpiryError::SizeOverflow)?;
    let multiplier = match unit {
        "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        "PB" => 1_000_000_000_000_000,
        "EB" => 1_000_000_000_000_000_000,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        "TiB" => 1 << 40,
        "PiB" => 1 << 50,
        "EiB" => 1 << 60,
        _ => return Err(ExpiryError::InvalidSize),
    };
    value
        .checked_mul(multiplier)
        .ok_or(ExpiryError::SizeOverflow)
}

pub fn parse_rfc3339_timestamp(input: &str) -> Result<OffsetDateTime, ExpiryError> {
    let bytes = input.as_bytes();
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(bytes.get(10), Some(b'T' | b't'))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return Err(ExpiryError::InvalidTimestamp);
    }

    let year = parse_fixed_i32(bytes, 0, 4)?;
    let month = parse_fixed_u8(bytes, 5, 2)?;
    let day = parse_fixed_u8(bytes, 8, 2)?;
    let hour = parse_fixed_u8(bytes, 11, 2)?;
    let minute = parse_fixed_u8(bytes, 14, 2)?;
    let second = parse_fixed_u8(bytes, 17, 2)?;

    let mut position = 19;
    let nanosecond = if bytes.get(position) == Some(&b'.') {
        position += 1;
        let fraction_start = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        let fraction_len = position - fraction_start;
        if fraction_len == 0 || fraction_len > 9 {
            return Err(ExpiryError::InvalidTimestamp);
        }
        let fraction = parse_fixed_u32(bytes, fraction_start, fraction_len)?;
        fraction
            * 10_u32
                .pow(u32::try_from(9 - fraction_len).map_err(|_| ExpiryError::InvalidTimestamp)?)
    } else {
        0
    };

    let offset = match bytes.get(position) {
        Some(b'Z' | b'z') if position + 1 == bytes.len() => UtcOffset::UTC,
        Some(sign @ (b'+' | b'-')) if position + 6 == bytes.len() => {
            if bytes.get(position + 3) != Some(&b':') {
                return Err(ExpiryError::InvalidTimestamp);
            }
            let offset_hour = parse_fixed_u8(bytes, position + 1, 2)?;
            let offset_minute = parse_fixed_u8(bytes, position + 4, 2)?;
            if offset_hour > 23 || offset_minute > 59 {
                return Err(ExpiryError::InvalidTimestamp);
            }
            let direction = if *sign == b'-' { -1 } else { 1 };
            UtcOffset::from_hms(
                direction * i8::try_from(offset_hour).map_err(|_| ExpiryError::InvalidTimestamp)?,
                direction
                    * i8::try_from(offset_minute).map_err(|_| ExpiryError::InvalidTimestamp)?,
                0,
            )
            .map_err(|_| ExpiryError::InvalidTimestamp)?
        }
        _ => return Err(ExpiryError::InvalidTimestamp),
    };

    let month = Month::try_from(month).map_err(|_| ExpiryError::InvalidTimestamp)?;
    let date =
        Date::from_calendar_date(year, month, day).map_err(|_| ExpiryError::InvalidTimestamp)?;
    let time = Time::from_hms_nano(hour, minute, second, nanosecond)
        .map_err(|_| ExpiryError::InvalidTimestamp)?;
    Ok(PrimitiveDateTime::new(date, time).assume_offset(offset))
}

#[cfg(test)]
pub fn checked_deadline(
    refreshed_at_unix_seconds: i64,
    retention_seconds: u64,
) -> Result<i64, ExpiryError> {
    let retention = i64::try_from(retention_seconds).map_err(|_| ExpiryError::DeadlineOverflow)?;
    refreshed_at_unix_seconds
        .checked_add(retention)
        .ok_or(ExpiryError::DeadlineOverflow)
}

pub fn remaining_seconds(deadline_unix_seconds: i64, now_unix_seconds: i64) -> u64 {
    u64::try_from(i128::from(deadline_unix_seconds) - i128::from(now_unix_seconds)).unwrap_or(0)
}

#[cfg(test)]
pub fn earliest_deadline(
    own_deadline_unix_seconds: Option<i64>,
    inherited_deadlines_unix_seconds: impl IntoIterator<Item = i64>,
) -> Option<i64> {
    inherited_deadlines_unix_seconds
        .into_iter()
        .fold(own_deadline_unix_seconds, |earliest, deadline| {
            Some(earliest.map_or(deadline, |current| current.min(deadline)))
        })
}

fn parse_fixed_u8(bytes: &[u8], start: usize, len: usize) -> Result<u8, ExpiryError> {
    parse_fixed_u32(bytes, start, len)?
        .try_into()
        .map_err(|_| ExpiryError::InvalidTimestamp)
}

fn parse_fixed_i32(bytes: &[u8], start: usize, len: usize) -> Result<i32, ExpiryError> {
    parse_fixed_u32(bytes, start, len)?
        .try_into()
        .map_err(|_| ExpiryError::InvalidTimestamp)
}

fn parse_fixed_u32(bytes: &[u8], start: usize, len: usize) -> Result<u32, ExpiryError> {
    let digits = bytes
        .get(start..start + len)
        .ok_or(ExpiryError::InvalidTimestamp)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return Err(ExpiryError::InvalidTimestamp);
    }
    digits.iter().try_fold(0_u32, |value, digit| {
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u32::from(*digit - b'0')))
            .ok_or(ExpiryError::InvalidTimestamp)
    })
}

#[cfg(test)]
impl fmt::Display for HumanDuration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut wrote_part = false;
        for (value, suffix) in [
            (self.days, "d"),
            (u64::from(self.hours), "h"),
            (u64::from(self.minutes), "m"),
            (u64::from(self.seconds), "s"),
        ] {
            if value != 0 || !wrote_part && suffix == "s" {
                if wrote_part {
                    formatter.write_str(" ")?;
                }
                write!(formatter, "{value}{suffix}")?;
                wrote_part = true;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(power: f64) -> DecayPolicy {
        DecayPolicy {
            min_age_seconds: 100,
            max_age_seconds: 1_000,
            max_size_bytes: 100,
            power,
        }
    }

    #[test]
    fn integer_backed_enums_round_trip() {
        for (raw, mode) in [
            (1, ExpiryMode::Relative),
            (2, ExpiryMode::Absolute),
            (3, ExpiryMode::Decay),
        ] {
            assert_eq!(ExpiryMode::try_from(raw), Ok(mode));
            assert_eq!(i64::from(mode), raw);
        }
        assert!(matches!(
            ExpiryMode::try_from(0),
            Err(ExpiryError::InvalidMode(0))
        ));

        for (raw, kind) in [
            (1, ExpiryTargetKind::Site),
            (2, ExpiryTargetKind::Folder),
            (3, ExpiryTargetKind::File),
        ] {
            assert_eq!(ExpiryTargetKind::try_from(raw), Ok(kind));
            assert_eq!(i64::from(kind), raw);
        }
        assert!(matches!(
            ExpiryTargetKind::try_from(4),
            Err(ExpiryError::InvalidTargetKind(4))
        ));
    }

    #[test]
    fn duration_grammar_accepts_each_unit() {
        assert_eq!(parse_duration_seconds("1s"), Ok(1));
        assert_eq!(parse_duration_seconds("2m"), Ok(120));
        assert_eq!(parse_duration_seconds("3h"), Ok(10_800));
        assert_eq!(parse_duration_seconds("4d"), Ok(345_600));
        assert_eq!(parse_duration_seconds("5w"), Ok(3_024_000));
    }

    #[test]
    fn duration_grammar_rejects_zero_invalid_and_overflow() {
        assert_eq!(parse_duration_seconds("0s"), Err(ExpiryError::ZeroDuration));
        for input in ["", "1", "1S", "-1s", "+1s", "1.5h", " 1h", "1h ", "1é"] {
            assert_eq!(
                parse_duration_seconds(input),
                Err(ExpiryError::InvalidDuration)
            );
        }
        assert_eq!(
            parse_duration_seconds("18446744073709551616s"),
            Err(ExpiryError::DurationOverflow)
        );
        assert_eq!(
            parse_duration_seconds("18446744073709551615w"),
            Err(ExpiryError::DurationOverflow)
        );
    }

    #[test]
    fn size_grammar_handles_decimal_and_binary_units() {
        assert_eq!(parse_size_bytes("0B"), Ok(0));
        assert_eq!(parse_size_bytes("2KB"), Ok(2_000));
        assert_eq!(parse_size_bytes("3MB"), Ok(3_000_000));
        assert_eq!(parse_size_bytes("4GB"), Ok(4_000_000_000));
        assert_eq!(parse_size_bytes("2KiB"), Ok(2_048));
        assert_eq!(parse_size_bytes("3MiB"), Ok(3 * (1 << 20)));
        assert_eq!(parse_size_bytes("2GiB"), Ok(2 * (1 << 30)));
        assert_eq!(parse_size_bytes("1EiB"), Ok(1 << 60));
    }

    #[test]
    fn size_grammar_rejects_invalid_and_overflow() {
        for input in ["", "12", "MiB", "-1B", "1.5GB", "1mb", "1K", " 1B"] {
            assert_eq!(parse_size_bytes(input), Err(ExpiryError::InvalidSize));
        }
        assert_eq!(
            parse_size_bytes("18446744073709551616B"),
            Err(ExpiryError::SizeOverflow)
        );
        assert_eq!(parse_size_bytes("16EiB"), Err(ExpiryError::SizeOverflow));
    }

    #[test]
    fn decay_endpoints_are_exact_and_above_max_is_clamped() {
        let policy = policy(3.0);
        assert_eq!(policy.retention_seconds(0), Ok(1_000));
        assert_eq!(policy.retention_seconds(100), Ok(100));
        assert_eq!(policy.retention_seconds(101), Ok(100));
        assert_eq!(policy.retention_seconds(u64::MAX), Ok(100));
    }

    #[test]
    fn decay_uses_requested_power() {
        assert_eq!(policy(1.0).retention_seconds(50), Ok(550));
        assert_eq!(policy(2.0).retention_seconds(50), Ok(325));
        assert_eq!(policy(3.0).retention_seconds(50), Ok(213));
        assert_eq!(policy(0.5).retention_seconds(75), Ok(550));
    }

    #[test]
    fn decay_validation_rejects_invalid_ranges_sizes_and_powers() {
        assert_eq!(
            DecayPolicy {
                min_age_seconds: 2,
                max_age_seconds: 1,
                ..policy(1.0)
            }
            .validate(),
            Err(ExpiryError::MinAgeExceedsMaxAge)
        );
        assert_eq!(
            DecayPolicy {
                max_size_bytes: 0,
                ..policy(1.0)
            }
            .validate(),
            Err(ExpiryError::ZeroMaxSize)
        );
        for power in [0.0, -1.0, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert_eq!(policy(power).validate(), Err(ExpiryError::InvalidPower));
        }
    }

    #[test]
    fn policies_compute_own_deadlines_without_string_dispatch() {
        let relative = ExpiryPolicy::Relative {
            duration_seconds: 60,
        };
        assert_eq!(relative.mode(), ExpiryMode::Relative);
        assert_eq!(relative.own_deadline(1_000, 999), Ok(1_060));

        let absolute = ExpiryPolicy::Absolute {
            deadline_unix_seconds: 42,
        };
        assert_eq!(absolute.mode(), ExpiryMode::Absolute);
        assert_eq!(absolute.own_deadline(1_000, 999), Ok(42));

        let decay = ExpiryPolicy::Decay(policy(2.0));
        assert_eq!(decay.mode(), ExpiryMode::Decay);
        assert_eq!(decay.own_deadline(1_000, 50), Ok(1_325));
    }

    #[test]
    fn deadline_and_remaining_helpers_cover_boundaries() {
        assert_eq!(checked_deadline(i64::MAX - 1, 1), Ok(i64::MAX));
        assert_eq!(
            checked_deadline(i64::MAX, 1),
            Err(ExpiryError::DeadlineOverflow)
        );
        assert_eq!(
            checked_deadline(0, u64::MAX),
            Err(ExpiryError::DeadlineOverflow)
        );
        assert_eq!(remaining_seconds(200, 100), 100);
        assert_eq!(remaining_seconds(100, 100), 0);
        assert_eq!(remaining_seconds(99, 100), 0);
        assert_eq!(remaining_seconds(i64::MAX, i64::MIN), u64::MAX);
        assert_eq!(earliest_deadline(Some(30), [40, 20, 50]), Some(20));
        assert_eq!(earliest_deadline(None, [40, 20, 50]), Some(20));
        assert_eq!(earliest_deadline(None, []), None);
    }

    #[test]
    fn rfc3339_parser_requires_an_explicit_valid_offset() {
        let utc = parse_rfc3339_timestamp("2026-08-20T19:20:00Z").unwrap();
        let offset = parse_rfc3339_timestamp("2026-08-20T21:20:00+02:00").unwrap();
        assert_eq!(utc.unix_timestamp(), offset.unix_timestamp());

        let fractional = parse_rfc3339_timestamp("2026-08-20T19:20:00.123456789-00:30").unwrap();
        assert_eq!(fractional.nanosecond(), 123_456_789);
        assert_eq!(fractional.offset().whole_minutes(), -30);

        for input in [
            "2026-08-20T19:20:00",
            "2026-02-30T19:20:00Z",
            "2026-08-20 19:20:00Z",
            "2026-08-20T24:00:00Z",
            "2026-08-20T19:60:00Z",
            "2026-08-20T19:20:60Z",
            "2026-08-20T19:20:00+24:00",
            "2026-08-20T19:20:00.1234567890Z",
            "2026-08-20T19:20:00+0200",
            "not-a-time",
        ] {
            assert_eq!(
                parse_rfc3339_timestamp(input),
                Err(ExpiryError::InvalidTimestamp),
                "{input}"
            );
        }
    }

    #[test]
    fn targets_validate_kind_and_path_together() {
        let site = ExpiryTarget {
            site: "hello".into(),
            path: None,
            kind: ExpiryTargetKind::Site,
        };
        assert_eq!(site.clone().validate(), Ok(site));

        let file = ExpiryTarget {
            site: "hello".into(),
            path: Some("index.html".into()),
            kind: ExpiryTargetKind::File,
        };
        assert_eq!(file.clone().validate(), Ok(file));

        assert_eq!(
            ExpiryTarget {
                site: "hello".into(),
                path: None,
                kind: ExpiryTargetKind::Folder,
            }
            .validate(),
            Err(ExpiryError::TargetKindPathMismatch)
        );
        assert_eq!(
            ExpiryTarget {
                site: String::new(),
                path: None,
                kind: ExpiryTargetKind::Site,
            }
            .validate(),
            Err(ExpiryError::EmptySite)
        );
    }

    #[test]
    fn human_duration_has_stable_components_and_text() {
        assert_eq!(
            HumanDuration::from_seconds(183_845),
            HumanDuration {
                days: 2,
                hours: 3,
                minutes: 4,
                seconds: 5,
            }
        );
        assert_eq!(
            HumanDuration::from_seconds(183_845).to_string(),
            "2d 3h 4m 5s"
        );
        assert_eq!(HumanDuration::from_seconds(0).to_string(), "0s");
    }

    #[test]
    fn serde_shapes_use_named_modes_and_kinds() {
        let policy = ExpiryPolicy::Decay(policy(2.0));
        let value = serde_json::to_value(policy).unwrap();
        assert_eq!(value["mode"], "decay");
        assert_eq!(value["min_age_seconds"], 100);

        let target = ExpiryTarget {
            site: "hello".into(),
            path: Some("assets".into()),
            kind: ExpiryTargetKind::Folder,
        };
        let value = serde_json::to_value(target).unwrap();
        assert_eq!(value["kind"], "folder");
    }
}
