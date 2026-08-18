use std::io::Write;
use std::process::{Command, Stdio};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jsonl-peek"))
}

#[test]
fn head_defaults_to_ten_lines_or_whatever_the_file_has() {
    let output = bin()
        .args(["head", "fixtures/sample.jsonl"])
        .output()
        .expect("run jsonl-peek head");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 5);
}

#[test]
fn head_respects_n() {
    let output = bin()
        .args(["head", "-n", "2", "fixtures/sample.jsonl"])
        .output()
        .expect("run jsonl-peek head -n 2");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![r#"{"id":1,"text":"alpha"}"#, r#"{"id":2,"text":"beta"}"#]
    );
}

#[test]
fn head_reads_stdin_when_no_file_given() {
    let mut child = bin()
        .args(["head", "-n", "1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn jsonl-peek head");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"a\":1}\n{\"a\":2}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{\"a\":1}\n");
}

#[test]
fn head_reports_a_missing_file_as_a_runtime_error() {
    let output = bin()
        .args(["head", "fixtures/does-not-exist.jsonl"])
        .output()
        .expect("run jsonl-peek head");
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn unknown_command_is_a_usage_error() {
    let output = bin().args(["bogus"]).output().expect("run jsonl-peek bogus");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn no_command_is_a_usage_error() {
    let output = bin().output().expect("run jsonl-peek");
    assert_eq!(output.status.code(), Some(2));
}
