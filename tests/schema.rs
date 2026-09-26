use std::io::Write;
use std::process::{Command, Stdio};

use jsonl_peek::json::{self, Value};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jsonl-peek"))
}

fn schema_on(input: &[u8], extra_args: &[&str]) -> String {
    let mut child = bin()
        .arg("schema")
        .args(extra_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn jsonl-peek schema");
    child.stdin.as_mut().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn schema_reports_record_count_and_paths() {
    let stdout = schema_on(b"{\"id\":1,\"tags\":[\"a\",\"b\"]}\n{\"id\":2}\n", &[]);
    assert!(stdout.contains("2 records, depth 3"));
    assert!(stdout.find("  id ").is_some());
    assert!(stdout.contains("100.0%  int:2"));
    assert!(stdout.contains("50.0%  array:1"));
    assert!(stdout.contains("50.0%  string:2"));
    // "tags" comes before "tags[]" which comes before nothing further here,
    // and both sort after "id" alphabetically.
    assert!(stdout.find("id").unwrap() < stdout.find("tags").unwrap());
}

#[test]
fn schema_reads_a_file_and_walks_nested_structure() {
    let output = bin()
        .args(["schema", "fixtures/sample.jsonl"])
        .output()
        .expect("run jsonl-peek schema");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("5 records, depth 3"));
    assert!(stdout.contains("100.0%  int:5"));
    assert!(stdout.contains("100.0%  string:5"));
}

#[test]
fn schema_depth_limits_how_far_paths_descend() {
    let stdout = schema_on(b"{\"a\":{\"b\":{\"c\":1}}}\n", &["--depth", "2"]);
    assert!(stdout.contains("  a "));
    assert!(stdout.contains("  a.b "));
    assert!(!stdout.contains("a.b.c"));
}

#[test]
fn schema_min_rate_hides_sparse_paths() {
    let stdout = schema_on(
        b"{\"id\":1,\"tags\":[\"a\"]}\n{\"id\":2}\n{\"id\":3}\n{\"id\":4}\n",
        &["--min-rate", "0.5"],
    );
    assert!(stdout.contains("  id "));
    assert!(!stdout.contains("tags"));
}

#[test]
fn schema_reports_unparseable_lines_without_counting_them_as_records() {
    let stdout = schema_on(b"{\"a\":1}\n\nbad\n", &[]);
    assert!(stdout.contains("1 records, depth 3"));
    assert!(stdout.contains("1 unparseable lines skipped"));
}

#[test]
fn schema_rejects_an_invalid_depth_as_a_usage_error() {
    let output = bin()
        .args(["schema", "--depth", "not-a-number"])
        .output()
        .expect("run jsonl-peek schema");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn schema_reports_a_missing_file_as_a_runtime_error() {
    let output = bin()
        .args(["schema", "fixtures/does-not-exist.jsonl"])
        .output()
        .expect("run jsonl-peek schema");
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn schema_json_output_is_valid_json_with_the_expected_fields() {
    let stdout = schema_on(b"{\"id\":1,\"tags\":[\"a\",\"b\"]}\n{\"id\":2}\nbad\n", &["--json"]);
    let value = json::parse(stdout.trim().as_bytes())
        .expect("schema --json output should itself be valid json");

    assert_eq!(value.get("records"), Some(&Value::Int(2)));
    assert_eq!(value.get("unparseable"), Some(&Value::Int(1)));
    assert_eq!(value.get("truncated"), Some(&Value::Bool(false)));

    let paths = value.get("paths").unwrap().as_array().unwrap();
    let id = paths.iter().find(|p| p.get("path") == Some(&Value::String("id".to_string()))).unwrap();
    assert_eq!(id.get("records_present"), Some(&Value::Int(2)));
    assert_eq!(id.get("types").unwrap().get("int"), Some(&Value::Int(2)));

    let tags_items = paths
        .iter()
        .find(|p| p.get("path") == Some(&Value::String("tags[]".to_string())))
        .unwrap();
    assert_eq!(tags_items.get("records_present"), Some(&Value::Int(1)));
    assert_eq!(tags_items.get("types").unwrap().get("string"), Some(&Value::Int(2)));
}
