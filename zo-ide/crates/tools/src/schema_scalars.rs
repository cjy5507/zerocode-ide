//! Putting back the scalar types a tool's own schema declares (t-68770).
//!
//! A deferred tool's schema never reaches the provider. The model calls it by
//! name, or through `CapabilityInvoke`, whose `input` is a bare `object` on the
//! wire — so nothing upstream holds a `boolean` or `integer` argument to its
//! type, and a model writes `"local": "false"` or `"duration_ms": "1"` where
//! the schema says `false` and `1`. serde then rejects the call
//! (`invalid type: string "false", expected a boolean`) before the tool runs.
//! zo itself carries JSON types through unchanged — the wrapper hands the
//! inner `Value` on as it came (`capability::unwrap_capability_invoke`) — so
//! the dispatch seam, the first place that holds the tool's schema, is where
//! the type goes back.
//!
//! Only what the schema says, and only the unambiguous spelling of it:
//!
//! * `boolean`: the words `true` and `false`, in any case, with surrounding
//!   whitespace.
//! * `integer`: a decimal integer — `-` at most, then digits — that fits 64
//!   bits and the schema's own `minimum`/`maximum`. The field's Rust type
//!   still checks its range, exactly as it would for a JSON number.
//!
//! A property whose type also admits a string keeps its string, and anything
//! else — a value outside the declared bounds included — reaches the tool as
//! sent, so the tool's own validation names the real mistake.

use serde_json::{Number, Value};

