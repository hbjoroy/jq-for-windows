use crate::ast::BinaryOp;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub(crate) kind: ParseErrorKind,
    pub(crate) offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
    Expected(String),
    InvalidLiteral(String),
    UnknownIdentifier(String),
    UnexpectedInput,
}

impl ParseError {
    pub(crate) fn new(message: impl Into<String>, offset: usize) -> Self {
        Self::expected(message, offset)
    }
    pub(crate) fn expected(expected: impl Into<String>, offset: usize) -> Self {
        Self {
            kind: ParseErrorKind::Expected(expected.into()),
            offset,
        }
    }

    pub(crate) fn invalid_literal(message: impl Into<String>, offset: usize) -> Self {
        Self {
            kind: ParseErrorKind::InvalidLiteral(message.into()),
            offset,
        }
    }

    pub(crate) fn unexpected(offset: usize) -> Self {
        Self {
            kind: ParseErrorKind::UnexpectedInput,
            offset,
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match &self.kind {
            ParseErrorKind::Expected(value) => format!("expected {value}"),
            ParseErrorKind::InvalidLiteral(value) => format!("invalid literal: {value}"),
            ParseErrorKind::UnknownIdentifier(value) => format!("unknown identifier {value}"),
            ParseErrorKind::UnexpectedInput => "unexpected input".to_owned(),
        };
        write!(f, "{message} at character {}", self.offset + 1)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    InvalidIndex {
        input: &'static str,
        index: &'static str,
    },
    InvalidIteration {
        input: &'static str,
    },
    InvalidSlice {
        input: &'static str,
    },
    InvalidUnary {
        input: &'static str,
        operation: &'static str,
    },
    InvalidBinary {
        left: &'static str,
        right: &'static str,
        operation: BinaryOp,
    },
    DivisionByZero,
    InvalidNumber,
    InvalidBuiltin {
        name: &'static str,
        input: &'static str,
    },
    Explicit(String),
    Serialization(String),
    UndefinedVariable(String),
    UndefinedFunction(String),
    WrongArity {
        name: String,
        expected: usize,
        actual: usize,
    },
    RecursionLimit {
        limit: usize,
    },
    InvalidPath(String),
    InvalidRegex(String),
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIndex { input, index } => write!(f, "cannot index {input} with {index}"),
            Self::InvalidIteration { input } => write!(f, "cannot iterate over {input}"),
            Self::InvalidSlice { input } => write!(f, "cannot slice {input}"),
            Self::InvalidUnary { input, operation } => {
                write!(f, "{input} cannot be used with {operation}")
            }
            Self::InvalidBinary {
                left,
                right,
                operation,
            } => {
                write!(f, "{left} and {right} cannot be used with {operation:?}")
            }
            Self::DivisionByZero => f.write_str("division by zero"),
            Self::InvalidNumber => f.write_str("operation produced a non-JSON number"),
            Self::InvalidBuiltin { name, input } => {
                write!(f, "{name} cannot be applied to {input}")
            }
            Self::Explicit(message) => f.write_str(message),
            Self::Serialization(message) => write!(f, "JSON serialization failed: {message}"),
            Self::UndefinedVariable(name) => write!(f, "${name} is not defined"),
            Self::UndefinedFunction(name) => write!(f, "{name} is not defined"),
            Self::WrongArity {
                name,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "{name}/{actual} is not defined (expected {expected} arguments)"
                )
            }
            Self::RecursionLimit { limit } => {
                write!(f, "function recursion limit ({limit}) exceeded")
            }
            Self::InvalidPath(message) => write!(f, "invalid path: {message}"),
            Self::InvalidRegex(message) => write!(f, "invalid regular expression: {message}"),
        }
    }
}

impl std::error::Error for EvalError {}
