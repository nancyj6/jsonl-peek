//! The `stats` subcommand's engine: one pass over a JSONL file that counts
//! lines, parses each one, collects the ones that fail, tracks the top-level
//! type of each valid record and the line-length distribution, and profiles
//! each `--field` path given in `StatsOptions`.

use std::collections::HashMap;
use std::io::{self, BufRead};

use crate::hist::Histogram;
use crate::json::{self, Value};
use crate::lines::LineReader;
use crate::path::FieldPath;

/// Cap on distinct values tracked per `--field`, matching the README's
/// promise that the per-field value table stops growing rather than holding
/// one entry per distinct value of a field with effectively unbounded
/// cardinality (free text, a UUID).
const MAX_DISTINCT_VALUES: usize = 10_000;

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
    /// One entry per `--field` path, in the order given on the command line.
    pub fields: Vec<FieldStats>,
}

/// Distribution of one `--field` path's matches across all valid records.
pub struct FieldStats {
    pub path: FieldPath,
    /// Valid records where the path matched at least one value.
    pub records_present: usize,
    /// Total values matched. Larger than `records_present` when the path
    /// contains a `[]` wildcard, since one record can then contribute
    /// several values.
    pub value_count: u64,
    pub types: TypeCounts,
    values: HashMap<String, u64>,
    /// Set once the distinct-value table has reached `MAX_DISTINCT_VALUES`
    /// and further distinct values are being counted in `value_count` and
    /// `types` but not tracked individually.
    pub values_truncated: bool,
}

impl FieldStats {
    fn new(path: FieldPath) -> Self {
        FieldStats {
            path,
            records_present: 0,
            value_count: 0,
            types: TypeCounts::default(),
            values: HashMap::new(),
            values_truncated: false,
        }
    }

    fn record(&mut self, value: &Value) {
        self.value_count += 1;
        self.types.record(value.type_name());
        let key = render_value(value);
        if let Some(count) = self.values.get_mut(&key) {
            *count += 1;
        } else if self.values.len() < MAX_DISTINCT_VALUES {
            self.values.insert(key, 1);
        } else {
            self.values_truncated = true;
        }
    }

    /// Number of distinct values seen so far (bounded by `MAX_DISTINCT_VALUES`).
    pub fn distinct(&self) -> usize {
        self.values.len()
    }

    /// The `n` most common values, most frequent first, ties broken by the
    /// value's rendered text so the order is deterministic.
    pub fn top(&self, n: usize) -> Vec<(&str, u64)> {
        let mut sorted: Vec<(&str, u64)> =
            self.values.iter().map(|(value, &count)| (value.as_str(), count)).collect();
        sorted.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        sorted.truncate(n);
        sorted
    }
}

/// Renders a JSON value as the text used to group and print distinct values
/// in a `--field` report: quoted and escaped for strings, plain for other
/// scalars. Arrays and objects have no meaningful "value" beyond their
/// shape, so they collapse to a marker naming that shape.
fn render_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => render_string(s),
        Value::Array(_) => "<array>".to_string(),
        Value::Object(_) => "<object>".to_string(),
    }
}

fn render_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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
            fields: options.fields.into_iter().map(FieldStats::new).collect(),
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
                    for field in &mut stats.fields {
                        let matches = field.path.select(&value);
                        if !matches.is_empty() {
                            field.records_present += 1;
                        }
                        for matched in matches {
                            field.record(matched);
                        }
                    }
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

    fn run_with_fields(input: &[u8], paths: &[&str]) -> Stats {
        let fields = paths.iter().map(|p| FieldPath::parse(p).unwrap()).collect();
        let options = StatsOptions { fields, ..StatsOptions::default() };
        Stats::from_reader(Cursor::new(input), options).unwrap()
    }

    #[test]
    fn field_tracks_presence_and_distinct_values() {
        let stats = run_with_fields(
            b"{\"role\":\"user\"}\n{\"role\":\"assistant\"}\n{\"role\":\"user\"}\n{}\n",
            &["role"],
        );
        let field = &stats.fields[0];
        assert_eq!(field.records_present, 3);
        assert_eq!(field.value_count, 3);
        assert_eq!(field.distinct(), 2);
        assert_eq!(field.top(10), vec![("\"user\"", 2), ("\"assistant\"", 1)]);
        assert_eq!(field.types.most_common(), vec![("string", 3)]);
    }

    #[test]
    fn field_wildcard_counts_every_matched_value_per_record() {
        let stats = run_with_fields(
            b"{\"messages\":[{\"role\":\"user\"},{\"role\":\"assistant\"}]}\n{\"messages\":[{\"role\":\"user\"}]}\n",
            &["messages[].role"],
        );
        let field = &stats.fields[0];
        assert_eq!(field.records_present, 2);
        assert_eq!(field.value_count, 3);
        assert_eq!(field.top(10), vec![("\"user\"", 2), ("\"assistant\"", 1)]);
    }

    #[test]
    fn field_missing_from_a_record_does_not_count_as_present() {
        let stats = run_with_fields(b"{\"a\":1}\n{\"role\":\"user\"}\n", &["role"]);
        let field = &stats.fields[0];
        assert_eq!(field.records_present, 1);
        assert_eq!(field.value_count, 1);
    }

    #[test]
    fn field_skips_invalid_lines() {
        let stats = run_with_fields(b"not json\n{\"role\":\"user\"}\n", &["role"]);
        assert_eq!(stats.fields[0].records_present, 1);
    }

    #[test]
    fn field_values_truncate_at_the_distinct_value_cap() {
        let mut input = Vec::new();
        for i in 0..(MAX_DISTINCT_VALUES + 1) {
            input.extend_from_slice(format!("{{\"id\":{i}}}\n").as_bytes());
        }
        let stats = run_with_fields(&input, &["id"]);
        let field = &stats.fields[0];
        assert_eq!(field.distinct(), MAX_DISTINCT_VALUES);
        assert!(field.values_truncated);
        assert_eq!(field.value_count, (MAX_DISTINCT_VALUES + 1) as u64);
    }

    #[test]
    fn render_value_quotes_and_escapes_strings() {
        assert_eq!(render_value(&Value::String("hi \"there\"\n".to_string())), "\"hi \\\"there\\\"\\n\"");
        assert_eq!(render_value(&Value::Int(-3)), "-3");
        assert_eq!(render_value(&Value::Bool(true)), "true");
        assert_eq!(render_value(&Value::Null), "null");
        assert_eq!(render_value(&Value::Array(vec![])), "<array>");
        assert_eq!(render_value(&Value::Object(vec![])), "<object>");
    }
}
