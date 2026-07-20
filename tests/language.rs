use jq_for_windows::{EvalError, compile, evaluate as evaluate_filter};
use serde_json::{Value, json};

fn evaluate(source: &str, input: Value) -> Vec<Value> {
    evaluate_filter(&compile(source).unwrap(), &input).unwrap()
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
    assert!(evaluate_filter(&compile(".[1:2]").unwrap(), &json!({})).is_err());
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
    assert!(evaluate_filter(&compile(".name").unwrap(), &json!(1)).is_err());
    assert_eq!(
        evaluate_filter(&compile("1 / 0").unwrap(), &Value::Null)
            .unwrap_err()
            .to_string(),
        "division by zero"
    );
    assert!(
        evaluate_filter(&compile(".[]?").unwrap(), &Value::Null)
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
        evaluate_filter(&compile("keys").unwrap(), &json!(true)),
        Err(EvalError::InvalidBuiltin {
            name: "keys",
            input: "boolean"
        })
    ));
    assert_eq!(
        evaluate_filter(&compile("error(\"stopp\")").unwrap(), &Value::Null)
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
        evaluate_filter(&compile("$missing").unwrap(), &Value::Null).unwrap_err(),
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
        evaluate_filter(&compile("missing").unwrap(), &Value::Null),
        Err(EvalError::UndefinedFunction(name)) if name == "missing"
    ));
    assert!(matches!(
        evaluate_filter(&compile("def one(f): f; one").unwrap(), &Value::Null),
        Err(EvalError::WrongArity {
            expected: 1,
            actual: 0,
            ..
        })
    ));
    assert!(matches!(
        evaluate_filter(
            &compile("def forever: forever; forever").unwrap(),
            &Value::Null
        ),
        Err(EvalError::RecursionLimit { limit: 64 })
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
