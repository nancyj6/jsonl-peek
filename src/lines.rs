use std::io::{self, BufRead};

const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// One line from the underlying reader, with its line terminator stripped
/// and (on the first line only) a leading UTF-8 BOM stripped too.
pub struct RawLine<'a> {
    pub number: usize,
    pub bytes: &'a [u8],
}

/// Splits a reader into lines without allocating a fresh buffer per line.
///
/// Handles LF and CRLF terminators, a missing terminator on the final line,
/// and a leading UTF-8 BOM. Blank lines come back as an empty slice rather
/// than being skipped, since whether to skip them is a decision for the
/// caller (`sample` does, `head` does not).
pub struct LineReader<R> {
    inner: R,
    buf: Vec<u8>,
    line_no: usize,
    checked_bom: bool,
}

impl<R: BufRead> LineReader<R> {
    pub fn new(inner: R) -> Self {
        LineReader {
            inner,
            buf: Vec::new(),
            line_no: 0,
            checked_bom: false,
        }
    }

    pub fn next_line(&mut self) -> io::Result<Option<RawLine<'_>>> {
        self.buf.clear();
        let n = self.inner.read_until(b'\n', &mut self.buf)?;
        if n == 0 {
            return Ok(None);
        }
        self.line_no += 1;

        let mut start = 0;
        if !self.checked_bom {
            self.checked_bom = true;
            if self.buf.starts_with(&UTF8_BOM) {
                start = UTF8_BOM.len();
            }
        }

        let mut end = self.buf.len();
        if end > start && self.buf[end - 1] == b'\n' {
            end -= 1;
            if end > start && self.buf[end - 1] == b'\r' {
                end -= 1;
            }
        }

        Ok(Some(RawLine {
            number: self.line_no,
            bytes: &self.buf[start..end],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn read_all(input: &[u8]) -> Vec<(usize, Vec<u8>)> {
        let mut reader = LineReader::new(Cursor::new(input));
        let mut out = Vec::new();
        while let Some(line) = reader.next_line().unwrap() {
            out.push((line.number, line.bytes.to_vec()));
        }
        out
    }

    #[test]
    fn splits_on_lf() {
        let lines = read_all(b"a\nb\nc\n");
        assert_eq!(
            lines,
            vec![(1, b"a".to_vec()), (2, b"b".to_vec()), (3, b"c".to_vec())]
        );
    }

    #[test]
    fn strips_crlf() {
        let lines = read_all(b"a\r\nb\r\n");
        assert_eq!(lines, vec![(1, b"a".to_vec()), (2, b"b".to_vec())]);
    }

    #[test]
    fn keeps_final_line_without_trailing_newline() {
        let lines = read_all(b"a\nb");
        assert_eq!(lines, vec![(1, b"a".to_vec()), (2, b"b".to_vec())]);
    }

    #[test]
    fn strips_leading_bom_once() {
        let mut input = UTF8_BOM.to_vec();
        input.extend_from_slice(b"a\nb\n");
        let lines = read_all(&input);
        assert_eq!(lines, vec![(1, b"a".to_vec()), (2, b"b".to_vec())]);
    }

    #[test]
    fn keeps_blank_lines() {
        let lines = read_all(b"a\n\nb\n");
        assert_eq!(
            lines,
            vec![(1, b"a".to_vec()), (2, b"".to_vec()), (3, b"b".to_vec())]
        );
    }

    #[test]
    fn empty_input_yields_no_lines() {
        assert_eq!(read_all(b""), Vec::new());
    }

    #[test]
    fn bom_only_input_is_a_single_blank_line() {
        assert_eq!(read_all(&UTF8_BOM), vec![(1, Vec::new())]);
    }
}
