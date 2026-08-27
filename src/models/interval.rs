//! The interval literal a human types into a cadence field.
//!
//! One grammar for every such field — today that is the epic editor's
//! `FEED_INTERVAL_SECS` section — because two spellings of ten minutes in one
//! application is a bug waiting to be filed. See "Interval literals" in
//! `docs/specs/core.allium` for the normative statement.
//!
//! Not used by the MCP/CLI surfaces: those take a JSON integer of seconds and
//! never a string. This grammar is for humans typing into a field.

/// The units a literal may carry, largest first.
///
/// Scanned by [`parse_interval_secs`] for a typed suffix. One table, so a unit
/// is worth the same thing wherever it is read.
const UNITS: [(char, i64); 4] = [('d', 86_400), ('h', 3600), ('m', 60), ('s', 1)];

/// The example set every user-facing surface quotes when describing this
/// grammar — today, the editor section's parse-error message.
///
/// Exported rather than retyped, because a grammar described differently at
/// each surface is how one ends up advertising a suffix the parser does not
/// implement. Adding a unit is then a change to [`UNITS`] and this line, not a
/// grep.
pub const INTERVAL_EXAMPLES: &str = "600, 10m, 2h, 1d";

/// Parse an interval literal into a strictly-positive number of seconds.
///
/// Accepts a bare integer (seconds) or an integer with a single `s`/`m`/`h`/`d`
/// suffix, case-insensitively, with surrounding whitespace ignored:
///
/// | input   | result       |
/// |---------|--------------|
/// | `"600"` | `Some(600)`  |
/// | `"600s"`| `Some(600)`  |
/// | `"10m"` | `Some(600)`  |
/// | `"2h"`  | `Some(7200)` |
/// | `"1d"`  | `Some(86400)`|
///
/// Returns `None` for anything else: a bare suffix (`"m"`), a negative
/// (`"-5"`), zero in any unit (`"0"`, `"0m"`), a fraction (`"1.5h"`), a
/// compound (`"1h30m"`), or arbitrary text. Zero is rejected rather than read
/// as "off" because every calling surface already spells "off" as the empty
/// value, and a zero-second cadence would busy-loop the feed runner.
///
/// A bare integer means *seconds* — not because seconds are the friendly unit,
/// but because that is what the column holds (`Epic.feed_interval_secs`) and
/// what the field accepted before suffixes existed. Keeping the bare form's
/// meaning is what makes suffix support purely additive: no value a user typed
/// before changes meaning.
pub fn parse_interval_secs(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    // Matching on the last *byte*, not the last char: an ASCII-alphabetic
    // suffix is one byte wide by definition, so the split needs no width
    // arithmetic. A multi-byte trailing char is not alphabetic here and falls
    // through to the bare-seconds arm, where the digit check rejects it.
    let (digits, multiplier) = match trimmed.as_bytes().last() {
        Some(&last) if last.is_ascii_alphabetic() => {
            // Only the suffix is case-insensitive; digits have no case to fold.
            let suffix = last.to_ascii_lowercase() as char;
            let (_, multiplier) = UNITS.iter().find(|(u, _)| *u == suffix)?;
            (&trimmed[..trimmed.len() - 1], *multiplier)
        }
        _ => (trimmed, 1),
    };
    // Rejects every non-digit body and a leading `+`/`-` — `i64::from_str`
    // would accept the signs, and a negative cadence is not a thing. The bare
    // suffix ("m") lands here too: it leaves an empty digit string, which
    // `parse` below rejects.
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: i64 = digits.parse().ok()?;
    // Overflow is a parse failure, not a saturation: `"9999999999999999999d"`
    // must be refused rather than quietly becoming i64::MAX seconds.
    let secs = value.checked_mul(multiplier)?;
    (secs > 0).then_some(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_interval_secs ---

    /// The pre-suffix behaviour, which every stored value and every previously
    /// typed value depends on: a bare integer is seconds.
    #[test]
    fn a_bare_integer_is_seconds() {
        assert_eq!(parse_interval_secs("600"), Some(600));
        assert_eq!(parse_interval_secs("1"), Some(1));
    }

    #[test]
    fn each_suffix_scales_to_seconds() {
        assert_eq!(parse_interval_secs("600s"), Some(600));
        assert_eq!(parse_interval_secs("10m"), Some(600));
        assert_eq!(parse_interval_secs("2h"), Some(7200));
        assert_eq!(parse_interval_secs("1d"), Some(86_400));
    }

    #[test]
    fn suffixes_are_case_insensitive() {
        assert_eq!(parse_interval_secs("10M"), Some(600));
        assert_eq!(parse_interval_secs("2H"), Some(7200));
        assert_eq!(parse_interval_secs("1D"), Some(86_400));
        assert_eq!(parse_interval_secs("30S"), Some(30));
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(parse_interval_secs("  10m  "), Some(600));
        assert_eq!(parse_interval_secs("\t600\n"), Some(600));
    }

    /// Zero is a parse failure in every unit, not a synonym for "off": the
    /// empty value already means off on every calling surface, and a
    /// zero-second cadence would busy-loop the feed runner.
    #[test]
    fn zero_is_rejected_in_every_unit() {
        assert_eq!(parse_interval_secs("0"), None);
        assert_eq!(parse_interval_secs("0s"), None);
        assert_eq!(parse_interval_secs("0m"), None);
        assert_eq!(parse_interval_secs("0h"), None);
        assert_eq!(parse_interval_secs("0d"), None);
    }

    #[test]
    fn negatives_are_rejected() {
        assert_eq!(parse_interval_secs("-5"), None);
        assert_eq!(parse_interval_secs("-5m"), None);
    }

    /// A leading `+` parses fine via `i64::from_str` and must not: the field
    /// takes a count, not a signed offset.
    #[test]
    fn an_explicit_plus_sign_is_rejected() {
        assert_eq!(parse_interval_secs("+600"), None);
    }

    #[test]
    fn a_bare_suffix_with_no_digits_is_rejected() {
        assert_eq!(parse_interval_secs("m"), None);
        assert_eq!(parse_interval_secs("s"), None);
    }

    #[test]
    fn unknown_suffixes_are_rejected() {
        assert_eq!(parse_interval_secs("10w"), None);
        assert_eq!(parse_interval_secs("10y"), None);
    }

    #[test]
    fn fractions_and_compounds_are_rejected() {
        assert_eq!(parse_interval_secs("1.5h"), None);
        assert_eq!(parse_interval_secs("1h30m"), None);
    }

    #[test]
    fn arbitrary_text_is_rejected() {
        assert_eq!(parse_interval_secs("abc"), None);
        assert_eq!(parse_interval_secs(""), None);
        assert_eq!(parse_interval_secs("   "), None);
        assert_eq!(parse_interval_secs("6 0 0"), None);
    }

    /// Saturating here would turn a nonsense entry into the largest cadence
    /// representable rather than refusing it.
    #[test]
    fn overflow_is_a_parse_failure_not_a_saturation() {
        assert_eq!(parse_interval_secs("9999999999999999999d"), None);
        assert_eq!(parse_interval_secs("999999999999999999999"), None);
    }
}
