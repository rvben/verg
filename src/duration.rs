use crate::error::Error;

/// Parses a human-readable duration string into a [`std::time::Duration`].
///
/// Accepted formats:
/// - Bare integer: treated as seconds (`"45"` -> 45s)
/// - Integer followed by a unit suffix: `s` (seconds), `m` (minutes), `h` (hours), `d` (days)
///
/// Zero is allowed (`"0s"` -> `Duration::ZERO`). The caller is responsible for
/// rejecting a zero value if the context requires a positive duration.
pub fn parse_duration(s: &str) -> Result<std::time::Duration, Error> {
    let s = s.trim();

    if s.is_empty() {
        return Err(Error::Config("duration must not be empty".to_string()));
    }

    // Bare integer: all characters are ASCII digits.
    if s.chars().all(|c| c.is_ascii_digit()) {
        let secs: u64 = s
            .parse()
            .map_err(|_| Error::Config(format!("duration value too large: {s}")))?;
        return Ok(std::time::Duration::from_secs(secs));
    }

    // Split the last CHARACTER as the unit suffix. Split on a char boundary
    // (not s.len() - 1) so a multi-byte trailing char does not panic split_at.
    let last = s.chars().next_back().expect("non-empty checked above");
    let (num_part, unit) = s.split_at(s.len() - last.len_utf8());

    if num_part.is_empty() {
        return Err(Error::Config(format!(
            "duration is missing a numeric value before the unit: {s}"
        )));
    }

    // Reject anything with a sign character or a decimal point.
    if num_part.starts_with('-') || num_part.starts_with('+') {
        return Err(Error::Config(format!(
            "duration must be a non-negative integer: {s}"
        )));
    }
    if num_part.contains('.') {
        return Err(Error::Config(format!(
            "duration must be an integer (no decimals): {s}"
        )));
    }

    let value: u64 = num_part.parse().map_err(|_| {
        Error::Config(format!(
            "duration numeric part is not a valid non-negative integer: {s}"
        ))
    })?;

    let multiplier: u64 = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        other => {
            return Err(Error::Config(format!(
                "unknown duration unit {other:?} in {s:?}; expected s, m, h, or d"
            )));
        }
    };

    let secs = value
        .checked_mul(multiplier)
        .ok_or_else(|| Error::Config(format!("duration overflows u64 seconds: {s}")))?;

    Ok(std::time::Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // --- accept cases ---

    #[test]
    fn parse_seconds_suffix() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
    }

    #[test]
    fn parse_minutes_suffix() {
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
    }

    #[test]
    fn parse_hours_suffix() {
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7_200));
    }

    #[test]
    fn parse_days_suffix() {
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86_400));
    }

    #[test]
    fn parse_bare_integer_as_seconds() {
        assert_eq!(parse_duration("45").unwrap(), Duration::from_secs(45));
    }

    #[test]
    fn parse_zero_seconds_allowed() {
        assert_eq!(parse_duration("0s").unwrap(), Duration::ZERO);
    }

    #[test]
    fn parse_zero_bare_allowed() {
        assert_eq!(parse_duration("0").unwrap(), Duration::ZERO);
    }

    #[test]
    fn parse_large_but_valid_minutes() {
        assert_eq!(parse_duration("60m").unwrap(), Duration::from_secs(3_600));
    }

    // --- reject cases ---

    #[test]
    fn reject_empty_string() {
        assert!(matches!(parse_duration(""), Err(Error::Config(_))));
    }

    #[test]
    fn reject_unit_only_no_number() {
        assert!(matches!(parse_duration("m"), Err(Error::Config(_))));
    }

    #[test]
    fn reject_unknown_unit() {
        assert!(matches!(parse_duration("5x"), Err(Error::Config(_))));
    }

    #[test]
    fn reject_multibyte_unit_without_panic() {
        // A multi-byte trailing char must NOT panic split_at; it is an unknown
        // unit and must return an error.
        assert!(matches!(parse_duration("5ñ"), Err(Error::Config(_))));
        assert!(matches!(parse_duration("3€"), Err(Error::Config(_))));
    }

    #[test]
    fn reject_negative_value() {
        assert!(matches!(parse_duration("-3m"), Err(Error::Config(_))));
    }

    #[test]
    fn reject_float_value() {
        assert!(matches!(parse_duration("1.5h"), Err(Error::Config(_))));
    }

    #[test]
    fn reject_overflow_days() {
        // u64::MAX / 86400 = 213503982334601 (rounds down); adding 1 overflows.
        assert!(matches!(
            parse_duration("213503982334602d"),
            Err(Error::Config(_))
        ));
    }

    #[test]
    fn reject_overflow_bare_integer() {
        // u64::MAX is 18446744073709551615; this is one more digit.
        assert!(matches!(
            parse_duration("99999999999999999999"),
            Err(Error::Config(_))
        ));
    }

    #[test]
    fn reject_whitespace_only() {
        assert!(matches!(parse_duration("   "), Err(Error::Config(_))));
    }

    #[test]
    fn reject_unit_with_no_number_hours() {
        assert!(matches!(parse_duration("h"), Err(Error::Config(_))));
    }

    #[test]
    fn reject_unknown_unit_x_with_number() {
        assert!(matches!(parse_duration("10x"), Err(Error::Config(_))));
    }
}
