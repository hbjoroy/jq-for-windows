#![forbid(unsafe_code)]

use jq_for_windows::{compile, evaluate_with_variables};
use serde_json::Value;
use std::fs;
use std::io::{self, Read};
use std::process::ExitCode;

#[derive(Debug, Default)]
struct CliOptions {
    compact_output: bool,
    raw_output: bool,
    raw_input: bool,
    null_input: bool,
    slurp: bool,
    exit_status: bool,
    filter: Option<String>,
    files: Vec<String>,
    variables: Vec<(String, Value)>,
}

#[derive(Debug)]
struct CliError {
    message: String,
    code: u8,
}

impl CliError {
    fn new(message: impl Into<String>, code: u8) -> Self {
        Self {
            message: message.into(),
            code,
        }
    }
}

fn main() -> ExitCode {
    match execute() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("jq: error: {}", error.message);
            ExitCode::from(error.code)
        }
    }
}

fn execute() -> Result<u8, CliError> {
    let Some(options) = parse_options()? else {
        return Ok(0);
    };
    let filter = compile(options.filter.as_deref().unwrap_or("."))
        .map_err(|error| CliError::new(format!("compile error: {error}"), 3))?;
    let inputs = read_inputs(&options)?;
    let mut last_output = None;

    for input in inputs {
        for output in evaluate_with_variables(&filter, &input, &options.variables)
            .map_err(|error| CliError::new(error.to_string(), 5))?
        {
            write_output(&output, &options)?;
            last_output = Some(output);
        }
    }

    if options.exit_status {
        Ok(match last_output {
            Some(Value::Null | Value::Bool(false)) => 1,
            Some(_) => 0,
            None => 4,
        })
    } else {
        Ok(0)
    }
}

fn parse_options() -> Result<Option<CliOptions>, CliError> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let mut options = CliOptions::default();
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        match argument.as_str() {
            "-c" | "--compact-output" => options.compact_output = true,
            "-r" | "--raw-output" => options.raw_output = true,
            "-R" | "--raw-input" => options.raw_input = true,
            "-n" | "--null-input" => options.null_input = true,
            "-s" | "--slurp" => options.slurp = true,
            "-e" | "--exit-status" => options.exit_status = true,
            "--arg" | "--argjson" => {
                let name = arguments.get(index + 1).ok_or_else(|| {
                    CliError::new(format!("{argument} requires a name and value"), 2)
                })?;
                let source = arguments.get(index + 2).ok_or_else(|| {
                    CliError::new(format!("{argument} requires a name and value"), 2)
                })?;
                let value = if argument == "--argjson" {
                    serde_json::from_str(source).map_err(|error| {
                        CliError::new(format!("invalid JSON for --argjson {name}: {error}"), 2)
                    })?
                } else {
                    Value::String(source.clone())
                };
                options.variables.push((name.clone(), value));
                index += 2;
            }
            "--version" => {
                println!("jq-for-windows-{}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            value if value.starts_with('-') => {
                return Err(CliError::new(format!("unknown option {value}"), 2));
            }
            value if options.filter.is_none() => options.filter = Some(value.to_owned()),
            value => options.files.push(value.to_owned()),
        }
        index += 1;
    }
    Ok(Some(options))
}

fn read_inputs(options: &CliOptions) -> Result<Vec<Value>, CliError> {
    if options.null_input {
        return Ok(vec![Value::Null]);
    }
    let sources = read_sources(&options.files)?;
    if options.raw_input {
        let combined = sources.concat();
        if options.slurp {
            return Ok(vec![Value::String(combined)]);
        }
        return Ok(combined
            .split_terminator('\n')
            .map(|line| {
                let line = if cfg!(windows) {
                    line.strip_suffix('\r').unwrap_or(line)
                } else {
                    line
                };
                Value::String(line.to_owned())
            })
            .collect());
    }

    let mut values = Vec::new();
    for source in sources {
        values.extend(
            serde_json::Deserializer::from_str(&source)
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| CliError::new(format!("parse error: {error}"), 5))?,
        );
    }
    Ok(if options.slurp {
        vec![Value::Array(values)]
    } else {
        values
    })
}

fn read_sources(files: &[String]) -> Result<Vec<String>, CliError> {
    if files.is_empty() {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .map_err(|error| CliError::new(error.to_string(), 2))?;
        Ok(vec![source])
    } else {
        files
            .iter()
            .map(|path| {
                fs::read_to_string(path)
                    .map_err(|error| CliError::new(format!("{path}: {error}"), 2))
            })
            .collect()
    }
}

fn write_output(output: &Value, options: &CliOptions) -> Result<(), CliError> {
    let serialized = if options.raw_output {
        output
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| output.to_string())
    } else if options.compact_output {
        serde_json::to_string(output).map_err(|error| CliError::new(error.to_string(), 5))?
    } else {
        serde_json::to_string_pretty(output).map_err(|error| CliError::new(error.to_string(), 5))?
    };
    println!("{serialized}");
    Ok(())
}

fn print_help() {
    println!(
        "jq-for-windows {}\n\nUsage: jq [OPTIONS] [FILTER] [FILES...]\n\nOptions:\n  -c, --compact-output  Emit compact JSON\n  -r, --raw-output      Emit strings without JSON quotes\n  -R, --raw-input       Read input as text lines\n  -s, --slurp           Read all inputs into one array or string\n  -n, --null-input      Use null instead of reading input\n  -e, --exit-status     Set status from the last output\n      --arg NAME VALUE  Bind VALUE as a string\n      --argjson N JSON  Bind a parsed JSON value\n  -h, --help            Print help\n      --version         Print version",
        env!("CARGO_PKG_VERSION")
    );
}
