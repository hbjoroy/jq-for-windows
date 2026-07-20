#![forbid(unsafe_code)]

mod ast;
mod environment;
mod error;
mod eval;
mod parser;

pub use ast::{BinaryOp, Builtin, Filter, Format, InterpolationPart, UnaryOp, UpdateOp};
pub use error::{EvalError, ParseError, ParseErrorKind};
use serde_json::Value;

pub fn compile(source: &str) -> Result<Filter, ParseError> {
    parser::compile(source)
}

pub fn evaluate(filter: &Filter, input: &Value) -> Result<Vec<Value>, EvalError> {
    eval::evaluate(filter, input)
}

pub fn evaluate_with_variables(
    filter: &Filter,
    input: &Value,
    variables: &[(String, Value)],
) -> Result<Vec<Value>, EvalError> {
    eval::evaluate_with_variables(filter, input, variables)
}
