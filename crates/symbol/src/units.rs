const HOURS_PER_DAY: u64 = 24;
const MINUTES_PER_HOUR: u64 = 60;

pub const SECONDS_PER_MINUTE: u64 = 60;
pub const SECONDS_PER_HOUR: u64 = MINUTES_PER_HOUR * SECONDS_PER_MINUTE;
pub const SECONDS_PER_DAY: u64 = HOURS_PER_DAY * SECONDS_PER_HOUR;

pub const KIB: u64 = 1024;
pub const MIB: u64 = 1024 * KIB;

#[cfg(test)]
const LEAP_YEARS_PER_GREGORIAN_CYCLE: u64 = 97;
#[cfg(test)]
const YEARS_PER_GREGORIAN_CYCLE: u64 = 400;
#[cfg(test)]
const COMMON_YEARS_PER_GREGORIAN_CYCLE: u64 =
    YEARS_PER_GREGORIAN_CYCLE - LEAP_YEARS_PER_GREGORIAN_CYCLE;
#[cfg(test)]
const DAYS_PER_COMMON_YEAR: u64 = 365;
#[cfg(test)]
const DAYS_PER_LEAP_YEAR: u64 = 366;
#[cfg(test)]
const DAYS_PER_GREGORIAN_CYCLE: u64 = COMMON_YEARS_PER_GREGORIAN_CYCLE * DAYS_PER_COMMON_YEAR
    + LEAP_YEARS_PER_GREGORIAN_CYCLE * DAYS_PER_LEAP_YEAR;
#[cfg(test)]
const MONTHS_PER_YEAR: u64 = 12;

#[cfg(test)]
pub const SECONDS_PER_WEEK: u64 = 7 * SECONDS_PER_DAY;
#[cfg(test)]
pub const SECONDS_PER_YEAR: u64 =
    DAYS_PER_GREGORIAN_CYCLE * SECONDS_PER_DAY / YEARS_PER_GREGORIAN_CYCLE;
#[cfg(test)]
pub const SECONDS_PER_MONTH: u64 = SECONDS_PER_YEAR / MONTHS_PER_YEAR;

#[cfg(test)]
pub const GIB: u64 = 1024 * MIB;
#[cfg(test)]
pub const TIB: u64 = 1024 * GIB;
#[cfg(test)]
pub const PIB: u64 = 1024 * TIB;
#[cfg(test)]
pub const EIB: u64 = 1024 * PIB;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gregorian_average_units_are_stable() {
        assert_eq!(SECONDS_PER_DAY, 86_400);
        assert_eq!(SECONDS_PER_WEEK, 604_800);
        assert_eq!(SECONDS_PER_MONTH, 2_629_746);
        assert_eq!(SECONDS_PER_YEAR, 31_556_952);
    }

    #[test]
    fn binary_units_scale_by_1024() {
        assert_eq!(MIB, 1024 * KIB);
        assert_eq!(GIB, 1024 * MIB);
        assert_eq!(TIB, 1024 * GIB);
        assert_eq!(PIB, 1024 * TIB);
        assert_eq!(EIB, 1024 * PIB);
    }
}
