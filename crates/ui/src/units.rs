//! Durations as people read and type them: "30 s", "500 ms", "2 min".
//! Files keep milliseconds; only the text in the interface changes.

/// A duration in milliseconds as the largest whole unit: `30000` reads
/// "30 s", `120000` reads "2 min", `1500` reads "1500 ms".
pub fn format_ms(ms: u64) -> String {
    if ms >= 60_000 && ms.is_multiple_of(60_000) {
        format!("{} min", ms / 60_000)
    } else if ms >= 1000 && ms.is_multiple_of(1000) {
        format!("{} s", ms / 1000)
    } else {
        format!("{ms} ms")
    }
}

/// A typed duration in milliseconds. A bare number is milliseconds; the
/// units `ms`, `s`, `m` and `min` are accepted, with or without a space.
/// `None` when the text is not a duration.
pub fn parse_ms(text: &str) -> Option<u64> {
    let t = text.trim().to_ascii_lowercase();
    let split = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let n: u64 = num.parse().ok()?;
    let factor = match unit.trim() {
        "" | "ms" => 1,
        "s" | "sec" => 1000,
        "m" | "min" => 60_000,
        _ => return None,
    };
    n.checked_mul(factor)
}

/// An optional duration as the text of a field: empty when it is not set.
pub fn ms_text(v: Option<u64>) -> String {
    v.map(format_ms).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_the_largest_whole_unit() {
        assert_eq!(format_ms(0), "0 ms");
        assert_eq!(format_ms(500), "500 ms");
        assert_eq!(format_ms(1500), "1500 ms");
        assert_eq!(format_ms(30_000), "30 s");
        assert_eq!(format_ms(90_000), "90 s");
        assert_eq!(format_ms(120_000), "2 min");
    }

    #[test]
    fn parses_numbers_and_units() {
        assert_eq!(parse_ms("5000"), Some(5000));
        assert_eq!(parse_ms(" 30s "), Some(30_000));
        assert_eq!(parse_ms("30 s"), Some(30_000));
        assert_eq!(parse_ms("500ms"), Some(500));
        assert_eq!(parse_ms("2 MIN"), Some(120_000));
        assert_eq!(parse_ms("1m"), Some(60_000));
        assert_eq!(parse_ms(""), None);
        assert_eq!(parse_ms("s"), None);
        assert_eq!(parse_ms("5 h"), None);
        assert_eq!(parse_ms("1.5s"), None);
        assert_eq!(parse_ms("99999999999999999999"), None);
    }

    #[test]
    fn every_formatted_value_parses_back() {
        for ms in [0, 1, 999, 1000, 1500, 59_000, 60_000, 61_000, 3_600_000] {
            assert_eq!(parse_ms(&format_ms(ms)), Some(ms), "{ms}");
        }
    }
}
