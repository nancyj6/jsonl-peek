use std::fmt;

/// A parsed JSON value. Objects keep members in the order they appeared in
/// the source; duplicate keys are kept rather than merged, since deciding
/// which one wins is a policy question for the caller, not the parser.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// Short type name, as used in `stats`/`schema` reports (`int:2000`).
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Object(members) => Some(members),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// Looks up a member by key on an object. Returns `None` for any other
    /// value, or if the key is absent.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object()?.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

/// A parse failure with the 1-based byte column into the input where it was
/// detected. Column, not line: the parser is handed one JSONL record at a
/// time, and the record's line number in the file is the caller's business.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub column: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "col {}: {}", self.column, self.message)
    }
}

/// Parses a single JSON value from `input`, requiring the whole input
/// (aside from surrounding whitespace) to be consumed. Strict: no trailing
/// commas, no single-quoted strings, no unquoted keys, no `NaN`/`Infinity`,
/// no leading zeros, no raw control characters in strings, and no lone
/// UTF-16 surrogates in `\u` escapes.
pub fn parse(input: &[u8]) -> Result<Value, ParseError> {
    let text = std::str::from_utf8(input).map_err(|err| ParseError {
        column: err.valid_up_to() + 1,
        message: "invalid UTF-8".to_string(),
    })?;
    let mut parser = Parser { text, pos: 0 };
    parser.skip_whitespace();
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.pos != text.len() {
        return Err(parser.error("trailing characters after value"));
    }
    Ok(value)
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn error(&self, message: &str) -> ParseError {
        ParseError { column: self.pos + 1, message: message.to_string() }
    }

    fn byte(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.byte(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn parse_value(&mut self) -> Result<Value, ParseError> {
        match self.byte() {
            None => Err(self.error("unexpected end of input")),
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => self.parse_string().map(Value::String),
            Some(b't') => self.expect_literal("true", Value::Bool(true)),
            Some(b'f') => self.expect_literal("false", Value::Bool(false)),
            Some(b'n') => self.expect_literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(_) => {
                let ch = self.text[self.pos..].chars().next().unwrap();
                Err(self.error(&format!("unexpected character '{ch}'")))
            }
        }
    }

    fn expect_literal(&mut self, literal: &str, value: Value) -> Result<Value, ParseError> {
        if self.text[self.pos..].starts_with(literal) {
            self.pos += literal.len();
            Ok(value)
        } else {
            Err(self.error(&format!("expected '{literal}'")))
        }
    }

    fn parse_object(&mut self) -> Result<Value, ParseError> {
        self.pos += 1; // '{'
        self.skip_whitespace();
        let mut members = Vec::new();
        if self.byte() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(members));
        }
        loop {
            self.skip_whitespace();
            if self.byte() != Some(b'"') {
                return Err(self.error("expected string key"));
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.byte() != Some(b':') {
                return Err(self.error("expected ':' after object key"));
            }
            self.pos += 1;
            self.skip_whitespace();
            let value = self.parse_value()?;
            members.push((key, value));
            self.skip_whitespace();
            match self.byte() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(members));
                }
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Value, ParseError> {
        self.pos += 1; // '['
        self.skip_whitespace();
        let mut items = Vec::new();
        if self.byte() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.parse_value()?);
            self.skip_whitespace();
            match self.byte() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        self.pos += 1; // opening '"'
        let mut out = String::new();
        loop {
            let byte = self.byte().ok_or_else(|| self.error("unterminated string"))?;
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    self.parse_escape(&mut out)?;
                }
                0x00..=0x1F => return Err(self.error("control character in string")),
                _ => {
                    let ch = self.text[self.pos..].chars().next().unwrap();
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    fn parse_escape(&mut self, out: &mut String) -> Result<(), ParseError> {
        let esc = self.byte().ok_or_else(|| self.error("unterminated escape"))?;
        match esc {
            b'"' => {
                out.push('"');
                self.pos += 1;
            }
            b'\\' => {
                out.push('\\');
                self.pos += 1;
            }
            b'/' => {
                out.push('/');
                self.pos += 1;
            }
            b'b' => {
                out.push('\u{8}');
                self.pos += 1;
            }
            b'f' => {
                out.push('\u{c}');
                self.pos += 1;
            }
            b'n' => {
                out.push('\n');
                self.pos += 1;
            }
            b'r' => {
                out.push('\r');
                self.pos += 1;
            }
            b't' => {
                out.push('\t');
                self.pos += 1;
            }
            b'u' => {
                self.pos += 1;
                let unit = self.parse_hex4()?;
                let ch = if (0xD800..=0xDBFF).contains(&unit) {
                    if self.byte() == Some(b'\\')
                        && self.text.as_bytes().get(self.pos + 1) == Some(&b'u')
                    {
                        self.pos += 2;
                        let low = self.parse_hex4()?;
                        if !(0xDC00..=0xDFFF).contains(&low) {
                            return Err(self.error("unpaired UTF-16 surrogate"));
                        }
                        let cp = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                        char::from_u32(cp).ok_or_else(|| self.error("invalid surrogate pair"))?
                    } else {
                        return Err(self.error("unpaired UTF-16 surrogate"));
                    }
                } else if (0xDC00..=0xDFFF).contains(&unit) {
                    return Err(self.error("unpaired UTF-16 surrogate"));
                } else {
                    char::from_u32(unit).ok_or_else(|| self.error("invalid unicode escape"))?
                };
                out.push(ch);
            }
            _ => return Err(self.error("invalid escape sequence")),
        }
        Ok(())
    }

    fn parse_hex4(&mut self) -> Result<u32, ParseError> {
        let mut value = 0u32;
        for _ in 0..4 {
            let byte = self.byte().ok_or_else(|| self.error("incomplete unicode escape"))?;
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(self.error("invalid unicode escape")),
            };
            value = value * 16 + digit as u32;
            self.pos += 1;
        }
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<Value, ParseError> {
        let start = self.pos;
        if self.byte() == Some(b'-') {
            self.pos += 1;
        }
        match self.byte() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while matches!(self.byte(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.error("invalid number")),
        }

        let mut is_float = false;
        if self.byte() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            let frac_start = self.pos;
            while matches!(self.byte(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
            if frac_start == self.pos {
                return Err(self.error("expected digit after decimal point"));
            }
        }
        if matches!(self.byte(), Some(b'e' | b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.byte(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            let exp_start = self.pos;
            while matches!(self.byte(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
            if exp_start == self.pos {
                return Err(self.error("expected digit in exponent"));
            }
        }

        let text = &self.text[start..self.pos];
        if !is_float {
            if let Ok(n) = text.parse::<i64>() {
                return Ok(Value::Int(n));
            }
        }
        text.parse::<f64>().map(Value::Float).map_err(|_| self.error("invalid number"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(input: &str) -> Value {
        parse(input.as_bytes()).unwrap_or_else(|err| panic!("expected {input:?} to parse, got {err}"))
    }

    fn err(input: &str) -> ParseError {
        parse(input.as_bytes()).err().unwrap_or_else(|| panic!("expected {input:?} to be rejected"))
    }

    #[test]
    fn accepts_scalars() {
        assert_eq!(ok("null"), Value::Null);
        assert_eq!(ok("true"), Value::Bool(true));
        assert_eq!(ok("false"), Value::Bool(false));
        assert_eq!(ok("0"), Value::Int(0));
        assert_eq!(ok("-17"), Value::Int(-17));
        assert_eq!(ok("3.5"), Value::Float(3.5));
        assert_eq!(ok("1e3"), Value::Float(1000.0));
        assert_eq!(ok("-2.5E-2"), Value::Float(-0.025));
    }

    #[test]
    fn large_integer_falls_back_to_float() {
        assert_eq!(ok("99999999999999999999"), Value::Float(99999999999999999999.0));
    }

    #[test]
    fn accepts_strings_with_escapes() {
        assert_eq!(ok(r#""a\nb\t\"c""#), Value::String("a\nb\t\"c".to_string()));
        assert_eq!(ok(r#""café""#), Value::String("caf\u{e9}".to_string()));
        assert_eq!(ok(r#""😀""#), Value::String("\u{1f600}".to_string()));
        assert_eq!(ok("\"h\u{e9}llo\""), Value::String("h\u{e9}llo".to_string()));
    }

    #[test]
    fn accepts_nested_array_and_object() {
        let value = ok(r#"{"a":[1,2,{"b":null}],"c":true}"#);
        let members = value.as_object().unwrap();
        assert_eq!(members[0].0, "a");
        let inner = members[0].1.as_array().unwrap();
        assert_eq!(inner[0], Value::Int(1));
        assert_eq!(inner[2].get("b"), Some(&Value::Null));
        assert_eq!(value.get("c"), Some(&Value::Bool(true)));
    }

    #[test]
    fn allows_whitespace_around_tokens() {
        assert_eq!(ok(" \t{ \"a\" : 1 ,\n\"b\" : 2 }\r\n"), ok(r#"{"a":1,"b":2}"#));
    }

    #[test]
    fn rejects_trailing_comma() {
        assert!(parse(br#"[1,2,]"#).is_err());
        assert!(parse(br#"{"a":1,}"#).is_err());
    }

    #[test]
    fn rejects_single_quotes_and_unquoted_keys() {
        assert!(parse(b"{'a':1}").is_err());
        assert!(parse(b"{a:1}").is_err());
    }

    #[test]
    fn rejects_nan_and_infinity() {
        assert!(parse(b"NaN").is_err());
        assert!(parse(b"Infinity").is_err());
        assert!(parse(b"-Infinity").is_err());
    }

    #[test]
    fn rejects_leading_zero() {
        assert!(parse(b"01").is_err());
        assert!(parse(b"-01").is_err());
    }

    #[test]
    fn rejects_raw_control_character_in_string() {
        let mut input = vec![b'"'];
        input.push(0x07);
        input.push(b'"');
        assert!(parse(&input).is_err());
    }

    #[test]
    fn rejects_lone_surrogate() {
        assert!(parse(br#""\ud83d""#).is_err());
        assert!(parse(br#""\ud83dX""#).is_err());
    }

    #[test]
    fn rejects_trailing_characters() {
        assert!(parse(b"1 2").is_err());
        assert!(parse(b"{}garbage").is_err());
    }

    #[test]
    fn rejects_empty_and_unterminated_input() {
        assert!(parse(b"").is_err());
        assert!(parse(b"\"unterminated").is_err());
        assert!(parse(b"{\"a\":1").is_err());
    }

    #[test]
    fn error_column_points_at_the_offending_byte() {
        let e = err(r#"{"a": ,"b": 2}"#);
        assert_eq!(e.column, 7);
    }

    #[test]
    fn invalid_utf8_is_reported_at_the_byte_offset() {
        let mut input = br#"{"a":""#.to_vec();
        input.push(0xFF);
        input.extend_from_slice(br#""}"#);
        let e = parse(&input).unwrap_err();
        assert_eq!(e.column, 7);
        assert_eq!(e.message, "invalid UTF-8");
    }
}
