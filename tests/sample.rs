use std::io::Write;
use std::process::{Command, Stdio};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_jsonl-peek"))
}

#[test]
fn sample_of_the_whole_file_returns_every_line_in_order() {
    let output = bin()
        .args(["sample", "-n", "5", "--seed", "1", "fixtures/sample.jsonl"])
        .output()
        .expect("run jsonl-peek sample");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec![
            r#"{"id":1,"text":"alpha"}"#,
            r#"{"id":2,"text":"beta"}"#,
            r#"{"id":3,"text":"gamma"}"#,
            r#"{"id":4,"text":"delta"}"#,
            r#"{"id":5,"text":"epsilon"}"#,
        ]
    );
}

#[test]
fn same_seed_reproduces_the_same_sample() {
    let run = || {
        bin()
            .args(["sample", "-n", "2", "--seed", "42", "fixtures/sample.jsonl"])
            .output()
            .expect("run jsonl-peek sample")
            .stdout
    };
    assert_eq!(run(), run());
}

#[test]
fn sample_never_exceeds_the_requested_count() {
    let output = bin()
        .args(["sample", "-n", "2", "--seed", "7", "fixtures/sample.jsonl"])
        .output()
        .expect("run jsonl-peek sample");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 2);
}

#[test]
fn sample_keeps_original_file_order() {
    let output = bin()
        .args(["sample", "-n", "3", "--seed", "99", "fixtures/sample.jsonl"])
        .output()
        .expect("run jsonl-peek sample");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let ids: Vec<u32> = stdout
        .lines()
        .map(|line| {
            let start = line.find("\"id\":").unwrap() + 5;
            let rest = &line[start..];
            let end = rest.find(',').unwrap();
            rest[..end].parse().unwrap()
        })
        .collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted);
}

#[test]
fn sample_skips_blank_lines() {
    let mut child = bin()
        .args(["sample", "-n", "10", "--seed", "3"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn jsonl-peek sample");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"a\":1}\n\n{\"a\":2}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{\"a\":1}\n{\"a\":2}\n");
}

#[test]
fn sample_reads_stdin_when_no_file_given() {
    let mut child = bin()
        .args(["sample", "-n", "1", "--seed", "5"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn jsonl-peek sample");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"a\":1}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{\"a\":1}\n");
}
