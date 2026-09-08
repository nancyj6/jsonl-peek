use std::io::Write;
use std::process::{Command, Stdio};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jsonl-peek"))
}

fn stats_on(input: &[u8], extra_args: &[&str]) -> String {
    let mut child = bin()
        .arg("stats")
        .args(extra_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn jsonl-peek stats");
    child.stdin.as_mut().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn stats_reports_line_and_type_counts() {
    let stdout = stats_on(b"{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n", &[]);
    assert!(stdout.contains("file    -"));
    assert!(stdout.contains("blank 0   invalid 0   valid 3"));
    assert!(stdout.contains("top level  object:3"));
    assert!(stdout.contains("line length in bytes"));
}

#[test]
fn stats_reads_a_file_and_reports_its_size() {
    let output = bin()
        .args(["stats", "fixtures/sample.jsonl"])
        .output()
        .expect("run jsonl-peek stats");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("file    fixtures/sample.jsonl"));
    assert!(stdout.contains("blank 0   invalid 0   valid 5"));
    assert!(stdout.contains("bytes"));
    assert!(stdout.contains("top level  object:5"));
}

#[test]
fn stats_counts_blank_and_invalid_lines_separately() {
    let stdout = stats_on(b"{\"a\":1}\n\nbad\n", &[]);
    assert!(stdout.contains("blank 1   invalid 1   valid 1"));
    assert!(stdout.contains("invalid lines (1 total, showing 1)"));
    assert!(stdout.contains("line 3 col 1: unexpected character 'b'"));
}

#[test]
fn stats_max_errors_limits_the_reported_issues_but_not_the_total() {
    let stdout = stats_on(b"bad\nbad\nbad\n", &["--max-errors", "1"]);
    assert!(stdout.contains("invalid lines (3 total, showing 1)"));
    assert_eq!(stdout.matches("col 1:").count(), 1);
}

#[test]
fn stats_field_reports_presence_and_top_values() {
    let stdout = stats_on(
        b"{\"role\":\"user\"}\n{\"role\":\"assistant\"}\n{\"role\":\"user\"}\n",
        &["--field", "role"],
    );
    assert!(stdout.contains("field role"));
    assert!(stdout.contains("present in 3 of 3 records (100.0%), 3 values, types string:3"));
    assert!(stdout.contains("2 distinct values"));
    assert!(stdout.contains("66.7%  \"user\""));
    assert!(stdout.contains("33.3%  \"assistant\""));
    // "user" (2 occurrences) is listed before "assistant" (1).
    assert!(stdout.find("\"user\"").unwrap() < stdout.find("\"assistant\"").unwrap());
}

#[test]
fn stats_top_limits_the_number_of_values_shown_per_field() {
    let stdout = stats_on(
        b"{\"id\":1}\n{\"id\":2}\n{\"id\":3}\n",
        &["--field", "id", "--top", "2"],
    );
    // Each of the 3 distinct ids appears once (33.3%); only 2 value rows
    // should be printed. The presence-rate summary line also mentions a
    // percentage, so match on the per-value row shape (`%  "value"`) instead
    // of the percentage text alone.
    let value_rows = stdout.lines().filter(|line| line.contains("%  ")).count();
    assert_eq!(value_rows, 2);
}

#[test]
fn stats_rejects_an_invalid_field_path_as_a_usage_error() {
    let output = bin()
        .args(["stats", "--field", "messages[", "fixtures/sample.jsonl"])
        .output()
        .expect("run jsonl-peek stats");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn stats_reports_a_missing_file_as_a_runtime_error() {
    let output = bin()
        .args(["stats", "fixtures/does-not-exist.jsonl"])
        .output()
        .expect("run jsonl-peek stats");
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn stats_rejects_an_unknown_option() {
    let output = bin()
        .args(["stats", "--bogus"])
        .output()
        .expect("run jsonl-peek stats");
    assert_eq!(output.status.code(), Some(2));
}
