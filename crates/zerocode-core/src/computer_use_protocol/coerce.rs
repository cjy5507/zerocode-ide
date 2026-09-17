//! `set-value` writes a STRING the agent typed into a field whose value may be
//! a number or a boolean. `AttributeValueCoercion` from the macOS helper:
//! read the field's current kind, coerce the request to that kind (exactly —
//! a decimal that is really an integer stays an integer above double
//! precision), write, read back, and say whether what came back is what was
//! asked.

/// The kinds a settable value comes in.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeValue {
    String(String),
    Integer(i64),
    Double(f64),
    Boolean(bool),
}

impl AttributeValue {
    /// The text a verification shows for this value.
    #[must_use]
    pub fn preview(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Integer(value) => value.to_string(),
            Self::Double(value) => format_double(*value),
            Self::Boolean(value) => value.to_string(),
        }
    }
}

/// Swift's `String(Double)`: `3.0` prints as "3.0", `2.5` as "2.5".
fn format_double(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e16 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

/// What a read-back said against the value written.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadbackComparison {
    Match { actual_preview: String },
    Mismatch { actual_preview: String },
    Unsupported,
}

/// The decision for one `set-value`.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueCoercion {
    pub write_value: AttributeValue,
}

impl ValueCoercion {
    /// Coerce `requested` to the kind of `existing`; an unreadable or
    /// unhandled existing value means "write the string".
    #[must_use]
    pub fn new(existing: Option<&AttributeValue>, requested: &str) -> Self {
        let write_value = existing
            .and_then(|current| coerce(requested, current))
            .unwrap_or_else(|| AttributeValue::String(requested.to_string()));
        Self { write_value }
    }

    #[must_use]
    pub fn compare(&self, readback: Option<&AttributeValue>) -> ReadbackComparison {
        let Some(actual) = readback else {
            return ReadbackComparison::Unsupported;
        };
        if *actual == self.write_value {
            ReadbackComparison::Match {
                actual_preview: actual.preview(),
            }
        } else {
            ReadbackComparison::Mismatch {
                actual_preview: actual.preview(),
            }
        }
    }
}

fn coerce(requested: &str, current: &AttributeValue) -> Option<AttributeValue> {
    let trimmed = requested.trim();
    match current {
        AttributeValue::String(_) => Some(AttributeValue::String(requested.to_string())),
        AttributeValue::Integer(_) => parse_integer(trimmed).map(AttributeValue::Integer),
        AttributeValue::Double(_) => trimmed
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(AttributeValue::Double),
        AttributeValue::Boolean(_) => match trimmed.to_lowercase().as_str() {
            "true" | "1" => Some(AttributeValue::Boolean(true)),
            "false" | "0" => Some(AttributeValue::Boolean(false)),
            _ => None,
        },
    }
}

