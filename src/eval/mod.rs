mod path;
mod value;

use crate::environment::{Environment, FunctionEnvironment};
use crate::{BinaryOp, Builtin, EvalError, Filter, InterpolationPart, UpdateOp};
use path::{
    all_paths, apply_update, filter_paths, get_path, path_value, remove_path, set_path, value_path,
};
use serde_json::{Map, Value};
use std::collections::HashMap;
use value::{
    apply_binary, apply_unary, array_items, build_regex, builtin_name, compare_values, contains,
    deduplicate, flatten, format_value, has, index_value, is_truthy, iterate, join_part, keys,
    length, match_object, slice_value, sort_values, to_string_value, type_name,
};

pub(crate) fn evaluate(filter: &Filter, input: &Value) -> Result<Vec<Value>, EvalError> {
    Evaluator::default().evaluate(filter, input)
}

pub(crate) fn evaluate_with_variables(
    filter: &Filter,
    input: &Value,
    variables: &[(String, Value)],
) -> Result<Vec<Value>, EvalError> {
    let evaluator = variables
        .iter()
        .fold(Evaluator::default(), |evaluator, (name, value)| {
            evaluator.with_binding(name, value.clone())
        });
    evaluator.evaluate(filter, input)
}
const RECURSION_LIMIT: usize = 64;
#[derive(Debug, Clone, Default)]
struct Evaluator {
    environment: Environment,
    functions: FunctionEnvironment,
    parameters: HashMap<String, Filter>,
    call_depth: usize,
}

impl Evaluator {
    fn with_binding(&self, name: &str, value: Value) -> Self {
        Self {
            environment: self.environment.bind(name, value),
            ..self.clone()
        }
    }

    fn with_function(&self, name: &str, parameters: &[String], body: &Filter) -> Self {
        Self {
            functions: self.functions.bind(name, parameters, body),
            ..self.clone()
        }
    }

