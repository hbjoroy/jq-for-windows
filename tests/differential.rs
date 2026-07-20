use jq_for_windows::{compile, evaluate};
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn compatibility_corpus_matches_reference_jq_when_available() {
    let reference = std::env::var("JQ_REFERENCE").unwrap_or_else(|_| "jq".to_owned());
    let Ok(version) = Command::new(&reference).arg("--version").output() else {
        eprintln!("skipping differential corpus: set JQ_REFERENCE to an upstream jq executable");
        return;
    };
    if String::from_utf8_lossy(&version.stdout).contains("jq-for-windows") {
        eprintln!(
            "skipping differential corpus: {reference} resolves to this project, not upstream jq"
        );
        return;
    }

    let cases = [
        (r#"{"a":1}"#, "."),
        (r#"{"a":1}"#, ".missing"),
        ("[10,20,30]", ".[0], .[-1], .[9]"),
        ("[0,1,2,3]", ".[1:-1]"),
        (
            r#"{"users":[{"name":"Ada"},{"name":"Grace"}]}"#,
            ".users[].name",
        ),
        (r#"{"a":2,"b":3}"#, "[.a, .b, (.a + .b), (.a * .b)]"),
        ("null", "false // 9, null // 8, 0 // 7"),
        ("null", "3 > 2 and 1 != 0"),
        ("[1,2,3,4]", "map(select(. > 2) | . * 10)"),
        (r#"{"b":2,"a":1}"#, "keys, length, type"),
        ("[1,2,3,4]", "reduce .[] as $value (0; . + $value)"),
        ("[1,2,3]", "foreach .[] as $value (0; . + $value)"),
        (
            r#"[{"kind":"b","n":2},{"kind":"a","n":1}]"#,
            "sort_by(.kind)",
        ),
        ("[3,1,3,2]", "sort, unique, min, max, add"),
        ("[1,[2,[3]]]", "flatten, flatten(1)"),
        (r#"{"a":[1,2]}"#, "contains({a: [2]})"),
        (
            r#""Blue Star 1""#,
            r#"split(" "), startswith("Blue"), endswith("1")"#,
        ),
        (r#"{"name":"Ada","score":9}"#, r#""\(.name): \(.score)""#),
        (
            r#"{"score":85}"#,
            r#"if .score >= 90 then "A" elif .score >= 80 then "B" else "C" end"#,
        ),
        (r#"{"a":1,"nested":{"b":2}}"#, ".nested.b += 3"),
        (r#"{"a":1,"secret":true}"#, "del(.secret)"),
        (
            r#"{"a":{"b":2}}"#,
            r#"getpath(["a","b"]), setpath(["a","c"]; 3)"#,
        ),
        (r#""Blue Star 1""#, r#"test("star"; "i"), gsub(" "; "-")"#),
        (r#"{"a":1}"#, "@json"),
        (r#""jq""#, "@base64"),
        (r#""[1,true]""#, "fromjson"),
    ];

    for (input, filter) in cases {
        let input_value: Value = serde_json::from_str(input).unwrap();
        let ours = evaluate(&compile(filter).unwrap(), &input_value).unwrap();
        let mut child = Command::new(&reference)
            .args(["-c", filter])
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
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "reference jq rejected {filter}");
        let reference_values = serde_json::Deserializer::from_slice(&output.stdout)
            .into_iter()
            .collect::<Result<Vec<Value>, _>>()
            .unwrap();
        assert_eq!(ours, reference_values, "differential mismatch for {filter}");
    }
}
