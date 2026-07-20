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
