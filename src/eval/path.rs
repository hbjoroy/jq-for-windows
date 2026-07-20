use super::value::{apply_binary, normalized_index, type_name};
use crate::{BinaryOp, EvalError, Filter, UpdateOp};
use serde_json::{Map, Value};
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PathPart {
    Key(String),
    Index(i64),
}

pub(super) fn filter_paths(filter: &Filter) -> Result<Vec<Vec<PathPart>>, EvalError> {
    match filter {
        Filter::Identity => Ok(vec![Vec::new()]),
        Filter::Field(name) => Ok(vec![vec![PathPart::Key(name.clone())]]),
        Filter::Index(index) => Ok(vec![vec![PathPart::Index(*index)]]),
        Filter::Optional(inner) => filter_paths(inner),
        Filter::Pipe(left, right) => {
            let mut paths = Vec::new();
            for left in filter_paths(left)? {
                for right in filter_paths(right)? {
                    paths.push(left.iter().chain(&right).cloned().collect());
                }
            }
            Ok(paths)
        }
        Filter::Comma(left, right) => {
            let mut paths = filter_paths(left)?;
            paths.extend(filter_paths(right)?);
            Ok(paths)
        }
        _ => Err(EvalError::InvalidPath(
            "target must contain only fields and indices".to_owned(),
        )),
    }
}

pub(super) fn value_path(value: &Value) -> Result<Vec<PathPart>, EvalError> {
    let Value::Array(parts) = value else {
        return Err(EvalError::InvalidPath("path must be an array".to_owned()));
    };
    parts
        .iter()
        .map(|part| match part {
            Value::String(key) => Ok(PathPart::Key(key.clone())),
            Value::Number(index) => index
                .as_i64()
                .map(PathPart::Index)
                .ok_or_else(|| EvalError::InvalidPath("indices must be integers".to_owned())),
            _ => Err(EvalError::InvalidPath(
                "components must be strings or integers".to_owned(),
            )),
        })
        .collect()
}

pub(super) fn path_value(path: Vec<PathPart>) -> Value {
    Value::Array(
        path.into_iter()
            .map(|part| match part {
                PathPart::Key(key) => Value::String(key),
                PathPart::Index(index) => Value::Number(index.into()),
            })
            .collect(),
    )
}

pub(super) fn get_path(input: &Value, path: &[PathPart]) -> Value {
    path.iter()
        .fold(input.clone(), |value, part| match (value, part) {
            (Value::Object(map), PathPart::Key(key)) => {
                map.get(key).cloned().unwrap_or(Value::Null)
            }
            (Value::Array(items), PathPart::Index(index)) => normalized_index(*index, items.len())
                .and_then(|index| items.get(index).cloned())
                .unwrap_or(Value::Null),
            _ => Value::Null,
        })
}

pub(super) fn set_path(
    target: &mut Value,
    path: &[PathPart],
    value: Value,
) -> Result<(), EvalError> {
    let Some((part, rest)) = path.split_first() else {
        *target = value;
        return Ok(());
    };
    match part {
        PathPart::Key(key) => {
            if target.is_null() {
                *target = Value::Object(Map::new());
            }
            let Value::Object(map) = target else {
                return Err(EvalError::InvalidPath(format!(
                    "cannot set key {key:?} on {}",
                    type_name(target)
                )));
            };
            set_path(map.entry(key.clone()).or_insert(Value::Null), rest, value)
        }
        PathPart::Index(index) => {
            if target.is_null() {
                *target = Value::Array(Vec::new());
            }
            let Value::Array(items) = target else {
                return Err(EvalError::InvalidPath(format!(
                    "cannot set index on {}",
                    type_name(target)
                )));
            };
            let index = if *index < 0 {
                normalized_index(*index, items.len()).ok_or_else(|| {
                    EvalError::InvalidPath("negative index is out of range".to_owned())
                })?
            } else {
                *index as usize
            };
            if index >= items.len() {
                items.resize(index + 1, Value::Null);
            }
            set_path(&mut items[index], rest, value)
        }
    }
}

pub(super) fn remove_path(target: &mut Value, path: &[PathPart]) -> Result<(), EvalError> {
    let Some((part, rest)) = path.split_first() else {
        *target = Value::Null;
        return Ok(());
    };
    if rest.is_empty() {
        match (target, part) {
            (Value::Object(map), PathPart::Key(key)) => {
                map.shift_remove(key);
            }
            (Value::Array(items), PathPart::Index(index)) => {
                if let Some(index) = normalized_index(*index, items.len())
                    && index < items.len()
                {
                    items.remove(index);
                }
            }
            _ => {}
        }
        return Ok(());
    }
    match (target, part) {
        (Value::Object(map), PathPart::Key(key)) => {
            if let Some(value) = map.get_mut(key) {
                remove_path(value, rest)?;
            }
        }
        (Value::Array(items), PathPart::Index(index)) => {
            if let Some(index) = normalized_index(*index, items.len())
                && let Some(value) = items.get_mut(index)
            {
                remove_path(value, rest)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn all_paths(input: &Value) -> Vec<Vec<PathPart>> {
    fn visit(value: &Value, prefix: &mut Vec<PathPart>, output: &mut Vec<Vec<PathPart>>) {
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    prefix.push(PathPart::Key(key.clone()));
                    output.push(prefix.clone());
                    visit(value, prefix, output);
                    prefix.pop();
                }
            }
            Value::Array(items) => {
                for (index, value) in items.iter().enumerate() {
                    prefix.push(PathPart::Index(index as i64));
                    output.push(prefix.clone());
                    visit(value, prefix, output);
                    prefix.pop();
                }
            }
            _ => {}
        }
    }
    let mut output = Vec::new();
    visit(input, &mut Vec::new(), &mut output);
    output
}

pub(super) fn apply_update(
    operation: UpdateOp,
    current: &Value,
    value: &Value,
) -> Result<Value, EvalError> {
    let binary = match operation {
        UpdateOp::Assign | UpdateOp::Modify => return Ok(value.clone()),
        UpdateOp::Add => BinaryOp::Add,
        UpdateOp::Subtract => BinaryOp::Subtract,
        UpdateOp::Multiply => BinaryOp::Multiply,
        UpdateOp::Divide => BinaryOp::Divide,
        UpdateOp::Remainder => BinaryOp::Remainder,
    };
    apply_binary(binary, current, value)
}
