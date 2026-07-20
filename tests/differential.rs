use jq_for_windows::{compile, evaluate};
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

fn corpus() -> Value {
    serde_json::from_str(include_str!("../corpus/cases.json")).unwrap()
}

#[test]
fn generated_corpus_has_valid_metadata_and_unique_ids() {
    let corpus = corpus();
    assert_eq!(corpus["schema"], 1);
    assert_eq!(corpus["reference"], "jq-1.7.1");
    let cases = corpus["cases"].as_array().unwrap();
    assert!(cases.len() >= 100, "corpus unexpectedly shrank");
    let mut ids = std::collections::BTreeSet::new();
    for case in cases {
        let id = case["id"].as_str().unwrap();
        assert!(ids.insert(id), "duplicate corpus id: {id}");
        assert!(case["category"].is_string(), "missing category for {id}");
        assert!(case["filter"].is_string(), "missing filter for {id}");
        assert!(case.get("input").is_some(), "missing input for {id}");
    }
}

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

    let corpus = corpus();
    let cases = corpus["cases"].as_array().unwrap();
    let mut categories = std::collections::BTreeMap::<&str, usize>::new();

    for case in cases {
        let id = case["id"].as_str().unwrap();
        let category = case["category"].as_str().unwrap();
        let filter = case["filter"].as_str().unwrap();
        let input_value = case["input"].clone();
        *categories.entry(category).or_default() += 1;
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
            .write_all(serde_json::to_string(&input_value).unwrap().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "reference jq rejected {id}: {filter}"
        );
        let reference_values = serde_json::Deserializer::from_slice(&output.stdout)
            .into_iter()
            .collect::<Result<Vec<Value>, _>>()
            .unwrap();
        assert_eq!(
            ours, reference_values,
            "differential mismatch for {id}: {filter}"
        );
    }

    eprintln!(
        "matched {} generated cases across {} categories: {categories:?}",
        cases.len(),
        categories.len()
    );
}
