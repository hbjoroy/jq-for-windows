use crate::{BinaryOp, Builtin, EvalError, Format, UnaryOp};
use base64::Engine;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value};

const URI_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'=')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b']');
pub(super) fn length(input: &Value) -> Result<Value, EvalError> {
    let length = match input {
        Value::Null => 0,
        Value::String(text) => text.chars().count(),
        Value::Array(items) => items.len(),
        Value::Object(map) => map.len(),
        Value::Number(number) => {
            return number_value(number.as_f64().unwrap_or_default().abs());
        }
        Value::Bool(_) => {
            return Err(EvalError::InvalidBuiltin {
                name: "length",
                input: type_name(input),
            });
        }
    };
    Ok(Value::Number((length as u64).into()))
}

pub(super) fn keys(input: &Value) -> Result<Value, EvalError> {
    match input {
        Value::Array(items) => Ok(Value::Array(
            (0..items.len())
                .map(|index| Value::Number((index as u64).into()))
                .collect(),
        )),
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().cloned().collect();
            keys.sort_unstable();
            Ok(Value::Array(keys.into_iter().map(Value::String).collect()))
        }
        _ => Err(EvalError::InvalidBuiltin {
            name: "keys",
            input: type_name(input),
        }),
    }
}

pub(super) fn has(input: &Value, key: &Value) -> Result<bool, EvalError> {
    match (input, key) {
        (Value::Object(map), Value::String(key)) => Ok(map.contains_key(key)),
        (Value::Array(items), Value::Number(index)) => {
            let index = index.as_i64().ok_or(EvalError::InvalidBuiltin {
                name: "has",
                input: "non-integer index",
            })?;
            Ok(normalized_index(index, items.len()).is_some_and(|index| index < items.len()))
        }
        _ => Err(EvalError::InvalidBuiltin {
            name: "has",
            input: type_name(input),
        }),
    }
}

pub(super) fn iterate(input: &Value) -> Result<Vec<Value>, EvalError> {
    match input {
        Value::Array(items) => Ok(items.clone()),
        Value::Object(map) => Ok(map.values().cloned().collect()),
        _ => Err(EvalError::InvalidIteration {
            input: type_name(input),
        }),
    }
}

pub(super) fn to_string_value(input: &Value) -> Result<Value, EvalError> {
    match input {
        Value::String(text) => Ok(Value::String(text.clone())),
        value => serde_json::to_string(value)
            .map(Value::String)
            .map_err(|error| EvalError::Serialization(error.to_string())),
    }
}

pub(super) fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub(super) fn is_truthy(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false))
}

pub(super) fn apply_unary(operator: UnaryOp, value: Value) -> Result<Value, EvalError> {
    match operator {
        UnaryOp::Not => Ok(Value::Bool(!is_truthy(&value))),
        UnaryOp::Negate => value
            .as_f64()
            .ok_or(EvalError::InvalidUnary {
                input: type_name(&value),
                operation: "negation",
            })
            .and_then(|number| number_value(-number)),
    }
}

pub(super) fn apply_binary(
    operator: BinaryOp,
    left: &Value,
    right: &Value,
) -> Result<Value, EvalError> {
    use BinaryOp::*;
    match operator {
        Equal => Ok(Value::Bool(left == right)),
        NotEqual => Ok(Value::Bool(left != right)),
        And => Ok(Value::Bool(is_truthy(left) && is_truthy(right))),
        Or => Ok(Value::Bool(is_truthy(left) || is_truthy(right))),
        Less | LessEqual | Greater | GreaterEqual => {
            let ordering = compare_values(left, right);
            Ok(Value::Bool(match operator {
                Less => ordering.is_lt(),
                LessEqual => !ordering.is_gt(),
                Greater => ordering.is_gt(),
                GreaterEqual => !ordering.is_lt(),
                _ => unreachable!(),
            }))
        }
        Add => match (left, right) {
            (Value::Null, value) | (value, Value::Null) => Ok(value.clone()),
            (Value::Number(a), Value::Number(b)) => {
                number_value(a.as_f64().unwrap_or_default() + b.as_f64().unwrap_or_default())
            }
            (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{a}{b}"))),
            (Value::Array(a), Value::Array(b)) => {
                Ok(Value::Array(a.iter().chain(b).cloned().collect()))
            }
            (Value::Object(a), Value::Object(b)) => {
                let mut result = a.clone();
                result.extend(b.clone());
                Ok(Value::Object(result))
            }
            _ => incompatible(operator, left, right),
        },
        Subtract => match (left, right) {
            (Value::Number(a), Value::Number(b)) => {
                number_value(a.as_f64().unwrap_or_default() - b.as_f64().unwrap_or_default())
            }
            (Value::Array(a), Value::Array(b)) => Ok(Value::Array(
                a.iter().filter(|v| !b.contains(v)).cloned().collect(),
            )),
            _ => incompatible(operator, left, right),
        },
        Multiply | Divide | Remainder => {
            let (Some(a), Some(b)) = (left.as_f64(), right.as_f64()) else {
                return incompatible(operator, left, right);
            };
            if matches!(operator, Divide | Remainder) && b == 0.0 {
                return Err(EvalError::DivisionByZero);
            }
            number_value(match operator {
                Multiply => a * b,
                Divide => a / b,
                Remainder => a % b,
                _ => unreachable!(),
            })
        }
        Alternative => unreachable!(),
    }
}

pub(super) fn number_value(number: f64) -> Result<Value, EvalError> {
    if number.fract() == 0.0 && number >= i64::MIN as f64 && number <= i64::MAX as f64 {
        return Ok(Value::Number((number as i64).into()));
    }
    serde_json::Number::from_f64(number)
        .map(Value::Number)
        .ok_or(EvalError::InvalidNumber)
}

fn incompatible<T>(operator: BinaryOp, left: &Value, right: &Value) -> Result<T, EvalError> {
    Err(EvalError::InvalidBinary {
        left: type_name(left),
        right: type_name(right),
        operation: operator,
    })
}

pub(super) fn compare_values(left: &Value, right: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let rank = |value: &Value| match value {
        Value::Null => 0,
        Value::Bool(false) => 1,
        Value::Bool(true) => 2,
        Value::Number(_) => 3,
        Value::String(_) => 4,
        Value::Array(_) => 5,
        Value::Object(_) => 6,
    };
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a
            .as_f64()
            .partial_cmp(&b.as_f64())
            .unwrap_or(Ordering::Equal),
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::Array(a), Value::Array(b)) => compare_slices(a, b),
        (Value::Object(a), Value::Object(b)) => {
            let mut a: Vec<_> = a.iter().collect();
            let mut b: Vec<_> = b.iter().collect();
            a.sort_unstable_by_key(|(key, _)| *key);
            b.sort_unstable_by_key(|(key, _)| *key);
            for ((a_key, a_value), (b_key, b_value)) in a.iter().zip(&b) {
                let ordering = a_key
                    .cmp(b_key)
                    .then_with(|| compare_values(a_value, b_value));
                if !ordering.is_eq() {
                    return ordering;
                }
            }
            a.len().cmp(&b.len())
        }
        _ => rank(left).cmp(&rank(right)),
    }
}