    fn evaluate(&self, filter: &Filter, input: &Value) -> Result<Vec<Value>, EvalError> {
        match filter {
            Filter::Identity => Ok(vec![input.clone()]),
            Filter::Literal(value) => Ok(vec![value.clone()]),
            Filter::Field(name) => match input {
                Value::Object(map) => Ok(vec![map.get(name).cloned().unwrap_or(Value::Null)]),
                Value::Null => Ok(vec![Value::Null]),
                _ => Err(EvalError::InvalidIndex {
                    input: type_name(input),
                    index: "string",
                }),
            },
            Filter::Index(index) => match input {
                Value::Array(_) | Value::Object(_) | Value::Null => {
                    Ok(vec![index_value(input, *index)])
                }
                _ => Err(EvalError::InvalidIndex {
                    input: type_name(input),
                    index: "number",
                }),
            },
            Filter::Iterate => match input {
                Value::Array(items) => Ok(items.clone()),
                Value::Object(map) => Ok(map.values().cloned().collect()),
                _ => Err(EvalError::InvalidIteration {
                    input: type_name(input),
                }),
            },
            Filter::Slice(start, end) => match input {
                Value::Array(_) | Value::String(_) | Value::Null => {
                    Ok(vec![slice_value(input, *start, *end)])
                }
                _ => Err(EvalError::InvalidSlice {
                    input: type_name(input),
                }),
            },
            Filter::Array(inner) => Ok(vec![Value::Array(self.evaluate(inner, input)?)]),
            Filter::Object(fields) => build_objects(self, fields, input),
            Filter::Pipe(left, right) => {
                let mut output = Vec::new();
                for value in self.evaluate(left, input)? {
                    output.extend(self.evaluate(right, &value)?);
                }
                Ok(output)
            }
            Filter::Comma(left, right) => {
                let mut values = self.evaluate(left, input)?;
                values.extend(self.evaluate(right, input)?);
                Ok(values)
            }
            Filter::Optional(inner) => Ok(self.evaluate(inner, input).unwrap_or_default()),
            Filter::Unary(operator, inner) => self
                .evaluate(inner, input)?
                .into_iter()
                .map(|value| apply_unary(*operator, value))
                .collect(),
            Filter::Binary(BinaryOp::Alternative, left, right) => {
                let values: Vec<_> = self
                    .evaluate(left, input)?
                    .into_iter()
                    .filter(is_truthy)
                    .collect();
                if values.is_empty() {
                    self.evaluate(right, input)
                } else {
                    Ok(values)
                }
            }
            Filter::Binary(operator, left, right) => {
                let left_values = self.evaluate(left, input)?;
                let right_values = self.evaluate(right, input)?;
                let mut output = Vec::new();
                for left in &left_values {
                    for right in &right_values {
                        output.push(apply_binary(*operator, left, right)?);
                    }
                }
                Ok(output)
            }
            Filter::Builtin(builtin, arguments) => {
                self.evaluate_builtin(*builtin, arguments, input)
            }
            Filter::Variable(name) => self
                .environment
                .get(name)
                .map(|value| vec![value])
                .ok_or_else(|| EvalError::UndefinedVariable(name.clone())),
            Filter::Bind { source, name, body } => {
                let mut output = Vec::new();
                for value in self.evaluate(source, input)? {
                    output.extend(self.with_binding(name, value).evaluate(body, input)?);
                }
                Ok(output)
            }
            Filter::Define {
                name,
                parameters,
                body,
                then,
            } => self
                .with_function(name, parameters, body)
                .evaluate(then, input),
            Filter::Call { name, arguments } => self.evaluate_call(name, arguments, input),
            Filter::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let mut output = Vec::new();
                for condition in self.evaluate(condition, input)? {
                    let branch = if is_truthy(&condition) {
                        then_branch
                    } else {
                        else_branch
                    };
                    output.extend(self.evaluate(branch, input)?);
                }
                Ok(output)
            }
            Filter::Try { body, handler } => match self.evaluate(body, input) {
                Ok(output) => Ok(output),
                Err(error) => match handler {
                    Some(handler) => self.evaluate(handler, &Value::String(error.to_string())),
                    None => Ok(Vec::new()),
                },
            },
            Filter::Reduce {
                source,
                variable,
                initial,
                update,
            } => self.evaluate_fold(source, variable, initial, update, None, input),
            Filter::Foreach {
                source,
                variable,
                initial,
                update,
                extract,
            } => self.evaluate_fold(source, variable, initial, update, Some(extract), input),
            Filter::Update {
                target,
                operation,
                value,
            } => self.evaluate_update(target, *operation, value, input),
            Filter::Interpolate(parts) => self.evaluate_interpolation(parts, input),
        }
    }

    fn evaluate_interpolation(
        &self,
        parts: &[InterpolationPart],
        input: &Value,
    ) -> Result<Vec<Value>, EvalError> {
        let mut strings = vec![String::new()];
        for part in parts {
            match part {
                InterpolationPart::Text(text) => {
                    for output in &mut strings {
                        output.push_str(text);
                    }
                }
                InterpolationPart::Filter(filter) => {
                    let values = self.evaluate(filter, input)?;
                    let mut expanded = Vec::new();
                    for prefix in &strings {
                        for value in &values {
                            let Value::String(text) = to_string_value(value)? else {
                                return Err(EvalError::Serialization(
                                    "interpolation was not text".to_owned(),
                                ));
                            };
                            expanded.push(format!("{prefix}{text}"));
                        }
                    }
                    strings = expanded;
                }
            }
        }
        Ok(strings.into_iter().map(Value::String).collect())
    }

    fn evaluate_update(
        &self,
        target: &Filter,
        operation: UpdateOp,
        value_filter: &Filter,
        input: &Value,
    ) -> Result<Vec<Value>, EvalError> {
        let paths = filter_paths(target)?;
        let mut states = vec![input.clone()];
        for path in paths {
            let mut next = Vec::new();
            for state in states {
                let current = get_path(&state, &path);
                let rhs_input = if operation == UpdateOp::Assign {
                    input
                } else {
                    &current
                };
                let values = self.evaluate(value_filter, rhs_input)?;
                if values.is_empty() && operation == UpdateOp::Modify {
                    let mut updated = state;
                    remove_path(&mut updated, &path)?;
                    next.push(updated);
                } else {
                    for value in values {
                        let value = apply_update(operation, &current, &value)?;
                        let mut updated = state.clone();
                        set_path(&mut updated, &path, value)?;
                        next.push(updated);
                    }
                }
            }
            states = next;
        }
        Ok(states)
    }

    fn evaluate_fold(
        &self,
        source: &Filter,
        variable: &str,
        initial: &Filter,
        update: &Filter,
        extract: Option<&Filter>,
        input: &Value,
    ) -> Result<Vec<Value>, EvalError> {
        let source_values = self.evaluate(source, input)?;
        let mut accumulators = self.evaluate(initial, input)?;
        let mut extracted = Vec::new();
        for source_value in source_values {
            let evaluator = self.with_binding(variable, source_value);
            let mut next = Vec::new();
            for accumulator in accumulators {
                for updated in evaluator.evaluate(update, &accumulator)? {
                    if let Some(extract) = extract {
                        extracted.extend(evaluator.evaluate(extract, &updated)?);
                    }
                    next.push(updated);
                }
            }
            accumulators = next;
        }
        Ok(if extract.is_some() {
            extracted
        } else {
            accumulators
        })
    }

    fn evaluate_call(
        &self,
        name: &str,
        arguments: &[Filter],
        input: &Value,
    ) -> Result<Vec<Value>, EvalError> {
        if arguments.is_empty()
            && let Some(parameter) = self.parameters.get(name)
        {
            return self.evaluate(parameter, input);
        }
        let function = self
            .functions
            .get(name)
            .ok_or_else(|| EvalError::UndefinedFunction(name.to_owned()))?;
        if function.parameters.len() != arguments.len() {
            return Err(EvalError::WrongArity {
                name: name.to_owned(),
                expected: function.parameters.len(),
                actual: arguments.len(),
            });
        }
        if self.call_depth >= RECURSION_LIMIT {
            return Err(EvalError::RecursionLimit {
                limit: RECURSION_LIMIT,
            });
        }
        let parameters = function
            .parameters
            .iter()
            .cloned()
            .zip(arguments.iter().cloned())
            .collect();
        Self {
            parameters,
            call_depth: self.call_depth + 1,
            ..self.clone()
        }
        .evaluate(&function.body, input)
    }

    fn evaluate_builtin(
        &self,
        builtin: Builtin,
        arguments: &[Filter],
        input: &Value,
    ) -> Result<Vec<Value>, EvalError> {
        use Builtin::*;
        match builtin {
            Length => Ok(vec![length(input)?]),
            Type => Ok(vec![Value::String(type_name(input).to_owned())]),
            Keys => Ok(vec![keys(input)?]),
            Empty => Ok(Vec::new()),
            ToString => Ok(vec![to_string_value(input)?]),
            Has => {
                let mut output = Vec::new();
                for key in self.evaluate(&arguments[0], input)? {
                    output.push(Value::Bool(has(input, &key)?));
                }
                Ok(output)
            }
            Map => {
                let values = iterate(input)?;
                let mut output = Vec::new();
                for value in values {
                    output.extend(self.evaluate(&arguments[0], &value)?);
                }
                Ok(vec![Value::Array(output)])
            }
            Select => {
                let mut output = Vec::new();
                for condition in self.evaluate(&arguments[0], input)? {
                    if is_truthy(&condition) {
                        output.push(input.clone());
                    }
                }
                Ok(output)
            }
            Error => {
                let values = if let Some(argument) = arguments.first() {
                    self.evaluate(argument, input)?
                } else {
                    vec![input.clone()]
                };
                let message = values
                    .first()
                    .map(to_string_value)
                    .transpose()?
                    .unwrap_or(Value::Null);
                Err(EvalError::Explicit(
                    message.as_str().unwrap_or("null").to_owned(),
                ))
            }
            Del => {
                let mut output = input.clone();
                for argument in arguments {
                    for path in filter_paths(argument)? {
                        remove_path(&mut output, &path)?;
                    }
                }
                Ok(vec![output])
            }
            GetPath => {
                let mut output = Vec::new();
                for path in self.evaluate(&arguments[0], input)? {
                    output.push(get_path(input, &value_path(&path)?));
                }
                Ok(output)
            }
            SetPath => {
                let paths = self.evaluate(&arguments[0], input)?;
                let values = self.evaluate(&arguments[1], input)?;
                let mut output = Vec::new();
                for path in paths {
                    let path = value_path(&path)?;
                    for value in &values {
                        let mut updated = input.clone();
                        set_path(&mut updated, &path, value.clone())?;
                        output.push(updated);
                    }
                }
                Ok(output)
            }
            Paths => Ok(all_paths(input).into_iter().map(path_value).collect()),
            Sort => Ok(vec![sort_values(input)?]),
            SortBy | GroupBy | UniqueBy => {
                self.evaluate_keyed_collection_builtin(builtin, &arguments[0], input)
            }
            Unique => {
                let Value::Array(items) = sort_values(input)? else {
                    unreachable!()
                };
                Ok(vec![Value::Array(deduplicate(items))])
            }
            Min | Max => {
                let items = array_items(input, builtin_name(builtin))?;
                let selected = if builtin == Min {
                    items.iter().min_by(|a, b| compare_values(a, b))
                } else {
                    items.iter().max_by(|a, b| compare_values(a, b))
                };
                Ok(vec![selected.cloned().unwrap_or(Value::Null)])
            }
            Add => {
                let items = array_items(input, "add")?;
                let mut total = Value::Null;
                for item in items {
                    total = apply_binary(BinaryOp::Add, &total, item)?;
                }
                Ok(vec![total])
            }
            Flatten => {
                let depth = if let Some(argument) = arguments.first() {
                    self.evaluate(argument, input)?
                        .first()
                        .and_then(Value::as_u64)
                        .ok_or(EvalError::InvalidBuiltin {
                            name: "flatten",
                            input: type_name(input),
                        })? as usize
                } else {
                    usize::MAX
                };
                Ok(vec![flatten(input, depth)?])
            }
            Contains | Inside => {
                let mut output = Vec::new();
                for other in self.evaluate(&arguments[0], input)? {
                    output.push(Value::Bool(if builtin == Contains {
                        contains(input, &other)
                    } else {
                        contains(&other, input)
                    }));
                }
                Ok(output)
            }
            StartsWith | EndsWith | Split => {
                self.evaluate_string_builtin(builtin, &arguments[0], input)
            }
            Join => {
                let items = array_items(input, "join")?;
                let mut output = Vec::new();
                for separator in self.evaluate(&arguments[0], input)? {
                    let separator = separator.as_str().ok_or(EvalError::InvalidBuiltin {
                        name: "join",
                        input: type_name(&separator),
                    })?;
                    let parts = items.iter().map(join_part).collect::<Result<Vec<_>, _>>()?;
                    output.push(Value::String(parts.join(separator)));
                }
                Ok(output)
            }
            ToNumber => match input {
                Value::Number(_) => Ok(vec![input.clone()]),
                Value::String(text) => serde_json::from_str::<Value>(text)
                    .ok()
                    .filter(Value::is_number)
                    .map(|value| vec![value])
                    .ok_or(EvalError::InvalidBuiltin {
                        name: "tonumber",
                        input: "string",
                    }),
                _ => Err(EvalError::InvalidBuiltin {
                    name: "tonumber",
                    input: type_name(input),
                }),
            },
            FromJson => {
                let text = input.as_str().ok_or(EvalError::InvalidBuiltin {
                    name: "fromjson",
                    input: type_name(input),
                })?;
                serde_json::from_str(text)
                    .map(|value| vec![value])
                    .map_err(|error| EvalError::Explicit(error.to_string()))
            }
            Test | Match | Capture | Scan => self.evaluate_regex_builtin(builtin, arguments, input),
            Sub | Gsub => self.evaluate_substitution(builtin, arguments, input),
            Format(format) => Ok(vec![format_value(format, input)?]),
        }
    }

    fn evaluate_regex_builtin(
        &self,
        builtin: Builtin,
        arguments: &[Filter],
        input: &Value,
    ) -> Result<Vec<Value>, EvalError> {
        let text = input.as_str().ok_or(EvalError::InvalidBuiltin {
            name: builtin_name(builtin),
            input: type_name(input),
        })?;
        let pattern = self.first_string(&arguments[0], input, builtin_name(builtin))?;
        let flags = arguments
            .get(1)
            .map(|filter| self.first_string(filter, input, builtin_name(builtin)))
            .transpose()?
            .unwrap_or_default();
        let regex = build_regex(&pattern, &flags)?;
        match builtin {
            Builtin::Test => Ok(vec![Value::Bool(regex.is_match(text))]),
            Builtin::Scan => Ok(regex
                .find_iter(text)
                .map(|found| Value::String(found.as_str().to_owned()))
                .collect()),
            Builtin::Match => {
                let matches: Vec<_> = if flags.contains('g') {
                    regex.captures_iter(text).collect()
                } else {
                    regex.captures(text).into_iter().collect()
                };
                Ok(matches
                    .into_iter()
                    .map(|captures| match_object(text, &regex, &captures))
                    .collect())
            }
            Builtin::Capture => {
                let Some(captures) = regex.captures(text) else {
                    return Ok(Vec::new());
                };
                let mut object = Map::new();
                for name in regex.capture_names().flatten() {
                    object.insert(
                        name.to_owned(),
                        captures
                            .name(name)
                            .map(|value| Value::String(value.as_str().to_owned()))
                            .unwrap_or(Value::Null),
                    );
                }
                Ok(vec![Value::Object(object)])
            }
            _ => Err(EvalError::InvalidBuiltin {
                name: "regex",
                input: type_name(input),
            }),
        }
    }

    fn evaluate_substitution(
        &self,
        builtin: Builtin,
        arguments: &[Filter],
        input: &Value,
    ) -> Result<Vec<Value>, EvalError> {
        let text = input.as_str().ok_or(EvalError::InvalidBuiltin {
            name: builtin_name(builtin),
            input: type_name(input),
        })?;
        let pattern = self.first_string(&arguments[0], input, builtin_name(builtin))?;
        let replacement = self.first_string(&arguments[1], input, builtin_name(builtin))?;
        let regex = build_regex(&pattern, "")?;
        let result = if builtin == Builtin::Gsub {
            regex.replace_all(text, replacement.as_str())
        } else {
            regex.replace(text, replacement.as_str())
        };
        Ok(vec![Value::String(result.into_owned())])
    }

    fn first_string(
        &self,
        filter: &Filter,
        input: &Value,
        name: &'static str,
    ) -> Result<String, EvalError> {
        let values = self.evaluate(filter, input)?;
        values
            .first()
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(EvalError::InvalidBuiltin {
                name,
                input: "non-string argument",
            })
    }

    fn evaluate_keyed_collection_builtin(
        &self,
        builtin: Builtin,
        key_filter: &Filter,
        input: &Value,
    ) -> Result<Vec<Value>, EvalError> {
        let items = array_items(input, builtin_name(builtin))?;
        let mut keyed = Vec::new();
        for item in items {
            let keys = self.evaluate(key_filter, item)?;
            keyed.push((keys.first().cloned().unwrap_or(Value::Null), item.clone()));
        }
        keyed.sort_by(|(a, _), (b, _)| compare_values(a, b));
        let result = match builtin {
            Builtin::SortBy => Value::Array(keyed.into_iter().map(|(_, value)| value).collect()),
            Builtin::UniqueBy => {
                keyed.dedup_by(|(a, _), (b, _)| a == b);
                Value::Array(keyed.into_iter().map(|(_, value)| value).collect())
            }
            Builtin::GroupBy => {
                let mut groups: Vec<Vec<Value>> = Vec::new();
                let mut previous: Option<Value> = None;
                for (key, value) in keyed {
                    if previous.as_ref() != Some(&key) {
                        groups.push(Vec::new());
                        previous = Some(key);
                    }
                    if let Some(group) = groups.last_mut() {
                        group.push(value);
                    }
                }
                Value::Array(groups.into_iter().map(Value::Array).collect())
            }
            _ => unreachable!(),
        };
        Ok(vec![result])
    }

    fn evaluate_string_builtin(
        &self,
        builtin: Builtin,
        argument: &Filter,
        input: &Value,
    ) -> Result<Vec<Value>, EvalError> {
        let text = input.as_str().ok_or(EvalError::InvalidBuiltin {
            name: builtin_name(builtin),
            input: type_name(input),
        })?;
        let mut output = Vec::new();
        for value in self.evaluate(argument, input)? {
            let pattern = value.as_str().ok_or(EvalError::InvalidBuiltin {
                name: builtin_name(builtin),
                input: type_name(&value),
            })?;
            output.push(match builtin {
                Builtin::StartsWith => Value::Bool(text.starts_with(pattern)),
                Builtin::EndsWith => Value::Bool(text.ends_with(pattern)),
                Builtin::Split => Value::Array(
                    text.split(pattern)
                        .map(|part| Value::String(part.to_owned()))
                        .collect(),
                ),
                _ => unreachable!(),
            });
        }
        Ok(output)
    }
}

fn build_objects(
    evaluator: &Evaluator,
    fields: &[(String, Filter)],
    input: &Value,
) -> Result<Vec<Value>, EvalError> {
    let mut objects = vec![Map::new()];
    for (name, filter) in fields {
        let values = evaluator.evaluate(filter, input)?;
        let mut expanded = Vec::new();
        for object in objects {
            for value in &values {
                let mut object = object.clone();
                object.insert(name.clone(), value.clone());
                expanded.push(object);
            }
        }
        objects = expanded;
    }
    Ok(objects.into_iter().map(Value::Object).collect())
}