/// `input` with every string the schema types as `boolean` or `integer`
/// turned into that type, or `None` when nothing needed it — the common case,
/// which costs one read of the input and no copy of it.
#[must_use]
pub(crate) fn restore_declared_scalars(schema: &Value, input: &Value) -> Option<Value> {
    match input {
        Value::String(text) => {
            declared_scalar(schema).and_then(|scalar| scalar.parse(schema, text))
        }
        Value::Object(fields) => {
            let properties = schema.get("properties")?.as_object()?;
            let mut restored: Option<serde_json::Map<String, Value>> = None;
            for (key, field) in fields {
                let Some(field_schema) = properties.get(key) else {
                    continue;
                };
                if let Some(value) = restore_declared_scalars(field_schema, field) {
                    restored.get_or_insert_with(|| fields.clone()).insert(key.clone(), value);
                }
            }
            restored.map(Value::Object)
        }
        Value::Array(items) => {
            // Tuple validation (`items: [..]`) names no single schema for an
            // element, so only the one-schema form is followed.
            let item_schema = schema.get("items").filter(|items| items.is_object())?;
            let mut restored: Option<Vec<Value>> = None;
            for (index, item) in items.iter().enumerate() {
                if let Some(value) = restore_declared_scalars(item_schema, item) {
                    restored.get_or_insert_with(|| items.clone())[index] = value;
                }
            }
            restored.map(Value::Array)
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
enum Scalar {
    Boolean,
    Integer,
}

impl Scalar {
    fn parse(self, schema: &Value, text: &str) -> Option<Value> {
        let text = text.trim();
        match self {
            Self::Boolean if text.eq_ignore_ascii_case("true") => Some(Value::Bool(true)),
            Self::Boolean if text.eq_ignore_ascii_case("false") => Some(Value::Bool(false)),
            Self::Boolean => None,
            Self::Integer => decimal_integer(text)
                .filter(|value| within_declared_bounds(schema, *value))
                .and_then(sixty_four_bit_number)
                .map(Value::Number),
        }
    }
}

/// The one scalar type a property admits, `null` aside: `"boolean"` or
/// `"integer"`, alone or in a type list beside `"null"` only. A list that also
/// admits a string — or anything else — is not one of these.
fn declared_scalar(schema: &Value) -> Option<Scalar> {
    let name = match schema.get("type")? {
        Value::String(name) => name.as_str(),
        Value::Array(names) => {
            let mut admitted = names.iter().filter(|name| name.as_str() != Some("null"));
            let only = admitted.next()?.as_str()?;
            if admitted.next().is_some() {
                return None;
            }
            only
        }
        _ => return None,
    };
    match name {
        "boolean" => Some(Scalar::Boolean),
        "integer" => Some(Scalar::Integer),
        _ => None,
    }
}

/// A decimal integer as it is written by hand (`12`, `-3`, `007`): no `+`,
/// fraction, exponent or separator. Wide enough to hold every 64-bit value of
/// either sign, so the bounds and the fit are checked on the real number.
fn decimal_integer(text: &str) -> Option<i128> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<i128>().ok()
}

/// Whether `value` meets the integer `minimum`/`maximum` the schema declares.
/// A bound that is absent, or not an integer, constrains nothing here.
fn within_declared_bounds(schema: &Value, value: i128) -> bool {
    let bound = |key: &str| {
        schema
            .get(key)
            .and_then(|bound| bound.as_i64().map(i128::from).or_else(|| bound.as_u64().map(i128::from)))
    };
    bound("minimum").is_none_or(|minimum| value >= minimum)
        && bound("maximum").is_none_or(|maximum| value <= maximum)
}

/// The JSON number for `value`, when it fits 64 bits of either sign.
fn sixty_four_bit_number(value: i128) -> Option<Number> {
    i64::try_from(value)
        .map(Number::from)
        .or_else(|_| u64::try_from(value).map(Number::from))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::restore_declared_scalars;
    use serde_json::json;

    fn schema() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "flag": {"type": "boolean"},
                "maybe": {"type": ["boolean", "null"]},
                "either": {"type": ["boolean", "string"]},
                "count": {"type": "integer", "minimum": 0},
                "label": {"type": "string"},
                "steps": {
                    "type": "array",
                    "items": {"type": "object", "properties": {"force": {"type": "boolean"}}}
                },
                "nested": {"type": "object", "properties": {"limit": {"type": "integer"}}}
            }
        })
    }

    #[test]
    fn the_words_and_digits_a_schema_types_come_back_typed() {
        let restored = restore_declared_scalars(
            &schema(),
            &json!({
                "flag": " FALSE ",
                "maybe": "true",
                "count": "12",
                "steps": [{"force": "true"}, {"force": false}],
                "nested": {"limit": "-3"},
                "label": "true"
            }),
        )
        .expect("something to restore");
        assert_eq!(
            restored,
            json!({
                "flag": false,
                "maybe": true,
                "count": 12,
                "steps": [{"force": true}, {"force": false}],
                "nested": {"limit": -3},
                "label": "true"
            })
        );
    }

    /// Nothing to put back is the common case: no copy is made, and the
    /// caller dispatches the input it already holds.
    #[test]
    fn a_typed_input_needs_nothing() {
        let input = json!({"flag": true, "count": 3, "label": "x", "steps": [{"force": true}]});
        assert_eq!(restore_declared_scalars(&schema(), &input), None);
        assert_eq!(restore_declared_scalars(&schema(), &json!({"unknown": "true"})), None);
        assert_eq!(restore_declared_scalars(&json!({"type": "object"}), &json!({"flag": "true"})), None);
    }

    /// Only the unambiguous spelling comes back; everything else stays as
    /// sent, so the tool's own error names it.
    #[test]
    fn anything_but_the_plain_spelling_stays_as_sent() {
        for (field, value) in [
            ("flag", json!("yes")),
            ("flag", json!("1")),
            ("flag", json!("")),
            ("either", json!("true")),
            ("count", json!("1.5")),
            ("count", json!("-1")),
            ("count", json!("+1")),
            ("count", json!("1e3")),
            ("count", json!("1_000")),
            ("count", json!("-")),
            ("count", json!("99999999999999999999")),
            ("count", json!("twelve")),
        ] {
            let input = json!({ field: value });
            assert_eq!(restore_declared_scalars(&schema(), &input), None, "{input}");
        }
    }

    /// Either end of 64 bits still fits; past them, or past a bound the
    /// schema declares, the string stays. Inside both, the field's own Rust
    /// type decides the rest, as it would for a typed number.
    #[test]
    fn a_decimal_inside_the_declared_bounds_and_sixty_four_bits_comes_back_as_a_number() {
        let restored = restore_declared_scalars(&schema(), &json!({"count": "18446744073709551615"}));
        assert_eq!(restored, Some(json!({"count": u64::MAX})));
        let restored =
            restore_declared_scalars(&schema(), &json!({"nested": {"limit": "-9223372036854775808"}}));
        assert_eq!(restored, Some(json!({"nested": {"limit": i64::MIN}})));
        let bounded = json!({"type": "object", "properties": {"n": {"type": "integer", "minimum": 1, "maximum": 100}}});
        assert_eq!(restore_declared_scalars(&bounded, &json!({"n": "100"})), Some(json!({"n": 100})));
        assert_eq!(restore_declared_scalars(&bounded, &json!({"n": "101"})), None);
        assert_eq!(restore_declared_scalars(&bounded, &json!({"n": "0"})), None);
    }

    /// Every boolean and integer a registered builtin declares at the top of
    /// its schema is put back through the same seam — the road `MemoryWrite`,
    /// `SendMessage`, `session_recall`, `Sleep` and the rest share.
    #[test]
    fn every_builtin_top_level_boolean_and_integer_is_covered() {
        let mut covered = 0_usize;
        for spec in crate::mvp_tool_specs() {
            let Some(properties) = spec
                .input_schema
                .get("properties")
                .and_then(serde_json::Value::as_object)
            else {
                continue;
            };
            for (name, property) in properties {
                let spelled = match property.get("type").and_then(serde_json::Value::as_str) {
                    Some("boolean") => json!("true"),
                    // The schema's own minimum is inside its bounds by definition.
                    Some("integer") => {
                        let floor = property.get("minimum").and_then(serde_json::Value::as_i64);
                        json!(floor.unwrap_or(1).to_string())
                    }
                    _ => continue,
                };
                let restored = restore_declared_scalars(&spec.input_schema, &json!({ name.as_str(): spelled }))
                    .unwrap_or_else(|| panic!("{}.{name} was not restored", spec.name));
                assert!(
                    restored[name.as_str()].is_boolean() || restored[name.as_str()].is_i64(),
                    "{}.{name}: {restored}",
                    spec.name
                );
                covered += 1;
            }
        }
        assert!(covered > 20, "the scan stopped seeing the schemas: {covered} covered");
    }
}
