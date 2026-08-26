const LEAP_YEARS_PER_GREGORIAN_CYCLE: u64 = 97;
const YEARS_PER_GREGORIAN_CYCLE: u64 = 400;
const AVERAGE_DAYS_PER_YEAR: f64 =
    365.0 + LEAP_YEARS_PER_GREGORIAN_CYCLE as f64 / YEARS_PER_GREGORIAN_CYCLE as f64;

pub const SECONDS_PER_MINUTE: u64 = 60;
pub const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;
pub const SECONDS_PER_DAY: u64 = 24 * SECONDS_PER_HOUR;
pub const SECONDS_PER_WEEK: u64 = 7 * SECONDS_PER_DAY;
pub const SECONDS_PER_MONTH: u64 = (AVERAGE_DAYS_PER_YEAR / 12.0 * SECONDS_PER_DAY as f64) as u64;
pub const SECONDS_PER_YEAR: u64 = (AVERAGE_DAYS_PER_YEAR * SECONDS_PER_DAY as f64) as u64;

pub const KIB: u64 = 1024;
pub const MIB: u64 = 1024 * KIB;
pub const GIB: u64 = 1024 * MIB;
pub const TIB: u64 = 1024 * GIB;
pub const PIB: u64 = 1024 * TIB;
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
