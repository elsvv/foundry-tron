//! TRX/SUN unit conversions. 1 TRX = 1_000_000 SUN.

pub const SUN_PER_TRX: u64 = 1_000_000;

pub const fn trx_to_sun(trx: u64) -> u64 {
    trx * SUN_PER_TRX
}

/// Formats a SUN amount as a decimal TRX string, e.g. 1_500_000 -> "1.500000".
pub fn format_sun_as_trx(sun: u64) -> String {
    format!("{}.{:06}", sun / SUN_PER_TRX, sun % SUN_PER_TRX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_trx_to_sun() {
        assert_eq!(trx_to_sun(1), 1_000_000);
        assert_eq!(trx_to_sun(1000), 1_000_000_000);
    }

    #[test]
    fn formats_sun_as_trx() {
        assert_eq!(format_sun_as_trx(1_500_000), "1.500000");
        assert_eq!(format_sun_as_trx(0), "0.000000");
        assert_eq!(format_sun_as_trx(1), "0.000001");
    }
}
