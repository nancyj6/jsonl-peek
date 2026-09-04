//! The `stats` subcommand's engine: one pass over a JSONL file that counts
//! lines, parses each one, collects the ones that fail, and tracks the
//! top-level type of each valid record plus the line-length distribution.
//! `--field` distributions build on top of this in a later pass over the
//! same struct.

use std::io::{self, BufRead};

use crate::hist::Histogram;
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

/// Counts of each JSON type seen at the top level of a record, in the order
/// each type was first encountered. Kept as a small ordered list rather than
/// a map: real files are almost always homogeneous, so this is one entry in
/// the common case and never more than the seven `Value` variants.
#[derive(Default)]
pub struct TypeCounts {
    counts: Vec<(&'static str, u64)>,
}

impl TypeCounts {
    fn record(&mut self, type_name: &'static str) {
        match self.counts.iter_mut().find(|(name, _)| *name == type_name) {
            Some((_, count)) => *count += 1,
            None => self.counts.push((type_name, 1)),
        }
    }

    /// Type/count pairs, most frequent first, for the `top level` report line.
    pub fn most_common(&self) -> Vec<(&'static str, u64)> {
        let mut sorted = self.counts.clone();
        sorted.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        sorted
    }
}

pub struct Stats {
    pub lines: usize,
    pub blank: usize,
    pub valid: usize,
    pub issues: Vec<Issue>,
    /// Set once `issues` has reached `max_errors` and further broken lines
    /// are being counted but not recorded.
    pub issues_truncated: bool,
    pub top_level_types: TypeCounts,
    /// Byte length of every non-blank line, valid or not. Blank lines are
    /// excluded since a length-0 entry would just drag `min` to 0 without
    /// telling you anything about record size.
    pub line_length: Histogram,
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
            top_level_types: TypeCounts::default(),
            line_length: Histogram::new(),
        };

        while let Some(line) = lines.next_line()? {
            stats.lines += 1;
            if line.bytes.is_empty() {
                stats.blank += 1;
                continue;
            }
            stats.line_length.record(line.bytes.len() as u64);
            match json::parse(line.bytes) {
                Ok(value) => {
                    stats.valid += 1;
                    stats.top_level_types.record(value.type_name());
                }
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

    #[test]
    fn top_level_types_are_counted_per_valid_record() {
        let stats = run(b"{\"a\":1}\n[1,2]\n{\"b\":2}\n\"x\"\n");
        assert_eq!(stats.top_level_types.most_common(), vec![("object", 2), ("array", 1), ("string", 1)]);
    }

    #[test]
    fn top_level_types_skip_invalid_and_blank_lines() {
        let stats = run(b"{\"a\":1}\nnot json\n\n");
        assert_eq!(stats.top_level_types.most_common(), vec![("object", 1)]);
    }

    #[test]
    fn line_length_covers_valid_and_invalid_but_not_blank_lines() {
        let stats = run(b"{\"a\":1}\nbad\n\n");
        assert_eq!(stats.line_length.count(), 2);
        assert_eq!(stats.line_length.min(), Some(3));
        assert_eq!(stats.line_length.max(), Some(7));
    }
}