fn compare_slices(left: &[Value], right: &[Value]) -> std::cmp::Ordering {
    for (left, right) in left.iter().zip(right) {
        let ordering = compare_values(left, right);
        if !ordering.is_eq() {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

pub(super) fn builtin_name(builtin: Builtin) -> &'static str {
    match builtin {
        Builtin::SortBy => "sort_by",
        Builtin::GroupBy => "group_by",
        Builtin::UniqueBy => "unique_by",
        Builtin::Min => "min",
        Builtin::Max => "max",
        Builtin::StartsWith => "startswith",
        Builtin::EndsWith => "endswith",
        Builtin::Split => "split",
        _ => "builtin",
    }
}

pub(super) fn array_items<'a>(
    input: &'a Value,
    name: &'static str,
) -> Result<&'a [Value], EvalError> {
    input
        .as_array()
        .map(Vec::as_slice)
        .ok_or(EvalError::InvalidBuiltin {
            name,
            input: type_name(input),
        })
}

pub(super) fn sort_values(input: &Value) -> Result<Value, EvalError> {
    let mut items = array_items(input, "sort")?.to_vec();
    items.sort_by(compare_values);
    Ok(Value::Array(items))
}

pub(super) fn deduplicate(mut items: Vec<Value>) -> Vec<Value> {
    items.dedup();
    items
}

pub(super) fn flatten(input: &Value, depth: usize) -> Result<Value, EvalError> {
    let items = match input {
        Value::Array(items) => items.iter().collect::<Vec<_>>(),
        Value::Object(object) => object.values().collect::<Vec<_>>(),
        _ => {
            return Err(EvalError::InvalidBuiltin {
                name: "flatten",
                input: type_name(input),
            });
        }
    };
    let mut output = Vec::new();
    for item in items {
        if depth > 0 && item.is_array() {
            let Value::Array(flattened) = flatten(item, depth - 1)? else {
                return Err(EvalError::InvalidBuiltin {
                    name: "flatten",
                    input: type_name(item),
                });
            };
            output.extend(flattened);
        } else {
            output.push((*item).clone());
        }
    }
    Ok(Value::Array(output))
}

pub(super) fn contains(container: &Value, candidate: &Value) -> bool {
    match (container, candidate) {
        (Value::String(text), Value::String(part)) => text.contains(part),
        (Value::Array(items), Value::Array(expected)) => expected
            .iter()
            .all(|expected| items.iter().any(|item| contains(item, expected))),
        (Value::Object(map), Value::Object(expected)) => expected
            .iter()
            .all(|(key, expected)| map.get(key).is_some_and(|value| contains(value, expected))),
        _ => container == candidate,
    }
}

pub(super) fn join_part(value: &Value) -> Result<String, EvalError> {
    match value {
        Value::Null => Ok(String::new()),
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        _ => Err(EvalError::InvalidBuiltin {
            name: "join",
            input: type_name(value),
        }),
    }
}

