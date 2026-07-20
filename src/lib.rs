#![forbid(unsafe_code)]

mod ast;
mod error;

pub use ast::{BinaryOp, Builtin, Filter, Format, InterpolationPart, UnaryOp, UpdateOp};
use base64::Engine;
pub use error::{EvalError, ParseError, ParseErrorKind};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::rc::Rc;

type Error = ParseError;

pub fn compile(source: &str) -> Result<Filter, ParseError> {
    let mut parser = Parser { source, offset: 0 };
    let filter = parser.parse_program()?;
    parser.whitespace();
    if parser.offset != source.len() {
        return Err(ParseError::unexpected(parser.offset));
    }
    Ok(filter)
}

pub fn evaluate(filter: &Filter, input: &Value) -> Result<Vec<Value>, EvalError> {
    Evaluator::default().evaluate(filter, input)
}

pub fn evaluate_with_variables(
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

#[derive(Debug, Clone, Default)]
struct Environment(Option<Rc<Binding>>);

#[derive(Debug)]
struct Binding {
    name: String,
    value: Value,
    parent: Environment,
}

#[derive(Debug, Clone, Default)]
struct FunctionEnvironment(Option<Rc<FunctionBinding>>);

#[derive(Debug, Clone)]
struct FunctionBinding {
    name: String,
    parameters: Vec<String>,
    body: Filter,
    parent: FunctionEnvironment,
}

impl Environment {
    fn bind(&self, name: &str, value: Value) -> Self {
        Self(Some(Rc::new(Binding {
            name: name.to_owned(),
            value,
            parent: self.clone(),
        })))
    }

    fn get(&self, name: &str) -> Option<Value> {
        let mut environment = self;
        while let Some(binding) = &environment.0 {
            if binding.name == name {
                return Some(binding.value.clone());
            }
            environment = &binding.parent;
        }
        None
    }
}

impl FunctionEnvironment {
    fn bind(&self, name: &str, parameters: &[String], body: &Filter) -> Self {
        Self(Some(Rc::new(FunctionBinding {
            name: name.to_owned(),
            parameters: parameters.to_vec(),
            body: body.clone(),
            parent: self.clone(),
        })))
    }

    fn get(&self, name: &str) -> Option<Rc<FunctionBinding>> {
        let mut environment = self;
        while let Some(binding) = &environment.0 {
            if binding.name == name {
                return Some(Rc::clone(binding));
            }
            environment = &binding.parent;
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PathPart {
    Key(String),
    Index(i64),
}

fn filter_paths(filter: &Filter) -> Result<Vec<Vec<PathPart>>, EvalError> {
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

fn value_path(value: &Value) -> Result<Vec<PathPart>, EvalError> {
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

fn path_value(path: Vec<PathPart>) -> Value {
    Value::Array(
        path.into_iter()
            .map(|part| match part {
                PathPart::Key(key) => Value::String(key),
                PathPart::Index(index) => Value::Number(index.into()),
            })
            .collect(),
    )
}

fn get_path(input: &Value, path: &[PathPart]) -> Value {
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

fn set_path(target: &mut Value, path: &[PathPart], value: Value) -> Result<(), EvalError> {
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

fn remove_path(target: &mut Value, path: &[PathPart]) -> Result<(), EvalError> {
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

fn all_paths(input: &Value) -> Vec<Vec<PathPart>> {
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

fn apply_update(operation: UpdateOp, current: &Value, value: &Value) -> Result<Value, EvalError> {
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

fn length(input: &Value) -> Result<Value, EvalError> {
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

fn keys(input: &Value) -> Result<Value, EvalError> {
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

fn has(input: &Value, key: &Value) -> Result<bool, EvalError> {
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

fn iterate(input: &Value) -> Result<Vec<Value>, EvalError> {
    match input {
        Value::Array(items) => Ok(items.clone()),
        Value::Object(map) => Ok(map.values().cloned().collect()),
        _ => Err(EvalError::InvalidIteration {
            input: type_name(input),
        }),
    }
}

fn to_string_value(input: &Value) -> Result<Value, EvalError> {
    match input {
        Value::String(text) => Ok(Value::String(text.clone())),
        value => serde_json::to_string(value)
            .map(Value::String)
            .map_err(|error| EvalError::Serialization(error.to_string())),
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn is_truthy(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false))
}

fn apply_unary(operator: UnaryOp, value: Value) -> Result<Value, EvalError> {
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

fn apply_binary(operator: BinaryOp, left: &Value, right: &Value) -> Result<Value, EvalError> {
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

fn number_value(number: f64) -> Result<Value, EvalError> {
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

fn compare_values(left: &Value, right: &Value) -> std::cmp::Ordering {
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

fn builtin_name(builtin: Builtin) -> &'static str {
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

fn array_items<'a>(input: &'a Value, name: &'static str) -> Result<&'a [Value], EvalError> {
    input
        .as_array()
        .map(Vec::as_slice)
        .ok_or(EvalError::InvalidBuiltin {
            name,
            input: type_name(input),
        })
}

fn sort_values(input: &Value) -> Result<Value, EvalError> {
    let mut items = array_items(input, "sort")?.to_vec();
    items.sort_by(compare_values);
    Ok(Value::Array(items))
}

fn deduplicate(mut items: Vec<Value>) -> Vec<Value> {
    items.dedup();
    items
}

fn flatten(input: &Value, depth: usize) -> Result<Value, EvalError> {
    let items = array_items(input, "flatten")?;
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
            output.push(item.clone());
        }
    }
    Ok(Value::Array(output))
}

fn contains(container: &Value, candidate: &Value) -> bool {
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

fn join_part(value: &Value) -> Result<String, EvalError> {
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

fn build_regex(pattern: &str, flags: &str) -> Result<Regex, EvalError> {
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

fn match_object(text: &str, regex: &Regex, captures: &regex::Captures<'_>) -> Value {
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

fn format_value(format: Format, input: &Value) -> Result<Value, EvalError> {
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

fn index_value(input: &Value, index: i64) -> Value {
    match input {
        Value::Array(items) => normalized_index(index, items.len())
            .and_then(|i| items.get(i))
            .cloned()
            .unwrap_or(Value::Null),
        Value::Object(map) => map.get(&index.to_string()).cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn slice_value(input: &Value, start: Option<i64>, end: Option<i64>) -> Value {
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

fn normalized_index(index: i64, len: usize) -> Option<usize> {
    let index = if index < 0 { len as i64 + index } else { index };
    (index >= 0).then_some(index as usize)
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
}

impl Parser<'_> {
    fn parse_program(&mut self) -> Result<Filter, Error> {
        if !self.eat_word("def") {
            return self.parse_comma();
        }
        let name = self.identifier_required()?;
        let parameters = if self.eat('(') {
            let mut parameters = Vec::new();
            if !self.eat(')') {
                loop {
                    parameters.push(self.identifier_required()?);
                    if self.eat(')') {
                        break;
                    }
                    self.expect(';')?;
                }
            }
            parameters
        } else {
            Vec::new()
        };
        self.expect(':')?;
        let body = self.parse_comma()?;
        self.expect(';')?;
        let then = self.parse_program()?;
        Ok(Filter::Define {
            name,
            parameters,
            body: Box::new(body),
            then: Box::new(then),
        })
    }

    fn parse_comma(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_pipe()?;
        while self.eat(',') {
            filter = Filter::Comma(Box::new(filter), Box::new(self.parse_pipe()?));
        }
        Ok(filter)
    }

    fn parse_assignment(&mut self) -> Result<Filter, Error> {
        let target = self.parse_alternative()?;
        let operation = if self.eat_str("|=") {
            Some(UpdateOp::Modify)
        } else if self.eat_str("+=") {
            Some(UpdateOp::Add)
        } else if self.eat_str("-=") {
            Some(UpdateOp::Subtract)
        } else if self.eat_str("*=") {
            Some(UpdateOp::Multiply)
        } else if self.eat_str("/=") {
            Some(UpdateOp::Divide)
        } else if self.eat_str("%=") {
            Some(UpdateOp::Remainder)
        } else if !self.starts_with("==") && self.eat('=') {
            Some(UpdateOp::Assign)
        } else {
            None
        };
        match operation {
            Some(operation) => Ok(Filter::Update {
                target: Box::new(target),
                operation,
                value: Box::new(self.parse_assignment()?),
            }),
            None => Ok(target),
        }
    }

    fn parse_pipe(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_assignment()?;
        loop {
            if self.eat_word("as") {
                self.expect('$')?;
                let name = self.identifier_required()?;
                self.expect('|')?;
                return Ok(Filter::Bind {
                    source: Box::new(filter),
                    name,
                    body: Box::new(self.parse_pipe()?),
                });
            }
            if self.starts_with("|=") {
                break;
            } else if self.eat('|') {
                filter = Filter::Pipe(Box::new(filter), Box::new(self.parse_assignment()?));
            } else {
                break;
            }
        }
        Ok(filter)
    }

    fn parse_alternative(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_or()?;
        while self.eat_str("//") {
            filter = Filter::Binary(
                BinaryOp::Alternative,
                Box::new(filter),
                Box::new(self.parse_or()?),
            );
        }
        Ok(filter)
    }

    fn parse_or(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_and()?;
        while self.eat_word("or") {
            filter = Filter::Binary(BinaryOp::Or, Box::new(filter), Box::new(self.parse_and()?));
        }
        Ok(filter)
    }

    fn parse_and(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_comparison()?;
        while self.eat_word("and") {
            filter = Filter::Binary(
                BinaryOp::And,
                Box::new(filter),
                Box::new(self.parse_comparison()?),
            );
        }
        Ok(filter)
    }

    fn parse_comparison(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_additive()?;
        loop {
            let operator = if self.eat_str("==") {
                Some(BinaryOp::Equal)
            } else if self.eat_str("!=") {
                Some(BinaryOp::NotEqual)
            } else if self.eat_str("<=") {
                Some(BinaryOp::LessEqual)
            } else if self.eat_str(">=") {
                Some(BinaryOp::GreaterEqual)
            } else if self.eat('<') {
                Some(BinaryOp::Less)
            } else if self.eat('>') {
                Some(BinaryOp::Greater)
            } else {
                None
            };
            let Some(operator) = operator else { break };
            filter = Filter::Binary(operator, Box::new(filter), Box::new(self.parse_additive()?));
        }
        Ok(filter)
    }

    fn parse_additive(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_multiplicative()?;
        loop {
            let operator = if self.starts_with("+=") || self.starts_with("-=") {
                None
            } else if self.eat('+') {
                Some(BinaryOp::Add)
            } else if self.eat('-') {
                Some(BinaryOp::Subtract)
            } else {
                None
            };
            let Some(operator) = operator else { break };
            filter = Filter::Binary(
                operator,
                Box::new(filter),
                Box::new(self.parse_multiplicative()?),
            );
        }
        Ok(filter)
    }

    fn parse_multiplicative(&mut self) -> Result<Filter, Error> {
        let mut filter = self.parse_unary()?;
        loop {
            let operator =
                if self.starts_with("*=") || self.starts_with("/=") || self.starts_with("%=") {
                    None
                } else if self.eat('*') {
                    Some(BinaryOp::Multiply)
                } else if self.starts_with("//") {
                    None
                } else if self.eat('/') {
                    Some(BinaryOp::Divide)
                } else if self.eat('%') {
                    Some(BinaryOp::Remainder)
                } else {
                    None
                };
            let Some(operator) = operator else { break };
            filter = Filter::Binary(operator, Box::new(filter), Box::new(self.parse_unary()?));
        }
        Ok(filter)
    }

    fn parse_unary(&mut self) -> Result<Filter, Error> {
        if self.eat_word("not") {
            Ok(Filter::Unary(UnaryOp::Not, Box::new(self.parse_unary()?)))
        } else if self.eat('-') {
            Ok(Filter::Unary(
                UnaryOp::Negate,
                Box::new(self.parse_unary()?),
            ))
        } else {
            self.parse_primary()
        }
    }

    fn parse_primary(&mut self) -> Result<Filter, Error> {
        self.whitespace();
        let mut filter = match self.peek() {
            Some('.') => self.parse_dot()?,
            Some('[') => self.parse_array()?,
            Some('{') => self.parse_object()?,
            Some('$') => {
                self.bump();
                Filter::Variable(self.identifier_required()?)
            }
            Some('(') => {
                self.bump();
                let inner = self.parse_program()?;
                self.expect(')')?;
                inner
            }
            Some('"') => self.parse_interpolated_string()?,
            Some('@') => self.parse_format()?,
            Some('-' | '0'..='9') => Filter::Literal(self.json_literal()?),
            Some(c) if is_identifier_start(c) => self.parse_named_or_control()?,
            _ => return Err(Error::new("expected a filter", self.offset)),
        };

        loop {
            if self.eat('?') {
                filter = Filter::Optional(Box::new(filter));
            } else if self.next_non_whitespace() == Some('[') {
                let access = self.parse_bracket_access()?;
                filter = Filter::Pipe(Box::new(filter), Box::new(access));
            } else if self.next_non_whitespace() == Some('.') {
                self.eat('.');
                let name = self.identifier_required()?;
                filter = Filter::Pipe(Box::new(filter), Box::new(Filter::Field(name)));
            } else {
                break;
            }
        }
        Ok(filter)
    }

    fn parse_dot(&mut self) -> Result<Filter, Error> {
        self.expect('.')?;
        if self.next_non_whitespace() == Some('[') {
            self.parse_bracket_access()
        } else if self.next_non_whitespace().is_some_and(is_identifier_start) {
            Ok(Filter::Field(self.identifier()))
        } else {
            Ok(Filter::Identity)
        }
    }

    fn parse_interpolated_string(&mut self) -> Result<Filter, Error> {
        let start = self.offset;
        self.expect('"')?;
        let mut raw = String::new();
        let mut parts = Vec::new();
        loop {
            match self.peek() {
                Some('"') => {
                    self.bump();
                    if !raw.is_empty() || parts.is_empty() {
                        parts.push(InterpolationPart::Text(decode_string_fragment(
                            &raw, start,
                        )?));
                    }
                    return Ok(Filter::Interpolate(parts));
                }
                Some('\\') => {
                    self.bump();
                    if self.peek() == Some('(') {
                        self.bump();
                        if !raw.is_empty() {
                            parts.push(InterpolationPart::Text(decode_string_fragment(
                                &raw, start,
                            )?));
                            raw.clear();
                        }
                        parts.push(InterpolationPart::Filter(self.parse_comma()?));
                        self.expect(')')?;
                    } else {
                        raw.push('\\');
                        let Some(escaped) = self.peek() else {
                            return Err(ParseError::invalid_literal("unterminated escape", start));
                        };
                        raw.push(escaped);
                        self.bump();
                    }
                }
                Some(character) => {
                    raw.push(character);
                    self.bump();
                }
                None => return Err(ParseError::invalid_literal("unterminated string", start)),
            }
        }
    }

    fn parse_format(&mut self) -> Result<Filter, Error> {
        self.expect('@')?;
        let name = self.identifier_required()?;
        let format = match name.as_str() {
            "json" => Format::Json,
            "csv" => Format::Csv,
            "tsv" => Format::Tsv,
            "uri" => Format::Uri,
            "base64" => Format::Base64,
            _ => return Err(ParseError::expected("a known format filter", self.offset)),
        };
        Ok(Filter::Builtin(Builtin::Format(format), Vec::new()))
    }

    fn parse_named_or_control(&mut self) -> Result<Filter, Error> {
        if self.eat_word("if") {
            self.parse_if_tail()
        } else if self.eat_word("try") {
            self.parse_try()
        } else if self.eat_word("reduce") {
            self.parse_reduce()
        } else if self.eat_word("foreach") {
            self.parse_foreach()
        } else {
            self.parse_named_filter()
        }
    }

    fn parse_if_tail(&mut self) -> Result<Filter, Error> {
        let condition = self.parse_comma()?;
        self.expect_word("then")?;
        let then_branch = self.parse_comma()?;
        let else_branch = if self.eat_word("elif") {
            self.parse_if_tail()?
        } else {
            self.expect_word("else")?;
            let branch = self.parse_comma()?;
            self.expect_word("end")?;
            branch
        };
        Ok(Filter::If {
            condition: Box::new(condition),
            then_branch: Box::new(then_branch),
            else_branch: Box::new(else_branch),
        })
    }

    fn parse_try(&mut self) -> Result<Filter, Error> {
        let body = self.parse_comma()?;
        let handler = if self.eat_word("catch") {
            Some(Box::new(self.parse_comma()?))
        } else {
            None
        };
        Ok(Filter::Try {
            body: Box::new(body),
            handler,
        })
    }

    fn parse_reduce(&mut self) -> Result<Filter, Error> {
        let source = self.parse_alternative()?;
        self.expect_word("as")?;
        self.expect('$')?;
        let variable = self.identifier_required()?;
        self.expect('(')?;
        let initial = self.parse_comma()?;
        self.expect(';')?;
        let update = self.parse_comma()?;
        self.expect(')')?;
        Ok(Filter::Reduce {
            source: Box::new(source),
            variable,
            initial: Box::new(initial),
            update: Box::new(update),
        })
    }

    fn parse_foreach(&mut self) -> Result<Filter, Error> {
        let source = self.parse_alternative()?;
        self.expect_word("as")?;
        self.expect('$')?;
        let variable = self.identifier_required()?;
        self.expect('(')?;
        let initial = self.parse_comma()?;
        self.expect(';')?;
        let update = self.parse_comma()?;
        let extract = if self.eat(';') {
            self.parse_comma()?
        } else {
            Filter::Identity
        };
        self.expect(')')?;
        Ok(Filter::Foreach {
            source: Box::new(source),
            variable,
            initial: Box::new(initial),
            update: Box::new(update),
            extract: Box::new(extract),
        })
    }

    fn parse_bracket_access(&mut self) -> Result<Filter, Error> {
        self.expect('[')?;
        if self.eat(']') {
            return Ok(Filter::Iterate);
        }
        let start = self.optional_integer()?;
        if self.eat(':') {
            let end = self.optional_integer()?;
            self.expect(']')?;
            Ok(Filter::Slice(start, end))
        } else if let Some(index) = start {
            self.expect(']')?;
            Ok(Filter::Index(index))
        } else {
            Err(Error::new("expected an index or slice", self.offset))
        }
    }

    fn parse_array(&mut self) -> Result<Filter, Error> {
        self.expect('[')?;
        if self.eat(']') {
            return Ok(Filter::Literal(Value::Array(Vec::new())));
        }
        let inner = self.parse_comma()?;
        self.expect(']')?;
        Ok(Filter::Array(Box::new(inner)))
    }

    fn parse_object(&mut self) -> Result<Filter, Error> {
        self.expect('{')?;
        let mut fields = Vec::new();
        if self.eat('}') {
            return Ok(Filter::Object(fields));
        }
        loop {
            let name = if self.next_non_whitespace() == Some('"') {
                self.json_literal()?.as_str().unwrap().to_owned()
            } else {
                self.identifier_required()?
            };
            self.expect(':')?;
            fields.push((name, self.parse_pipe()?));
            if !self.eat(',') {
                break;
            }
        }
        self.expect('}')?;
        Ok(Filter::Object(fields))
    }

    fn parse_named_filter(&mut self) -> Result<Filter, Error> {
        let word = self.identifier();
        match word.as_str() {
            "null" => Ok(Filter::Literal(Value::Null)),
            "true" => Ok(Filter::Literal(Value::Bool(true))),
            "false" => Ok(Filter::Literal(Value::Bool(false))),
            "length" => Ok(Filter::Builtin(Builtin::Length, Vec::new())),
            "type" => Ok(Filter::Builtin(Builtin::Type, Vec::new())),
            "keys" => Ok(Filter::Builtin(Builtin::Keys, Vec::new())),
            "empty" => Ok(Filter::Builtin(Builtin::Empty, Vec::new())),
            "tostring" => Ok(Filter::Builtin(Builtin::ToString, Vec::new())),
            "paths" => Ok(Filter::Builtin(Builtin::Paths, Vec::new())),
            "sort" => Ok(Filter::Builtin(Builtin::Sort, Vec::new())),
            "unique" => Ok(Filter::Builtin(Builtin::Unique, Vec::new())),
            "min" => Ok(Filter::Builtin(Builtin::Min, Vec::new())),
            "max" => Ok(Filter::Builtin(Builtin::Max, Vec::new())),
            "add" if self.next_non_whitespace() == Some('(') => Ok(Filter::Call {
                name: word,
                arguments: self.parse_call_arguments()?,
            }),
            "add" => Ok(Filter::Builtin(Builtin::Add, Vec::new())),
            "tonumber" => Ok(Filter::Builtin(Builtin::ToNumber, Vec::new())),
            "fromjson" => Ok(Filter::Builtin(Builtin::FromJson, Vec::new())),
            "flatten" if self.next_non_whitespace() == Some('(') => {
                self.parse_builtin_arguments(Builtin::Flatten, 1)
            }
            "flatten" => Ok(Filter::Builtin(Builtin::Flatten, Vec::new())),
            "has" => self.parse_builtin_argument(Builtin::Has),
            "map" => self.parse_builtin_argument(Builtin::Map),
            "select" => self.parse_builtin_argument(Builtin::Select),
            "sort_by" => self.parse_builtin_arguments(Builtin::SortBy, 1),
            "group_by" => self.parse_builtin_arguments(Builtin::GroupBy, 1),
            "unique_by" => self.parse_builtin_arguments(Builtin::UniqueBy, 1),
            "contains" => self.parse_builtin_arguments(Builtin::Contains, 1),
            "inside" => self.parse_builtin_arguments(Builtin::Inside, 1),
            "startswith" => self.parse_builtin_arguments(Builtin::StartsWith, 1),
            "endswith" => self.parse_builtin_arguments(Builtin::EndsWith, 1),
            "split" => self.parse_builtin_arguments(Builtin::Split, 1),
            "join" => self.parse_builtin_arguments(Builtin::Join, 1),
            "test" => self.parse_builtin_argument_range(Builtin::Test, 1, 2),
            "match" => self.parse_builtin_argument_range(Builtin::Match, 1, 2),
            "capture" => self.parse_builtin_argument_range(Builtin::Capture, 1, 2),
            "scan" => self.parse_builtin_argument_range(Builtin::Scan, 1, 2),
            "sub" => self.parse_builtin_arguments(Builtin::Sub, 2),
            "gsub" => self.parse_builtin_arguments(Builtin::Gsub, 2),
            "del" => self.parse_builtin_arguments(Builtin::Del, 1),
            "getpath" => self.parse_builtin_arguments(Builtin::GetPath, 1),
            "setpath" => self.parse_builtin_arguments(Builtin::SetPath, 2),
            "error" if self.next_non_whitespace() == Some('(') => {
                self.parse_builtin_argument(Builtin::Error)
            }
            "error" => Ok(Filter::Builtin(Builtin::Error, Vec::new())),
            _ if self.next_non_whitespace() == Some('(') => Ok(Filter::Call {
                name: word,
                arguments: self.parse_call_arguments()?,
            }),
            _ => Ok(Filter::Call {
                name: word,
                arguments: Vec::new(),
            }),
        }
    }

    fn parse_call_arguments(&mut self) -> Result<Vec<Filter>, Error> {
        self.expect('(')?;
        let mut arguments = Vec::new();
        if self.eat(')') {
            return Ok(arguments);
        }
        loop {
            arguments.push(self.parse_comma()?);
            if self.eat(')') {
                break;
            }
            self.expect(';')?;
        }
        Ok(arguments)
    }

    fn parse_builtin_argument(&mut self, builtin: Builtin) -> Result<Filter, Error> {
        self.expect('(')?;
        let argument = self.parse_comma()?;
        self.expect(')')?;
        Ok(Filter::Builtin(builtin, vec![argument]))
    }

    fn parse_builtin_arguments(
        &mut self,
        builtin: Builtin,
        expected: usize,
    ) -> Result<Filter, Error> {
        let arguments = self.parse_call_arguments()?;
        if arguments.len() != expected {
            return Err(ParseError::expected(
                format!("{expected} function arguments"),
                self.offset,
            ));
        }
        Ok(Filter::Builtin(builtin, arguments))
    }

    fn parse_builtin_argument_range(
        &mut self,
        builtin: Builtin,
        minimum: usize,
        maximum: usize,
    ) -> Result<Filter, Error> {
        let arguments = self.parse_call_arguments()?;
        if !(minimum..=maximum).contains(&arguments.len()) {
            return Err(ParseError::expected(
                format!("{minimum} to {maximum} function arguments"),
                self.offset,
            ));
        }
        Ok(Filter::Builtin(builtin, arguments))
    }

    fn json_literal(&mut self) -> Result<Value, Error> {
        self.whitespace();
        let start = self.offset;
        if self.peek() == Some('"') {
            self.bump();
            let mut escaped = false;
            loop {
                match self.peek() {
                    Some('"') if !escaped => {
                        self.bump();
                        break;
                    }
                    Some('\\') if !escaped => {
                        escaped = true;
                        self.bump();
                    }
                    Some(_) => {
                        escaped = false;
                        self.bump();
                    }
                    None => return Err(ParseError::invalid_literal("unterminated string", start)),
                }
            }
        } else {
            if self.peek() == Some('-') {
                self.bump();
            }
            while self
                .peek()
                .is_some_and(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
            {
                self.bump();
            }
        }
        serde_json::from_str(&self.source[start..self.offset])
            .map_err(|error| ParseError::invalid_literal(error.to_string(), start))
    }

    fn optional_integer(&mut self) -> Result<Option<i64>, Error> {
        self.whitespace();
        let start = self.offset;
        if self.peek() == Some('-') {
            self.bump();
        }
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        if self.offset == start {
            return Ok(None);
        }
        if &self.source[start..self.offset] == "-" {
            return Err(Error::new("expected an integer", start));
        }
        self.source[start..self.offset]
            .parse()
            .map(Some)
            .map_err(|_| Error::new("invalid integer", start))
    }

    fn identifier_required(&mut self) -> Result<String, Error> {
        self.whitespace();
        if self.peek().is_some_and(is_identifier_start) {
            Ok(self.identifier())
        } else {
            Err(Error::new("expected an identifier", self.offset))
        }
    }

    fn identifier(&mut self) -> String {
        self.whitespace();
        let start = self.offset;
        self.bump();
        while self.peek().is_some_and(is_identifier_continue) {
            self.bump();
        }
        self.source[start..self.offset].to_owned()
    }

    fn expect(&mut self, expected: char) -> Result<(), Error> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(Error::new(format!("expected '{expected}'"), self.offset))
        }
    }

    fn whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn next_non_whitespace(&mut self) -> Option<char> {
        self.whitespace();
        self.peek()
    }

    fn eat(&mut self, expected: char) -> bool {
        self.whitespace();
        if self.peek() == Some(expected) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_str(&mut self, expected: &str) -> bool {
        self.whitespace();
        if self.source[self.offset..].starts_with(expected) {
            self.offset += expected.len();
            true
        } else {
            false
        }
    }

    fn starts_with(&mut self, expected: &str) -> bool {
        self.whitespace();
        self.source[self.offset..].starts_with(expected)
    }

    fn eat_word(&mut self, expected: &str) -> bool {
        self.whitespace();
        let rest = &self.source[self.offset..];
        if rest.starts_with(expected)
            && rest[expected.len()..]
                .chars()
                .next()
                .is_none_or(|c| !is_identifier_continue(c))
        {
            self.offset += expected.len();
            true
        } else {
            false
        }
    }

    fn expect_word(&mut self, expected: &str) -> Result<(), Error> {
        if self.eat_word(expected) {
            Ok(())
        } else {
            Err(ParseError::expected(expected, self.offset))
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }

    fn bump(&mut self) {
        if let Some(c) = self.peek() {
            self.offset += c.len_utf8();
        }
    }
}

fn is_identifier_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_identifier_continue(c: char) -> bool {
    is_identifier_start(c) || c.is_ascii_digit()
}

fn decode_string_fragment(raw: &str, offset: usize) -> Result<String, ParseError> {
    serde_json::from_str::<String>(&format!("\"{raw}\""))
        .map_err(|error| ParseError::invalid_literal(error.to_string(), offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn evaluate(source: &str, input: Value) -> Vec<Value> {
        super::evaluate(&compile(source).unwrap(), &input).unwrap()
    }

    #[test]
    fn access_chains_pipes_and_commas() {
        let input = json!({"users": [{"name": "Ada"}, {"name": "Grace"}], "missing": 0});
        assert_eq!(
            evaluate(".users[].name, .missing", input),
            [json!("Ada"), json!("Grace"), json!(0)]
        );
    }

    #[test]
    fn literals_do_not_depend_on_input() {
        assert_eq!(
            evaluate("null, true, false, 12.5, \"hei\"", json!(99)),
            [
                json!(null),
                json!(true),
                json!(false),
                json!(12.5),
                json!("hei")
            ]
        );
    }

    #[test]
    fn array_constructor_collects_result_stream() {
        assert_eq!(
            evaluate("[.[] | .name]", json!([{"name": "Ada"}, {"name": "Grace"}])),
            [json!(["Ada", "Grace"])]
        );
    }

    #[test]
    fn object_constructor_builds_cartesian_results() {
        assert_eq!(
            evaluate("{name: .name, choice: (1, 2)}", json!({"name": "Ada"})),
            [
                json!({"name": "Ada", "choice": 1}),
                json!({"name": "Ada", "choice": 2})
            ]
        );
    }

    #[test]
    fn array_and_unicode_string_slices() {
        assert_eq!(
            evaluate(".word[1:3]", json!({"word": "blåbær"})),
            [json!("lå")]
        );
        assert_eq!(evaluate(".[1:-1]", json!([0, 1, 2, 3])), [json!([1, 2])]);
        assert!(super::evaluate(&compile(".[1:2]").unwrap(), &json!({})).is_err());
    }

    #[test]
    fn optional_access_is_accepted() {
        assert_eq!(evaluate(".missing?", json!({})), [Value::Null]);
        assert!(evaluate(".[]?", Value::Null).is_empty());
    }

    #[test]
    fn comma_has_lower_precedence_than_pipe() {
        assert_eq!(
            evaluate(".user | .name, .missing", json!({"user": {"name": "Ada"}})),
            [json!("Ada"), Value::Null]
        );
    }

    #[test]
    fn indices_include_negative_indices() {
        assert_eq!(
            evaluate(".[0], .[-1], .[9]", json!([10, 20, 30])),
            [json!(10), json!(30), Value::Null]
        );
    }

    #[test]
    fn arithmetic_obeys_precedence_and_streams() {
        assert_eq!(
            evaluate("(.a, .b) * 2 + 1", json!({"a": 3, "b": 5})),
            [json!(7), json!(11)]
        );
        assert_eq!(evaluate("[1, 2] + [3]", Value::Null), [json!([1, 2, 3])]);
        assert_eq!(evaluate("\"jq\" + \"!\"", Value::Null), [json!("jq!")]);
    }

    #[test]
    fn comparisons_boolean_logic_and_alternatives() {
        assert_eq!(
            evaluate("3 > 2 and (null // 4) == 4", Value::Null),
            [json!(true)]
        );
        assert_eq!(
            evaluate("false, null, 0 // 9", Value::Null),
            [json!(false), json!(null), json!(0)]
        );
        assert_eq!(evaluate("not false", Value::Null), [json!(true)]);
    }

    #[test]
    fn checked_evaluation_reports_type_and_math_errors() {
        assert!(super::evaluate(&compile(".name").unwrap(), &json!(1)).is_err());
        assert_eq!(
            super::evaluate(&compile("1 / 0").unwrap(), &Value::Null)
                .unwrap_err()
                .to_string(),
            "division by zero"
        );
        assert!(
            super::evaluate(&compile(".[]?").unwrap(), &Value::Null)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn scalar_builtins_report_length_type_and_string() {
        assert_eq!(
            evaluate("length, type, tostring", json!("blåbær")),
            [json!(6), json!("string"), json!("blåbær")]
        );
        assert_eq!(evaluate("length", json!(-12)), [json!(12)]);
    }

    #[test]
    fn keys_and_has_support_objects_and_arrays() {
        assert_eq!(
            evaluate("keys, has(\"a\"), has(\"z\")", json!({"b": 2, "a": 1})),
            [json!(["a", "b"]), json!(true), json!(false)]
        );
        assert_eq!(
            evaluate("has(1), has(9)", json!([10, 20])),
            [json!(true), json!(false)]
        );
    }

    #[test]
    fn map_select_and_empty_preserve_stream_semantics() {
        assert_eq!(
            evaluate(
                "map(select(.active) | .name)",
                json!([
                    {"name": "Ada", "active": true},
                    {"name": "Bob", "active": false},
                    {"name": "Grace", "active": true}
                ])
            ),
            [json!(["Ada", "Grace"])]
        );
        assert!(evaluate("empty", json!(1)).is_empty());
    }

    #[test]
    fn builtins_return_structured_runtime_errors() {
        assert!(matches!(
            super::evaluate(&compile("keys").unwrap(), &json!(true)),
            Err(EvalError::InvalidBuiltin {
                name: "keys",
                input: "boolean"
            })
        ));
        assert_eq!(
            super::evaluate(&compile("error(\"stopp\")").unwrap(), &Value::Null)
                .unwrap_err()
                .to_string(),
            "stopp"
        );
    }

    #[test]
    fn variables_bind_each_value_without_changing_body_input() {
        assert_eq!(
            evaluate(
                ".users[] as $user | {name: $user.name, collection: .title}",
                json!({
                    "title": "Pionerar",
                    "users": [{"name": "Ada"}, {"name": "Grace"}]
                })
            ),
            [
                json!({"name": "Ada", "collection": "Pionerar"}),
                json!({"name": "Grace", "collection": "Pionerar"})
            ]
        );
    }

    #[test]
    fn bindings_compose_and_inner_bindings_shadow_outer_ones() {
        assert_eq!(
            evaluate(".a as $a | .b as $b | $a + $b", json!({"a": 2, "b": 3})),
            [json!(5)]
        );
        assert_eq!(
            evaluate("1 as $x | ((2 as $x | $x), $x)", Value::Null),
            [json!(2), json!(1)]
        );
    }

    #[test]
    fn undefined_variables_are_structured_errors() {
        assert_eq!(
            super::evaluate(&compile("$missing").unwrap(), &Value::Null).unwrap_err(),
            EvalError::UndefinedVariable("missing".to_owned())
        );
    }

    #[test]
    fn user_functions_support_zero_and_filter_arguments() {
        assert_eq!(
            evaluate("def increment: . + 1; increment", json!(4)),
            [json!(5)]
        );
        assert_eq!(
            evaluate("def twice(f): f, f; twice(.name)", json!({"name": "Ada"})),
            [json!("Ada"), json!("Ada")]
        );
        assert_eq!(
            evaluate("def add(f; g): f + g; add(.a; .b)", json!({"a": 2, "b": 3})),
            [json!(5)]
        );
    }

    #[test]
    fn definitions_are_lexically_scoped_and_can_call_prior_definitions() {
        assert_eq!(
            evaluate(
                "def increment: . + 1; def twice_increment: increment | increment; twice_increment",
                json!(10)
            ),
            [json!(12)]
        );
    }

    #[test]
    fn function_failures_are_structured() {
        assert!(matches!(
            super::evaluate(&compile("missing").unwrap(), &Value::Null),
            Err(EvalError::UndefinedFunction(name)) if name == "missing"
        ));
        assert!(matches!(
            super::evaluate(&compile("def one(f): f; one").unwrap(), &Value::Null),
            Err(EvalError::WrongArity {
                expected: 1,
                actual: 0,
                ..
            })
        ));
        assert!(matches!(
            super::evaluate(
                &compile("def forever: forever; forever").unwrap(),
                &Value::Null
            ),
            Err(EvalError::RecursionLimit {
                limit: RECURSION_LIMIT
            })
        ));
    }

    #[test]
    fn conditionals_support_elif_and_result_streams() {
        assert_eq!(
            evaluate(
                "if .score >= 90 then \"A\" elif .score >= 80 then \"B\" else \"C\" end",
                json!({"score": 85})
            ),
            [json!("B")]
        );
        assert_eq!(
            evaluate("if (true, false) then 1 else 0 end", Value::Null),
            [json!(1), json!(0)]
        );
    }

    #[test]
    fn try_catch_receives_runtime_error_as_input() {
        assert_eq!(
            evaluate(
                "try (.value / 0) catch {error: ., recovered: true}",
                json!({"value": 4})
            ),
            [json!({"error": "division by zero", "recovered": true})]
        );
        assert!(evaluate("try error(\"stopp\")", Value::Null).is_empty());
    }

    #[test]
    fn reduce_folds_streams_with_accumulator_input() {
        assert_eq!(
            evaluate("reduce .[] as $value (0; . + $value)", json!([1, 2, 3, 4])),
            [json!(10)]
        );
        assert_eq!(
            evaluate("reduce .[] as $item ([]; . + [$item])", json!(["a", "b"])),
            [json!(["a", "b"])]
        );
    }

    #[test]
    fn foreach_emits_each_intermediate_extraction() {
        assert_eq!(
            evaluate("foreach .[] as $value (0; . + $value)", json!([1, 2, 3])),
            [json!(1), json!(3), json!(6)]
        );
        assert_eq!(
            evaluate(
                "foreach .[] as $value (0; . + $value; {sum: ., item: $value})",
                json!([2, 3])
            ),
            [json!({"sum": 2, "item": 2}), json!({"sum": 5, "item": 3})]
        );
    }

    #[test]
    fn assignment_and_modify_update_nested_paths() {
        assert_eq!(
            evaluate(
                ".user.name = \"Grace\" | .user.score |= . * 2",
                json!({"user": {"name": "Ada", "score": 5}})
            ),
            [json!({"user": {"name": "Grace", "score": 10}})]
        );
        assert_eq!(
            evaluate(".created.deep = 42", json!({})),
            [json!({"created": {"deep": 42}})]
        );
        assert_eq!(evaluate(".[0] += 5", json!([5, 8])), [json!([10, 8])]);
    }

    #[test]
    fn compound_updates_share_binary_operator_rules() {
        assert_eq!(
            evaluate(
                ".count += 2 | .items += [3] | .name += \"!\"",
                json!({"count": 3, "items": [1, 2], "name": "jq"})
            ),
            [json!({"count": 5, "items": [1, 2, 3], "name": "jq!"})]
        );
    }

    #[test]
    fn del_and_empty_updates_remove_paths() {
        assert_eq!(
            evaluate(
                "del(.secret, .items[1])",
                json!({"secret": true, "items": [10, 20, 30]})
            ),
            [json!({"items": [10, 30]})]
        );
        assert_eq!(
            evaluate(".a |= empty", json!({"a": 1, "b": 2})),
            [json!({"b": 2})]
        );
    }

    #[test]
    fn path_builtins_round_trip_values() {
        let input = json!({"user": {"name": "Ada"}, "items": [10, 20]});
        assert_eq!(
            evaluate("getpath([\"user\", \"name\"])", input.clone()),
            [json!("Ada")]
        );
        assert_eq!(
            evaluate("setpath([\"items\", 1]; 99)", input.clone()),
            [json!({"user": {"name": "Ada"}, "items": [10, 99]})]
        );
        assert_eq!(
            evaluate("paths", json!({"a": [1]})),
            [json!(["a"]), json!(["a", 0])]
        );
    }

    #[test]
    fn sorting_grouping_and_uniqueness_use_total_value_order() {
        assert_eq!(evaluate("sort", json!([3, 1, 2, 1])), [json!([1, 1, 2, 3])]);
        assert_eq!(
            evaluate(
                "sort_by(.score) | map(.name)",
                json!([
                    {"name": "Ada", "score": 9}, {"name": "Bob", "score": 5}
                ])
            ),
            [json!(["Bob", "Ada"])]
        );
        assert_eq!(
            evaluate(
                "group_by(.kind) | map(map(.value))",
                json!([
                    {"kind": "b", "value": 3}, {"kind": "a", "value": 1},
                    {"kind": "a", "value": 2}
                ])
            ),
            [json!([[1, 2], [3]])]
        );
        assert_eq!(evaluate("unique", json!([3, 1, 3, 2])), [json!([1, 2, 3])]);
    }

    #[test]
    fn aggregates_and_flatten_transform_arrays() {
        assert_eq!(
            evaluate("add, min, max", json!([3, 1, 2])),
            [json!(6), json!(1), json!(3)]
        );
        assert_eq!(
            evaluate("flatten", json!([1, [2, [3]]])),
            [json!([1, 2, 3])]
        );
        assert_eq!(
            evaluate("flatten(1)", json!([1, [2, [3]]])),
            [json!([1, 2, [3]])]
        );
    }

    #[test]
    fn containment_and_text_builtins_cover_common_queries() {
        assert_eq!(
            evaluate(
                "contains({a: [2]}), inside({a: [1, 2, 3]})",
                json!({"a": [1, 2]})
            ),
            [json!(true), json!(true)]
        );
        assert_eq!(
            evaluate(
                "startswith(\"Blue\"), endswith(\"1\"), split(\" \" )",
                json!("Blue Star 1")
            ),
            [json!(true), json!(true), json!(["Blue", "Star", "1"])]
        );
        assert_eq!(
            evaluate("join(\"-\")", json!(["a", 2, null])),
            [json!("a-2-")]
        );
        assert_eq!(evaluate("tonumber", json!("12.5")), [json!(12.5)]);
    }

    #[test]
    fn interpolation_preserves_filter_streams_and_json_escaping() {
        assert_eq!(
            evaluate("\"Hello \\(.name)!\"", json!({"name": "Ada"})),
            [json!("Hello Ada!")]
        );
        assert_eq!(
            evaluate("\"value=\\(.a, .b)\"", json!({"a": 1, "b": true})),
            [json!("value=1"), json!("value=true")]
        );
        assert_eq!(
            evaluate("\"line\\n\\(. + 1)\"", json!(2)),
            [json!("line\n3")]
        );
    }

    #[test]
    fn regex_builtins_match_capture_scan_and_replace() {
        assert_eq!(
            evaluate("test(\"star\"; \"i\")", json!("Blue Star 1")),
            [json!(true)]
        );
        assert_eq!(
            evaluate(
                "[match(\"[A-Za-z]+\"; \"g\") | .string]",
                json!("Blue Star 1")
            ),
            [json!(["Blue", "Star"])]
        );
        assert_eq!(
            evaluate(
                "capture(\"(?P<ship>Blue Star) (?P<number>[0-9]+)\")",
                json!("Blue Star 1")
            ),
            [json!({"ship": "Blue Star", "number": "1"})]
        );
        assert_eq!(
            evaluate("[scan(\"[0-9]+\")]", json!("A1 B22")),
            [json!(["1", "22"])]
        );
        assert_eq!(
            evaluate("gsub(\" smoke\"; \"\")", json!("whisky smoke smoke")),
            [json!("whisky")]
        );
    }

    #[test]
    fn format_filters_and_fromjson_round_trip_common_values() {
        assert_eq!(evaluate("@json", json!({"a": 1})), [json!("{\"a\":1}")]);
        assert_eq!(
            evaluate("@csv", json!(["Ada", 2, null])),
            [json!("\"Ada\",2,")]
        );
        assert_eq!(evaluate("@tsv", json!(["a\tb", 2])), [json!("a\\tb\t2")]);
        assert_eq!(evaluate("@uri", json!("blue star")), [json!("blue%20star")]);
        assert_eq!(evaluate("@base64", json!("jq")), [json!("anE=")]);
        assert_eq!(evaluate("fromjson", json!("[1,true]")), [json!([1, true])]);
    }
}
