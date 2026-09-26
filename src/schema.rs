//! The `schema` subcommand's engine: one pass over a JSONL file that walks
//! every record's structure up to a fixed depth and, for each distinct path
//! reached, tracks how many records contain it and what JSON type the value
//! at that path had each time it was seen.
//!
//! A path is written the same way `FieldPath` parses one: `.` between object
//! members, `[]` for "every element of this array". Schema discovery always
//! collapses arrays to a wildcard - it describes the shape of the file, not
//! any one record, so there is no such thing as "index 3" here.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead};

use crate::json::{self, Value};
use crate::lines::LineReader;
use crate::stats::TypeCounts;

/// Cap on distinct paths tracked, matching the README's promise that the
/// schema path table stops growing rather than holding one entry per path of
/// a file whose records do not share a shape.
const MAX_PATHS: usize = 2_000;

/// Knobs for a `Schema` run.
pub struct SchemaOptions {
    /// Maximum number of path segments to descend. A bare top-level key is
    /// depth 1; `messages[].role` is depth 3 (`messages`, `[]`, `role`).
    pub depth: usize,
    /// Paths present in fewer than this fraction of records (0.0-1.0) are
    /// left out of `Schema::paths`.
    pub min_rate: f64,
}

impl Default for SchemaOptions {
    fn default() -> Self {
        SchemaOptions { depth: 3, min_rate: 0.0 }
    }
}

/// Count and type breakdown of one path, as tracked by `Schema`.
#[derive(Default)]
pub struct PathEntry {
    /// Valid records containing at least one value at this path.
    pub records_present: usize,
    pub types: TypeCounts,
}

pub struct Schema {
    /// Valid (parsed) top-level records.
    pub records: usize,
    /// Lines that failed to parse as JSON. Blank lines are skipped and not
    /// counted here.
    pub unparseable: usize,
    /// Set once the path table has reached `MAX_PATHS` and a path not
    /// already in it is seen.
    pub truncated: bool,
    min_rate: f64,
    table: HashMap<String, PathEntry>,
}

/// One step in a discovered path: a named object member, or `[]`, standing
/// in for every element of an array.
enum Step {
    Key(String),
    Wildcard,
}

fn path_string(steps: &[Step]) -> String {
    let mut out = String::new();
    for step in steps {
        match step {
            Step::Key(key) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(key);
            }
            Step::Wildcard => out.push_str("[]"),
        }
    }
    out
}

impl Schema {
    /// Reads `reader` to the end, parsing every non-blank line as JSON and
    /// walking each valid record's structure up to `options.depth` segments
    /// deep.
    pub fn from_reader<R: BufRead>(reader: R, options: SchemaOptions) -> io::Result<Schema> {
        let mut lines = LineReader::new(reader);
        let mut schema = Schema {
            records: 0,
            unparseable: 0,
            truncated: false,
            min_rate: options.min_rate,
            table: HashMap::new(),
        };

        while let Some(line) = lines.next_line()? {
            if line.bytes.is_empty() {
                continue;
            }
            match json::parse(line.bytes) {
                Ok(value) => {
                    schema.records += 1;
                    let mut steps = Vec::new();
                    let mut seen = HashSet::new();
                    schema.walk(&value, &mut steps, options.depth, &mut seen);
                }
                Err(_) => schema.unparseable += 1,
            }
        }

        Ok(schema)
    }

    fn walk(&mut self, value: &Value, steps: &mut Vec<Step>, depth: usize, seen: &mut HashSet<String>) {
        if steps.len() >= depth {
            return;
        }
        match value {
            Value::Object(members) => {
                for (key, child) in members {
                    steps.push(Step::Key(key.clone()));
                    self.touch(child, steps, seen);
                    self.walk(child, steps, depth, seen);
                    steps.pop();
                }
            }
            Value::Array(items) => {
                if !items.is_empty() {
                    steps.push(Step::Wildcard);
                    for item in items {
                        self.touch(item, steps, seen);
                    }
                    for item in items {
                        self.walk(item, steps, depth, seen);
                    }
                    steps.pop();
                }
            }
            _ => {}
        }
    }

    /// Records one occurrence of `value` at the path spelled out by `steps`.
    /// `seen` is the set of paths already touched for the current record, so
    /// an array with ten elements bumps `records_present` once, not ten
    /// times.
    fn touch(&mut self, value: &Value, steps: &[Step], seen: &mut HashSet<String>) {
        let path = path_string(steps);
        if !self.table.contains_key(&path) {
            if self.table.len() >= MAX_PATHS {
                self.truncated = true;
                return;
            }
            self.table.insert(path.clone(), PathEntry::default());
        }
        let newly_seen = seen.insert(path.clone());
        let entry = self.table.get_mut(&path).unwrap();
        if newly_seen {
            entry.records_present += 1;
        }
        entry.types.record(value.type_name());
    }