/// An integer written as an integer, a decimal, or scientific notation —
/// exactly, without going through a double. `None` when the value is not
/// integral or does not fit an `i64`.
#[must_use]
pub fn parse_integer(value: &str) -> Option<i64> {
    if let Ok(integer) = value.parse::<i64>() {
        return Some(integer);
    }
    let mut unsigned = value;
    let is_negative = unsigned.starts_with('-');
    if is_negative || unsigned.starts_with('+') {
        unsigned = &unsigned[1..];
    }
    let exponent_index = unsigned.find(['e', 'E']);
    let significand = exponent_index.map_or(unsigned, |index| &unsigned[..index]);
    let exponent_text = exponent_index.map(|index| &unsigned[index + 1..]);
    if exponent_text.is_some_and(|text| text.contains(['e', 'E'])) {
        return None;
    }
    let exponent: Option<i64> = match exponent_text {
        Some(text) => {
            let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            // An exponent too large to parse is `None` here and the value is
            // decided below: all-zero digits are still zero.
            text.parse::<i64>().ok()
        }
        None => Some(0),
    };
    let mut parts = significand.splitn(3, '.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next().unwrap_or_default();
    if parts.next().is_some() {
        return None;
    }
    if (whole.is_empty() && fraction.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{whole}{fraction}");
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return Some(0);
    }
    let exponent = exponent?;
    let trailing_zero_count = significant
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'0')
        .count();
    let fraction_len = fraction.len() as i64;
    let normalized = if exponent >= fraction_len {
        let zero_count = exponent - fraction_len;
        if significant.len() > 19 || zero_count > 19 - significant.len() as i64 {
            return None;
        }
        format!("{significant}{}", "0".repeat(zero_count as usize))
    } else {
        let removed_count = if exponent >= 0 {
            fraction_len - exponent
        } else {
            if fraction_len > trailing_zero_count as i64
                || exponent.unsigned_abs() > (trailing_zero_count as i64 - fraction_len) as u64
            {
                return None;
            }
            fraction_len + exponent.unsigned_abs() as i64
        };
        if removed_count > trailing_zero_count as i64 {
            return None;
        }
        significant[..significant.len() - removed_count as usize].to_string()
    };
    if is_negative {
        format!("-{normalized}").parse().ok()
    } else {
        normalized.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(value: &str) -> AttributeValue {
        AttributeValue::String(value.into())
    }

    // ---- AttributeValueCoercionTests.swift, case for case ----

    #[test]
    fn a_string_value_remains_a_string_without_trimming() {
        let coercion = ValueCoercion::new(Some(&string("old")), "  new  ");
        assert_eq!(coercion.write_value, string("  new  "));
    }

    #[test]
    fn an_integer_value_preserves_integer_kind_exactly() {
        assert_eq!(
            ValueCoercion::new(Some(&AttributeValue::Integer(2)), "3.0").write_value,
            AttributeValue::Integer(3)
        );
        let coercion = ValueCoercion::new(Some(&AttributeValue::Integer(1)), "9007199254740993.0");
        assert_eq!(
            coercion.write_value,
            AttributeValue::Integer(9_007_199_254_740_993)
        );
        assert_eq!(
            coercion.compare(Some(&AttributeValue::Integer(9_007_199_254_740_992))),
            ReadbackComparison::Mismatch {
                actual_preview: "9007199254740992".into()
            }
        );
    }

    #[test]
    fn integer_scientific_notation_is_parsed_exactly() {
        for (requested, expected) in [
            ("1e3", 1_000),
            ("9.007199254740993e15", 9_007_199_254_740_993),
            ("10.0e-1", 1),
            ("+0.000E999999999999999999999", 0),
            ("-.0e-999999999999999999999", 0),
        ] {
            assert_eq!(
                ValueCoercion::new(Some(&AttributeValue::Integer(1)), requested).write_value,
                AttributeValue::Integer(expected),
                "{requested}"
            );
        }
        assert_eq!(
            ValueCoercion::new(Some(&AttributeValue::Integer(1)), "9223372036854775807.0")
                .write_value,
            AttributeValue::Integer(i64::MAX)
        );
        assert_eq!(
            ValueCoercion::new(
                Some(&AttributeValue::Integer(1)),
                "-9.223372036854775808e18"
            )
            .write_value,
            AttributeValue::Integer(i64::MIN)
        );
    }

    #[test]
    fn non_integral_and_out_of_range_integer_forms_use_the_string_fallback() {
        for requested in [
            "1e-1",
            "9223372036854775808",
            "-9223372036854775809",
            "1e999999999999999999999",
        ] {
            assert_eq!(
                ValueCoercion::new(Some(&AttributeValue::Integer(1)), requested).write_value,
                string(requested),
                "{requested}"
            );
        }
    }

    #[test]
    fn doubles_and_booleans_accept_their_aliases() {
        assert_eq!(
            ValueCoercion::new(Some(&AttributeValue::Double(2.5)), "3").write_value,
            AttributeValue::Double(3.0)
        );
        assert_eq!(
            ValueCoercion::new(Some(&AttributeValue::Boolean(false)), "TRUE").write_value,
            AttributeValue::Boolean(true)
        );
        assert_eq!(
            ValueCoercion::new(Some(&AttributeValue::Boolean(true)), "0").write_value,
            AttributeValue::Boolean(false)
        );
    }

    #[test]
    fn unreadable_and_invalid_values_use_the_string_fallback() {
        assert_eq!(ValueCoercion::new(None, "7").write_value, string("7"));
        assert_eq!(
            ValueCoercion::new(Some(&AttributeValue::Integer(2)), "2.5").write_value,
            string("2.5")
        );
        assert_eq!(
            ValueCoercion::new(Some(&AttributeValue::Double(2.5)), "many").write_value,
            string("many")
        );
        assert_eq!(
            ValueCoercion::new(Some(&AttributeValue::Boolean(false)), "maybe").write_value,
            string("maybe")
        );
    }

    #[test]
    fn readback_matches_each_supported_kind() {
        for (coercion, readback, preview) in [
            (
                ValueCoercion::new(Some(&string("old")), "new"),
                string("new"),
                "new",
            ),
            (
                ValueCoercion::new(Some(&AttributeValue::Integer(1)), "2"),
                AttributeValue::Integer(2),
                "2",
            ),
            (
                ValueCoercion::new(Some(&AttributeValue::Double(1.5)), "2.5"),
                AttributeValue::Double(2.5),
                "2.5",
            ),
            (
                ValueCoercion::new(Some(&AttributeValue::Boolean(false)), "true"),
                AttributeValue::Boolean(true),
                "true",
            ),
        ] {
            assert_eq!(
                coercion.compare(Some(&readback)),
                ReadbackComparison::Match {
                    actual_preview: preview.into()
                }
            );
        }
        let coercion = ValueCoercion::new(Some(&AttributeValue::Integer(1)), "2");
        assert_eq!(
            coercion.compare(Some(&AttributeValue::Integer(3))),
            ReadbackComparison::Mismatch {
                actual_preview: "3".into()
            }
        );
        assert_eq!(coercion.compare(None), ReadbackComparison::Unsupported);
        assert_eq!(AttributeValue::Double(3.0).preview(), "3.0");
    }
}
