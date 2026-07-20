use serde_json::{Value, json};
use std::path::PathBuf;

fn case(id: impl Into<String>, category: &str, input: Value, filter: impl Into<String>) -> Value {
    json!({"id": id.into(), "category": category, "input": input, "filter": filter.into()})
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut cases = vec![
        case("identity", "core", json!({"a": 1}), "."),
        case("missing-field", "core", json!({"a": 1}), ".missing"),
        case(
            "field-chain",
            "core",
            json!({"users": [{"name": "Ada"}, {"name": "Grace"}]}),
            ".users[].name",
        ),
        case("array-slice", "core", json!([0, 1, 2, 3]), ".[1:-1]"),
        case(
            "construct-array",
            "constructors",
            json!({"a": 2, "b": 3}),
            "[.a, .b, (.a + .b), (.a * .b)]",
        ),
        case(
            "alternative",
            "operators",
            Value::Null,
            "false // 9, null // 8, 0 // 7",
        ),
        case("boolean", "operators", Value::Null, "3 > 2 and 1 != 0"),
        case(
            "map-select",
            "streams",
            json!([1, 2, 3, 4]),
            "map(select(. > 2) | . * 10)",
        ),
        case(
            "reduce",
            "streams",
            json!([1, 2, 3, 4]),
            "reduce .[] as $value (0; . + $value)",
        ),
        case(
            "foreach",
            "streams",
            json!([1, 2, 3]),
            "foreach .[] as $value (0; . + $value)",
        ),
        case(
            "sorting",
            "collections",
            json!([3, 1, 3, 2]),
            "sort, unique, min, max, add",
        ),
        case(
            "flatten",
            "collections",
            json!([1, [2, [3]]]),
            "flatten, flatten(1)",
        ),
        case(
            "containment",
            "collections",
            json!({"a": [1, 2]}),
            "contains({a: [2]})",
        ),
        case(
            "text",
            "strings",
            json!("Blue Star 1"),
            r#"split(" "), startswith("Blue"), endswith("1")"#,
        ),
        case(
            "interpolation",
            "strings",
            json!({"name": "Ada", "score": 9}),
            r#""\(.name): \(.score)""#,
        ),
        case(
            "conditional",
            "control",
            json!({"score": 85}),
            r#"if .score >= 90 then "A" elif .score >= 80 then "B" else "C" end"#,
        ),
        case(
            "nested-update",
            "updates",
            json!({"a": 1, "nested": {"b": 2}}),
            ".nested.b += 3",
        ),
        case(
            "delete",
            "updates",
            json!({"a": 1, "secret": true}),
            "del(.secret)",
        ),
        case(
            "paths",
            "paths",
            json!({"a": {"b": 2}}),
            r#"getpath(["a","b"]), setpath(["a","c"]; 3)"#,
        ),
        case(
            "regex",
            "regex",
            json!("Blue Star 1"),
            r#"test("star"; "i"), gsub(" "; "-")"#,
        ),
        case("formats", "formats", json!({"a": 1}), "@json"),
        case("base64", "formats", json!("jq"), "@base64"),
        case("fromjson", "formats", json!("[1,true]"), "fromjson"),
    ];

    for left in -4..=4 {
        for right in -3..=3 {
            cases.push(case(
                format!("add-{left}-{right}"),
                "arithmetic",
                Value::Null,
                format!("{left} + {right}"),
            ));
            cases.push(case(
                format!("compare-{left}-{right}"),
                "comparison",
                Value::Null,
                format!("[{left} < {right}, {left} == {right}, {left} >= {right}]"),
            ));
        }
    }
    for index in -5..=5 {
        cases.push(case(
            format!("index-{index}"),
            "indexing",
            json!(["a", "b", "c"]),
            format!(".[{index}]"),
        ));
    }
    for value in [
        Value::Null,
        json!(false),
        json!(0),
        json!(""),
        json!([]),
        json!({}),
        json!([1, 2]),
    ] {
        let kind = match &value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        cases.push(case(
            format!("type-{kind}-{}", cases.len()),
            "types",
            value,
            "type",
        ));
    }
    for (id, value) in [
        ("null", Value::Null),
        ("number", json!(-7)),
        ("string", json!("αβ")),
        ("array", json!([1, 2, 3])),
        ("object", json!({"a": 1, "b": 2})),
    ] {
        cases.push(case(format!("length-{id}"), "types", value, "length"));
    }

    cases.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let document = json!({"schema": 1, "reference": "jq-1.7.1", "cases": cases});
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/cases.json");
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&document)?),
    )?;
    println!(
        "generated {} cases in {}",
        document["cases"].as_array().unwrap().len(),
        path.display()
    );
    Ok(())
}
