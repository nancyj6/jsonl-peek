//! The `stats` subcommand's engine: one pass over a JSONL file that counts
//! lines, parses each one, and collects the ones that fail. Type counts,
//! the line-length histogram and `--field` distributions build on top of
//! this in later passes over the same struct.

use std::io::{self, BufRead};

use crate::json;
use crate::lines::LineReader;
use crate::path::FieldPath;

/// Knobs for a `Stats` run. `max_errors` bounds `Stats::issues`, per the
/// README's promise that the error list stops growing rather than holding
/// one entry per broken line in a file that is mostly broken.
pub struct StatsOptions {
    pub fields: Vec<FieldPath>,
    pub top: usize,
    pub max_errors: usize,
}

impl Default for StatsOptions {
    fn default() -> Self {
        StatsOptions { fields: Vec::new(), top: 10, max_errors: 10 }
    }
}

/// One line that failed to parse as JSON.
pub struct Issue {
    pub line: usize,
    pub column: usize,
    pub reason: String,
}

pub struct Stats {
    pub lines: usize,
    pub blank: usize,
    pub valid: usize,
    pub issues: Vec<Issue>,
    /// Set once `issues` has reached `max_errors` and further broken lines
    /// are being counted but not recorded.
    pub issues_truncated: bool,
}

impl Stats {
    /// Reads `reader` to the end, parsing every non-blank line as JSON.
    /// Blank lines are counted separately and are not parse errors.
    pub fn from_reader<R: BufRead>(reader: R, options: StatsOptions) -> io::Result<Stats> {
        let mut lines = LineReader::new(reader);
        let mut stats = Stats {
            lines: 0,
            blank: 0,
            valid: 0,
            issues: Vec::new(),
            issues_truncated: false,
        };

        while let Some(line) = lines.next_line()? {
            stats.lines += 1;
            if line.bytes.is_empty() {
                stats.blank += 1;
                continue;
            }
            match json::parse(line.bytes) {
                Ok(_) => stats.valid += 1,
                Err(err) => {
                    if stats.issues.len() < options.max_errors {
                        stats.issues.push(Issue {
                            line: line.number,
                            column: err.column,
                            reason: err.message,
                        });
                    } else {
                        stats.issues_truncated = true;
                    }
                }
            }
        }

        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn run(input: &[u8]) -> Stats {
        Stats::from_reader(Cursor::new(input), StatsOptions::default()).unwrap()
    }

    #[test]
    fn counts_lines_blank_and_valid() {
        let stats = run(b"{\"a\":1}\n\n{\"b\":2}\n");
        assert_eq!(stats.lines, 3);
        assert_eq!(stats.blank, 1);
        assert_eq!(stats.valid, 2);
        assert!(stats.issues.is_empty());
    }

    #[test]
    fn blank_lines_are_not_parse_errors() {
        let stats = run(b"\n\n\n");
        assert_eq!(stats.lines, 3);
        assert_eq!(stats.blank, 3);
        assert_eq!(stats.valid, 0);
        assert!(stats.issues.is_empty());
    }

    #[test]
    fn broken_lines_are_reported_with_line_and_column() {
        let stats = run(b"{\"a\":1}\n{\"a\":,}\n");
        assert_eq!(stats.valid, 1);
        assert_eq!(stats.issues.len(), 1);
        assert_eq!(stats.issues[0].line, 2);
        assert_eq!(stats.issues[0].column, 6);
    }

    #[test]
    fn issue_list_stops_growing_at_max_errors() {
        let input = b"bad\nbad\nbad\nbad\nbad\n";
        let options = StatsOptions { max_errors: 2, ..StatsOptions::default() };
        let stats = Stats::from_reader(Cursor::new(input), options).unwrap();
        assert_eq!(stats.lines, 5);
        assert_eq!(stats.issues.len(), 2);
        assert!(stats.issues_truncated);
    }

    #[test]
    fn issues_truncated_stays_false_when_under_the_cap() {
        let stats = run(b"bad\n");
        assert_eq!(stats.issues.len(), 1);
        assert!(!stats.issues_truncated);
    }

    #[test]
    fn crlf_and_missing_final_newline_are_handled() {
        let stats = run(b"{\"a\":1}\r\n{\"b\":2}");
        assert_eq!(stats.lines, 2);
        assert_eq!(stats.valid, 2);
    }
}
