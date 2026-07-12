//! TRX/SUN unit conversions. 1 TRX = 1_000_000 SUN.

pub const SUN_PER_TRX: u64 = 1_000_000;

/// Number of decimal places a TRX amount can carry (1 TRX = 1e6 SUN).
const TRX_DECIMALS: usize = 6;

pub const fn trx_to_sun(trx: u64) -> u64 {
    trx * SUN_PER_TRX
}

/// Formats a SUN amount as a decimal TRX string, e.g. 1_500_000 -> "1.500000".
pub fn format_sun_as_trx(sun: u64) -> String {
    format!("{}.{:06}", sun / SUN_PER_TRX, sun % SUN_PER_TRX)
}

/// Error returned when a decimal TRX string cannot be parsed into SUN.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseSunError {
    /// The input was empty or contained only a bare decimal point.
    #[error("empty TRX amount")]
    Empty,
    /// The input contained a sign, exponent or other non-digit character.
    #[error("invalid TRX amount: {0}")]
    Invalid(String),
    /// The fractional part had more than six digits (SUN is the smallest unit).
    #[error("too many decimal places: TRX has at most 6 (1 TRX = 1e6 SUN)")]
    TooPrecise,
    /// The amount does not fit into a `u64` of SUN.
    #[error("TRX amount overflows u64 SUN")]
    Overflow,
}

/// Parses a decimal TRX amount into SUN. 1 TRX = 1_000_000 SUN, so at most six
/// fractional digits are accepted; sign characters, exponents and any other
/// non-digit input are rejected. Examples: `"1.5"` -> `1_500_000`,
/// `"0.000001"` -> `1`, `"1000"` -> `1_000_000_000`.
pub fn parse_trx_to_sun(trx: &str) -> Result<u64, ParseSunError> {
    let s = trx.trim();
    if s.is_empty() {
        return Err(ParseSunError::Empty);
    }
    let (int_str, frac_str) = s.split_once('.').unwrap_or((s, ""));
    // Reject a bare "." and any non-digit characters (this also rejects signs).
    if int_str.is_empty() && frac_str.is_empty() {
        return Err(ParseSunError::Invalid(trx.to_string()));
    }
    let all_digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(int_str) || !all_digits(frac_str) {
        return Err(ParseSunError::Invalid(trx.to_string()));
    }
    if frac_str.len() > TRX_DECIMALS {
        return Err(ParseSunError::TooPrecise);
    }
    let int_part: u64 = if int_str.is_empty() {
        0
    } else {
        int_str.parse().map_err(|_| ParseSunError::Overflow)?
    };
    // Right-pad the fraction to exactly six digits so it reads directly as SUN.
    let mut frac_padded = frac_str.to_string();
    while frac_padded.len() < TRX_DECIMALS {
        frac_padded.push('0');
    }
    let frac_part: u64 = frac_padded.parse().map_err(|_| ParseSunError::Overflow)?;
    int_part
        .checked_mul(SUN_PER_TRX)
        .and_then(|v| v.checked_add(frac_part))
        .ok_or(ParseSunError::Overflow)
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

    #[test]
    fn parses_trx_to_sun() {
        assert_eq!(parse_trx_to_sun("1.5").unwrap(), 1_500_000);
        assert_eq!(parse_trx_to_sun("1").unwrap(), 1_000_000);
        assert_eq!(parse_trx_to_sun("0.000001").unwrap(), 1);
        assert_eq!(parse_trx_to_sun("1000").unwrap(), 1_000_000_000);
        assert_eq!(parse_trx_to_sun("1.500000").unwrap(), 1_500_000);
        // Leading/trailing partial decimals.
        assert_eq!(parse_trx_to_sun(".5").unwrap(), 500_000);
        assert_eq!(parse_trx_to_sun("5.").unwrap(), 5_000_000);
        assert_eq!(parse_trx_to_sun("  2.25  ").unwrap(), 2_250_000);
    }

    #[test]
    fn parse_trx_round_trips_with_format() {
        for sun in [0u64, 1, 999_999, 1_500_000, 123_456_789] {
            assert_eq!(parse_trx_to_sun(&format_sun_as_trx(sun)).unwrap(), sun);
        }
    }

    #[test]
    fn rejects_bad_trx_amounts() {
        assert_eq!(parse_trx_to_sun(""), Err(ParseSunError::Empty));
        assert_eq!(parse_trx_to_sun("."), Err(ParseSunError::Invalid(".".into())));
        assert_eq!(parse_trx_to_sun("-1"), Err(ParseSunError::Invalid("-1".into())));
        assert_eq!(parse_trx_to_sun("1.2.3"), Err(ParseSunError::Invalid("1.2.3".into())));
        assert_eq!(parse_trx_to_sun("abc"), Err(ParseSunError::Invalid("abc".into())));
        // Seven fractional digits exceed SUN precision.
        assert_eq!(parse_trx_to_sun("1.0000001"), Err(ParseSunError::TooPrecise));
    }
}
