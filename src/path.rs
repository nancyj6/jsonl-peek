use std::fmt;

use crate::json::Value;

/// A single step in a field path: a named object member, a specific array
/// index (negative counts from the end), or `[]`, which fans out to every
/// element of an array.
#[derive(Debug, Clone, PartialEq)]
enum Segment {
    Key(String),
    Index(i64),
    Wildcard,
}

/// A parsed field selector, e.g. `meta.source` or `messages[].role`. See the
/// README's "Field path syntax" table for the supported forms.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldPath {
    raw: String,
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathError {
    pub message: String,
}

impl PathError {
    fn new(message: impl Into<String>) -> Self {
        PathError { message: message.into() }
    }
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw)
    }
}

impl FieldPath {
    /// Parses a field path. `.` separates object members; `[N]` selects an
    /// array element (negative `N` counts from the end); `[]` selects every
    /// element. A path may start with `[` directly, for records that are
    /// themselves arrays. `.` and `[` are the only reserved characters -
    /// anything else, including `]` on its own, is ordinary key text.
    pub fn parse(input: &str) -> Result<FieldPath, PathError> {
        if input.is_empty() {
            return Err(PathError::new("empty field path"));
        }
        let mut segments = Vec::new();
        for token in input.split('.') {
            if token.is_empty() {
                return Err(PathError::new(format!("'{input}' has an empty segment")));
            }
            segments.extend(parse_token(input, token)?);
        }
        Ok(FieldPath { raw: input.to_string(), segments })
    }

    /// Every value in `root` that matches this path. A missing key, an
    /// out-of-range index, or a segment applied to a value of the wrong
    /// shape (an index into an object, say) matches nothing rather than
    /// erroring - most records in a real dataset will be missing at least
    /// one optional field.
    pub fn select<'v>(&self, root: &'v Value) -> Vec<&'v Value> {
        let mut current = vec![root];
        for segment in &self.segments {
            let mut next = Vec::new();
            for value in current {
                match segment {
                    Segment::Key(key) => {
                        if let Some(found) = value.get(key) {
                            next.push(found);
                        }
                    }
                    Segment::Index(index) => {
                        if let Some(items) = value.as_array() {
                            if let Some(found) = resolve_index(items, *index) {
                                next.push(found);
                            }
                        }
                    }
                    Segment::Wildcard => {
                        if let Some(items) = value.as_array() {
                            next.extend(items.iter());
                        }
                    }
                }
            }
            current = next;
        }
        current
    }
}

fn resolve_index(items: &[Value], index: i64) -> Option<&Value> {
    let len = items.len() as i64;
    let actual = if index < 0 { len + index } else { index };
    if actual < 0 || actual >= len {
        None
    } else {
        items.get(actual as usize)
    }
}

/// Parses one dot-separated token, which is an optional key followed by zero
/// or more `[...]` groups (`messages[0]`, `messages[]`, or a bare `[0]`).
fn parse_token(whole: &str, token: &str) -> Result<Vec<Segment>, PathError> {
    let bytes = token.as_bytes();
    let mut segments = Vec::new();
    let mut pos = 0;

    if pos < bytes.len() && bytes[pos] != b'[' {
        let start = pos;
        while pos < bytes.len() && bytes[pos] != b'[' {
            pos += 1;
        }
        segments.push(Segment::Key(token[start..pos].to_string()));
    }

    while pos < bytes.len() {
        let close = token[pos..]
            .find(']')
            .map(|offset| pos + offset)
            .ok_or_else(|| PathError::new(format!("unterminated '[' in '{whole}'")))?;
        let content = &token[pos + 1..close];
        let segment = if content.is_empty() {
            Segment::Wildcard
        } else {
            let index: i64 = content
                .parse()
                .map_err(|_| PathError::new(format!("invalid index '[{content}]' in '{whole}'")))?;
            Segment::Index(index)
        };
        segments.push(segment);
        pos = close + 1;
    }

    if segments.is_empty() {
        return Err(PathError::new(format!("'{whole}' has an empty segment")));
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(input: &str) -> FieldPath {
        FieldPath::parse(input).unwrap_or_else(|err| panic!("expected {input:?} to parse, got {err}"))
    }

    fn parse_err(input: &str) -> PathError {
        FieldPath::parse(input).err().unwrap_or_else(|| panic!("expected {input:?} to be rejected"))
    }

    #[test]
    fn parses_a_bare_key() {
        assert_eq!(parse_ok("role").segments, vec![Segment::Key("role".to_string())]);
    }

    #[test]
    fn parses_a_dotted_path() {
        assert_eq!(
            parse_ok("meta.source").segments,
            vec![Segment::Key("meta".to_string()), Segment::Key("source".to_string())]
        );
    }

    #[test]
    fn parses_an_array_index() {
        assert_eq!(
            parse_ok("messages[0].content").segments,
            vec![
                Segment::Key("messages".to_string()),
                Segment::Index(0),
                Segment::Key("content".to_string()),
            ]
        );
    }

    #[test]
    fn parses_a_negative_index() {
        assert_eq!(
            parse_ok("messages[-1].content").segments,
            vec![
                Segment::Key("messages".to_string()),
                Segment::Index(-1),
                Segment::Key("content".to_string()),
            ]
        );
    }

    #[test]
    fn parses_a_wildcard() {
        assert_eq!(
            parse_ok("messages[].role").segments,
            vec![
                Segment::Key("messages".to_string()),
                Segment::Wildcard,
                Segment::Key("role".to_string()),
            ]
        );
    }

    #[test]
    fn parses_a_leading_index() {
        assert_eq!(
            parse_ok("[0].id").segments,
            vec![Segment::Index(0), Segment::Key("id".to_string())]
        );
    }

    #[test]
    fn display_returns_the_original_text() {
        assert_eq!(parse_ok("messages[].role").to_string(), "messages[].role");
    }

    #[test]
    fn rejects_empty_and_malformed_paths() {
        parse_err("");
        parse_err(".");
        parse_err(".role");
        parse_err("role.");
        parse_err("a..b");
        parse_err("messages[");
        parse_err("messages[abc]");
        parse_err("messages[0");
    }

    fn record() -> Value {
        crate::json::parse(
            br#"{"id":1,"messages":[{"role":"user"},{"role":"assistant"}],"tags":["a","b"]}"#,
        )
        .unwrap()
    }

    #[test]
    fn selects_a_top_level_key() {
        let record = record();
        assert_eq!(parse_ok("id").select(&record), vec![&Value::Int(1)]);
    }

    #[test]
    fn selects_every_array_element_with_a_wildcard() {
        let record = record();
        let roles = parse_ok("messages[].role").select(&record);
        assert_eq!(roles, vec![&Value::String("user".to_string()), &Value::String("assistant".to_string())]);
    }

    #[test]
    fn selects_the_last_element_with_a_negative_index() {
        let record = record();
        assert_eq!(
            parse_ok("messages[-1].role").select(&record),
            vec![&Value::String("assistant".to_string())]
        );
    }

    #[test]
    fn missing_key_selects_nothing() {
        let record = record();
        assert_eq!(parse_ok("meta.source").select(&record), Vec::<&Value>::new());
    }

    #[test]
    fn out_of_range_index_selects_nothing() {
        let record = record();
        assert_eq!(parse_ok("tags[5]").select(&record), Vec::<&Value>::new());
    }

    #[test]
    fn index_into_non_array_selects_nothing() {
        let record = record();
        assert_eq!(parse_ok("id[0]").select(&record), Vec::<&Value>::new());
    }
}
