use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

fn jq() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jq"))
}

fn run_with_input(arguments: &[&str], input: &str) -> std::process::Output {
    let mut child = jq()
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn arg_and_argjson_bind_typed_variables() {
    let output = jq()
        .args([
            "-n",
            "--arg",
            "name",
            "Ada",
            "--argjson",
            "score",
            "9",
            "-c",
            "{name: $name, score: $score}",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "{\"name\":\"Ada\",\"score\":9}\n"
    );
}

#[test]
fn raw_slurp_and_file_input_are_supported() {
    let output = run_with_input(&["-R", "-s", "-r", "."], "one\ntwo\n");
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "one\ntwo\n\n");

    let path = std::env::temp_dir().join(format!("jq-for-windows-{}.json", std::process::id()));
    fs::write(&path, "{\"value\":42}").unwrap();
    let output = jq()
        .args(["-c", ".value", path.to_str().unwrap()])
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "42\n");
}

#[test]
fn exit_status_distinguishes_false_and_empty_streams() {
    let false_output = run_with_input(&["-e", ".ok"], "{\"ok\":false}");
    assert_eq!(false_output.status.code(), Some(1));

    let empty_output = run_with_input(&["-e", ".missing | empty"], "{}");
    assert_eq!(empty_output.status.code(), Some(4));
}

#[test]
fn invalid_json_uses_runtime_error_status() {
    let output = run_with_input(&["."], "{");
    assert_eq!(output.status.code(), Some(5));
    assert!(!output.stderr.is_empty());
}

#[test]
fn generated_cli_corpus_matches_reference_jq_when_available() {
    let reference = std::env::var("JQ_REFERENCE").unwrap_or_else(|_| "jq".to_owned());
    let Ok(version) = Command::new(&reference).arg("--version").output() else {
        eprintln!("skipping differential CLI corpus: set JQ_REFERENCE to upstream jq");
        return;
    };
    if String::from_utf8_lossy(&version.stdout).contains("jq-for-windows") {
        return;
    }
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("../corpus/cases.json")).unwrap();
    for case in corpus["cli_cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let args = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap())
            .collect::<Vec<_>>();
        let stdin = case["stdin"].as_str().unwrap();
        let ours = run_command(jq(), &args, stdin);
        let reference = run_command(Command::new(&reference), &args, stdin);
        assert_eq!(
            ours.status.code(),
            reference.status.code(),
            "exit status mismatch for {id}"
        );
        assert_eq!(
            normalize_newlines(&ours.stdout),
            normalize_newlines(&reference.stdout),
            "stdout mismatch for {id}"
        );
        assert_eq!(
            ours.stderr.is_empty(),
            reference.stderr.is_empty(),
            "stderr presence mismatch for {id}"
        );
    }
}

fn normalize_newlines(bytes: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .into_bytes()
}

fn run_command(mut command: Command, arguments: &[&str], input: &str) -> std::process::Output {
    let mut child = command
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}