    /// The fraction of `self.records` that contain `entry`'s path.
    pub fn rate(&self, entry: &PathEntry) -> f64 {
        if self.records == 0 {
            0.0
        } else {
            entry.records_present as f64 / self.records as f64
        }
    }

    /// Path/stats pairs in alphabetical order, excluding any path whose rate
    /// falls below the configured `--min-rate`.
    pub fn paths(&self) -> Vec<(&str, &PathEntry)> {
        let mut items: Vec<(&str, &PathEntry)> = self
            .table
            .iter()
            .filter(|(_, entry)| self.rate(entry) >= self.min_rate)
            .map(|(path, entry)| (path.as_str(), entry))
            .collect();
        items.sort_unstable_by(|a, b| a.0.cmp(b.0));
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn run(input: &[u8], options: SchemaOptions) -> Schema {
        Schema::from_reader(Cursor::new(input), options).unwrap()
    }

    fn find<'a>(schema: &'a Schema, path: &str) -> &'a PathEntry {
        schema
            .paths()
            .into_iter()
            .find(|(p, _)| *p == path)
            .unwrap_or_else(|| panic!("path {path:?} not found"))
            .1
    }

    #[test]
    fn top_level_keys_are_paths_at_depth_one() {
        let schema = run(b"{\"id\":1,\"name\":\"a\"}\n", SchemaOptions::default());
        assert_eq!(schema.records, 1);
        let paths: Vec<&str> = schema.paths().into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths, vec!["id", "name"]);
        assert_eq!(find(&schema, "id").types.most_common(), vec![("int", 1)]);
    }

    #[test]
    fn nested_objects_use_dotted_paths() {
        let schema = run(b"{\"meta\":{\"source\":\"web\"}}\n", SchemaOptions::default());
        let paths: Vec<&str> = schema.paths().into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths, vec!["meta", "meta.source"]);
    }

    #[test]
    fn arrays_collapse_to_a_wildcard() {
        let schema = run(
            b"{\"messages\":[{\"role\":\"user\"},{\"role\":\"assistant\"}]}\n",
            SchemaOptions::default(),
        );
        let paths: Vec<&str> = schema.paths().into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths, vec!["messages", "messages[]", "messages[].role"]);
        let role = find(&schema, "messages[].role");
        assert_eq!(role.records_present, 1);
        assert_eq!(role.types.most_common(), vec![("string", 2)]);
    }

    #[test]
    fn a_record_is_only_counted_once_per_path_no_matter_how_many_matches() {
        let schema = run(
            b"{\"tags\":[\"a\",\"b\",\"c\"]}\n{\"tags\":[\"d\"]}\n",
            SchemaOptions::default(),
        );
        let tags = find(&schema, "tags[]");
        assert_eq!(tags.records_present, 2);
        assert_eq!(tags.types.most_common(), vec![("string", 4)]);
    }

    #[test]
    fn depth_limits_how_far_the_walk_descends() {
        let input = b"{\"a\":{\"b\":{\"c\":1}}}\n";
        let schema = run(input, SchemaOptions { depth: 2, ..SchemaOptions::default() });
        let paths: Vec<&str> = schema.paths().into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths, vec!["a", "a.b"]);
    }

    #[test]
    fn depth_zero_reports_no_paths() {
        let schema = run(b"{\"a\":1}\n", SchemaOptions { depth: 0, ..SchemaOptions::default() });
        assert!(schema.paths().is_empty());
        assert_eq!(schema.records, 1);
    }

    #[test]
    fn min_rate_hides_sparse_paths() {
        let input = b"{\"id\":1,\"tags\":[\"a\"]}\n{\"id\":2}\n{\"id\":3}\n{\"id\":4}\n";
        let schema = run(input, SchemaOptions { min_rate: 0.5, ..SchemaOptions::default() });
        let paths: Vec<&str> = schema.paths().into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths, vec!["id"]);
    }

    #[test]
    fn unparseable_lines_are_counted_and_blank_lines_are_not() {
        let schema = run(b"{\"a\":1}\n\nbad\n", SchemaOptions::default());
        assert_eq!(schema.records, 1);
        assert_eq!(schema.unparseable, 1);
    }

    #[test]
    fn path_table_truncates_at_the_cap() {
        let mut input = String::new();
        for i in 0..(MAX_PATHS + 1) {
            input.push_str(&format!("{{\"k{i}\":1}}\n"));
        }
        let schema = run(input.as_bytes(), SchemaOptions::default());
        assert_eq!(schema.paths().len(), MAX_PATHS);
        assert!(schema.truncated);
    }

    #[test]
    fn a_top_level_array_record_uses_a_bare_wildcard_path() {
        let schema = run(b"[1,2,3]\n", SchemaOptions::default());
        let paths: Vec<&str> = schema.paths().into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths, vec!["[]"]);
        assert_eq!(find(&schema, "[]").types.most_common(), vec![("int", 3)]);
    }
}