pub(super) fn build_regex(pattern: &str, flags: &str) -> Result<Regex, EvalError> {
    let mut builder = RegexBuilder::new(pattern);
    builder
        .case_insensitive(flags.contains('i'))
        .multi_line(flags.contains('m'))
        .dot_matches_new_line(flags.contains('s'))
        .ignore_whitespace(flags.contains('x'));
    builder
        .build()
        .map_err(|error| EvalError::InvalidRegex(error.to_string()))
}

pub(super) fn match_object(text: &str, regex: &Regex, captures: &regex::Captures<'_>) -> Value {
    let Some(whole) = captures.get(0) else {
        return Value::Null;
    };
    let offset = text[..whole.start()].chars().count();
    let length = whole.as_str().chars().count();
    let mut capture_values = Vec::new();
    for index in 1..captures.len() {
        let value = captures.get(index);
        let mut capture = Map::new();
        capture.insert(
            "offset".to_owned(),
            value
                .map(|value| Value::Number((text[..value.start()].chars().count() as u64).into()))
                .unwrap_or_else(|| Value::Number((-1_i64).into())),
        );
        capture.insert(
            "length".to_owned(),
            value
                .map(|value| Value::Number((value.as_str().chars().count() as u64).into()))
                .unwrap_or(Value::Number(0_u64.into())),
        );
        capture.insert(
            "string".to_owned(),
            value
                .map(|value| Value::String(value.as_str().to_owned()))
                .unwrap_or(Value::Null),
        );
        capture.insert(
            "name".to_owned(),
            regex
                .capture_names()
                .nth(index)
                .flatten()
                .map(|name| Value::String(name.to_owned()))
                .unwrap_or(Value::Null),
        );
        capture_values.push(Value::Object(capture));
    }
    let mut object = Map::new();
    object.insert("offset".to_owned(), Value::Number((offset as u64).into()));
    object.insert("length".to_owned(), Value::Number((length as u64).into()));
    object.insert(
        "string".to_owned(),
        Value::String(whole.as_str().to_owned()),
    );
    object.insert("captures".to_owned(), Value::Array(capture_values));
    Value::Object(object)
}

pub(super) fn format_value(format: Format, input: &Value) -> Result<Value, EvalError> {
    let text = match format {
        Format::Json => serde_json::to_string(input)
            .map_err(|error| EvalError::Serialization(error.to_string()))?,
        Format::Uri => {
            let text = input.as_str().ok_or(EvalError::InvalidBuiltin {
                name: "@uri",
                input: type_name(input),
            })?;
            utf8_percent_encode(text, URI_ENCODE_SET).to_string()
        }
        Format::Base64 => base64::engine::general_purpose::STANDARD.encode(
            input
                .as_str()
                .ok_or(EvalError::InvalidBuiltin {
                    name: "@base64",
                    input: type_name(input),
                })?
                .as_bytes(),
        ),
        Format::Csv => format_delimited(input, ',', true)?,
        Format::Tsv => format_delimited(input, '\t', false)?,
    };
    Ok(Value::String(text))
}

fn format_delimited(input: &Value, separator: char, csv: bool) -> Result<String, EvalError> {
    let items = array_items(input, if csv { "@csv" } else { "@tsv" })?;
    items
        .iter()
        .map(|value| {
            if csv {
                match value {
                    Value::String(text) => Ok(format!("\"{}\"", text.replace('"', "\"\""))),
                    Value::Null => Ok(String::new()),
                    _ => join_part(value),
                }
            } else {
                let text = join_part(value)?;
                Ok(text
                    .replace('\\', "\\\\")
                    .replace('\t', "\\t")
                    .replace('\r', "\\r")
                    .replace('\n', "\\n"))
            }
        })
        .collect::<Result<Vec<_>, EvalError>>()
        .map(|parts| parts.join(&separator.to_string()))
}

pub(super) fn index_value(input: &Value, index: i64) -> Value {
    match input {
        Value::Array(items) => normalized_index(index, items.len())
            .and_then(|i| items.get(i))
            .cloned()
            .unwrap_or(Value::Null),
        Value::Object(map) => map.get(&index.to_string()).cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

pub(super) fn slice_value(input: &Value, start: Option<i64>, end: Option<i64>) -> Value {
    match input {
        Value::Array(items) => {
            let (start, end) = slice_bounds(start, end, items.len());
            Value::Array(items[start..end].to_vec())
        }
        Value::String(text) => {
            let chars: Vec<_> = text.chars().collect();
            let (start, end) = slice_bounds(start, end, chars.len());
            Value::String(chars[start..end].iter().collect())
        }
        _ => Value::Null,
    }
}

fn slice_bounds(start: Option<i64>, end: Option<i64>, len: usize) -> (usize, usize) {
    let bound = |value: i64| {
        if value < 0 {
            (len as i64 + value).max(0) as usize
        } else {
            (value as usize).min(len)
        }
    };
    let start = start.map(bound).unwrap_or(0);
    let end = end.map(bound).unwrap_or(len).max(start);
    (start, end)
}

pub(super) fn normalized_index(index: i64, len: usize) -> Option<usize> {
    let index = if index < 0 { len as i64 + index } else { index };
    (index >= 0).then_some(index as usize)
}
