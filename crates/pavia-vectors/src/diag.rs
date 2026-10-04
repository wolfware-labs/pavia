//! CBOR diagnostic notation (RFC 8949 section 8), the basic subset the vectors use, plus the
//! pattern extensions of `vectors/README.md`: `*` (any value), `...` (map may have more keys)
//! and `$name` (a captured value).

use std::fmt;

use ciborium::Value;

/// A parsed diagnostic-notation item. Patterns may contain [`Diag::Any`], [`Diag::Ref`] and open
/// maps; plain values never do.
#[derive(Debug, Clone, PartialEq)]
pub enum Diag {
    /// Unsigned integer, major type 0.
    Uint(u64),
    /// Negative integer `-1 - n`, major type 1, stored as `n`.
    Nint(u64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
    Array(Vec<Diag>),
    /// Entries in written order; `open` is true when the map ends with `...`.
    Map {
        entries: Vec<(Diag, Diag)>,
        open: bool,
    },
    Tag(u64, Box<Diag>),
    Bool(bool),
    Null,
    /// `*`: matches any single value.
    Any,
    /// `$name`: a value captured earlier in a script.
    Ref(String),
}

/// Where and why parsing failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parses one diagnostic-notation item; trailing input other than whitespace is an error.
pub fn parse(input: &str) -> Result<Diag, ParseError> {
    let mut p = Parser {
        src: input.as_bytes(),
        pos: 0,
    };
    let item = p.item()?;
    p.skip_ws();
    if p.pos != p.src.len() {
        return Err(p.err("unexpected trailing input"));
    }
    Ok(item)
}

impl Diag {
    /// True if the item contains `*`, `$name` or an open map anywhere.
    pub fn is_pattern(&self) -> bool {
        match self {
            Diag::Any | Diag::Ref(_) => true,
            Diag::Map { entries, open } => {
                *open
                    || entries
                        .iter()
                        .any(|(k, v)| k.is_pattern() || v.is_pattern())
            }
            Diag::Array(items) => items.iter().any(Diag::is_pattern),
            Diag::Tag(_, inner) => inner.is_pattern(),
            _ => false,
        }
    }

    /// Every `$name` referenced in the item, in order of appearance.
    pub fn refs(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_refs(&mut out);
        out
    }

    fn collect_refs<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Diag::Ref(name) => out.push(name),
            Diag::Array(items) => items.iter().for_each(|i| i.collect_refs(out)),
            Diag::Map { entries, .. } => entries.iter().for_each(|(k, v)| {
                k.collect_refs(out);
                v.collect_refs(out);
            }),
            Diag::Tag(_, inner) => inner.collect_refs(out),
            _ => {}
        }
    }

    /// Converts a plain value to a `ciborium` value, keeping map entries in written order.
    /// Fails on pattern elements, which have no CBOR encoding.
    pub fn to_cbor(&self) -> Result<Value, String> {
        Ok(match self {
            Diag::Uint(n) => Value::Integer((*n).into()),
            Diag::Nint(n) => {
                let v = -1_i128 - i128::from(*n);
                Value::Integer(v.try_into().map_err(|_| format!("{v} out of CBOR range"))?)
            }
            Diag::Float(f) => Value::Float(*f),
            Diag::Text(s) => Value::Text(s.clone()),
            Diag::Bytes(b) => Value::Bytes(b.clone()),
            Diag::Array(items) => {
                Value::Array(items.iter().map(Diag::to_cbor).collect::<Result<_, _>>()?)
            }
            Diag::Map {
                entries,
                open: false,
            } => Value::Map(
                entries
                    .iter()
                    .map(|(k, v)| Ok((k.to_cbor()?, v.to_cbor()?)))
                    .collect::<Result<_, String>>()?,
            ),
            Diag::Tag(tag, inner) => Value::Tag(*tag, Box::new(inner.to_cbor()?)),
            Diag::Bool(b) => Value::Bool(*b),
            Diag::Null => Value::Null,
            Diag::Map { open: true, .. } => return Err("open map `...` is a pattern".into()),
            Diag::Any => return Err("`*` is a pattern".into()),
            Diag::Ref(name) => return Err(format!("`${name}` is a pattern")),
        })
    }
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn err(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            offset: self.pos,
            message: message.into(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.skip_ws();
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), ParseError> {
        if self.eat(byte) {
            Ok(())
        } else {
            Err(self.err(format!("expected `{}`", byte as char)))
        }
    }

    fn starts_with(&self, word: &str) -> bool {
        self.src[self.pos..].starts_with(word.as_bytes())
    }

    fn item(&mut self) -> Result<Diag, ParseError> {
        self.skip_ws();
        match self.peek() {
            None => Err(self.err("unexpected end of input")),
            Some(b'[') => self.array(),
            Some(b'{') => self.map(),
            Some(b'"') => Ok(Diag::Text(self.text()?)),
            Some(b'h') if self.starts_with("h'") => self.bytes(),
            Some(b'*') => {
                self.pos += 1;
                Ok(Diag::Any)
            }
            Some(b'$') => self.reference(),
            Some(b'-' | b'0'..=b'9') => self.number_or_tag(),
            Some(_) => self.word(),
        }
    }

    fn array(&mut self) -> Result<Diag, ParseError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        if self.eat(b']') {
            return Ok(Diag::Array(items));
        }
        loop {
            items.push(self.item()?);
            if self.eat(b']') {
                return Ok(Diag::Array(items));
            }
            self.expect(b',')?;
        }
    }

    fn map(&mut self) -> Result<Diag, ParseError> {
        self.expect(b'{')?;
        let mut entries = Vec::new();
        if self.eat(b'}') {
            return Ok(Diag::Map {
                entries,
                open: false,
            });
        }
        loop {
            self.skip_ws();
            if self.starts_with("...") {
                self.pos += 3;
                self.expect(b'}')?;
                return Ok(Diag::Map {
                    entries,
                    open: true,
                });
            }
            let key = self.item()?;
            self.expect(b':')?;
            let value = self.item()?;
            entries.push((key, value));
            if self.eat(b'}') {
                return Ok(Diag::Map {
                    entries,
                    open: false,
                });
            }
            self.expect(b',')?;
        }
    }

    fn text(&mut self) -> Result<String, ParseError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let rest = std::str::from_utf8(&self.src[self.pos..])
                .map_err(|_| self.err("invalid UTF-8"))?;
            let c = rest
                .chars()
                .next()
                .ok_or_else(|| self.err("unterminated string"))?;
            self.pos += c.len_utf8();
            match c {
                '"' => return Ok(out),
                '\\' => out.push(self.escape()?),
                _ => out.push(c),
            }
        }
    }

    fn escape(&mut self) -> Result<char, ParseError> {
        let c = self.peek().ok_or_else(|| self.err("unterminated escape"))?;
        self.pos += 1;
        Ok(match c {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let high = self.hex4()?;
                if (0xD800..0xDC00).contains(&high) {
                    if !self.starts_with("\\u") {
                        return Err(self.err("unpaired surrogate"));
                    }
                    self.pos += 2;
                    let low = self.hex4()?;
                    let code = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
                    char::from_u32(code).ok_or_else(|| self.err("invalid surrogate pair"))?
                } else {
                    char::from_u32(high).ok_or_else(|| self.err("invalid code point"))?
                }
            }
            other => return Err(self.err(format!("unknown escape `\\{}`", other as char))),
        })
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let digits = self
            .src
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| self.err("short \\u"))?;
        let s = std::str::from_utf8(digits).map_err(|_| self.err("invalid \\u"))?;
        let v = u32::from_str_radix(s, 16).map_err(|_| self.err("invalid \\u"))?;
        self.pos += 4;
        Ok(v)
    }

    fn bytes(&mut self) -> Result<Diag, ParseError> {
        self.pos += 2; // h'
        let start = self.pos;
        let end = self.src[start..]
            .iter()
            .position(|&b| b == b'\'')
            .map(|i| start + i)
            .ok_or_else(|| self.err("unterminated byte string"))?;
        let digits: Vec<u8> = self.src[start..end]
            .iter()
            .copied()
            .filter(|b| !b.is_ascii_whitespace())
            .collect();
        self.pos = end + 1;
        hex_decode(&digits)
            .map(Diag::Bytes)
            .map_err(|m| self.err(m))
    }

    fn reference(&mut self) -> Result<Diag, ParseError> {
        self.pos += 1; // $
        let start = self.pos;
        while matches!(self.peek(), Some(b'a'..=b'z' | b'0'..=b'9' | b'_')) {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.err("empty `$` reference"));
        }
        Ok(Diag::Ref(
            String::from_utf8_lossy(&self.src[start..self.pos]).into_owned(),
        ))
    }

    fn number_or_tag(&mut self) -> Result<Diag, ParseError> {
        let start = self.pos;
        if self.starts_with("-Infinity") {
            self.pos += 9;
            return Ok(Diag::Float(f64::NEG_INFINITY));
        }
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while matches!(
            self.peek(),
            Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
        ) {
            self.pos += 1;
        }
        let token = std::str::from_utf8(&self.src[start..self.pos]).expect("ASCII");
        let is_float = token.contains(['.', 'e', 'E']);
        if !is_float && self.peek() == Some(b'(') {
            let tag: u64 = token.parse().map_err(|_| self.err("invalid tag number"))?;
            self.pos += 1;
            let inner = self.item()?;
            self.expect(b')')?;
            return Ok(Diag::Tag(tag, Box::new(inner)));
        }
        if is_float {
            return token
                .parse()
                .map(Diag::Float)
                .map_err(|_| self.err("invalid float"));
        }
        if let Some(magnitude) = token.strip_prefix('-') {
            let m: u128 = magnitude.parse().map_err(|_| self.err("invalid integer"))?;
            if m == 0 {
                return Ok(Diag::Uint(0));
            }
            let n = u64::try_from(m - 1).map_err(|_| self.err("integer out of CBOR range"))?;
            return Ok(Diag::Nint(n));
        }
        token
            .parse()
            .map(Diag::Uint)
            .map_err(|_| self.err("integer out of CBOR range"))
    }

    fn word(&mut self) -> Result<Diag, ParseError> {
        for (word, value) in [
            ("true", Diag::Bool(true)),
            ("false", Diag::Bool(false)),
            ("null", Diag::Null),
            ("NaN", Diag::Float(f64::NAN)),
            ("Infinity", Diag::Float(f64::INFINITY)),
        ] {
            if self.starts_with(word) {
                self.pos += word.len();
                return Ok(value);
            }
        }
        Err(self.err("unexpected character"))
    }
}

/// Decodes hex digits (no separators) into bytes.
pub fn hex_decode(digits: &[u8]) -> Result<Vec<u8>, String> {
    if !digits.len().is_multiple_of(2) {
        return Err("odd number of hex digits".into());
    }
    digits
        .chunks(2)
        .map(|pair| {
            let s = std::str::from_utf8(pair).map_err(|_| "invalid hex".to_string())?;
            u8::from_str_radix(s, 16).map_err(|_| format!("invalid hex `{s}`"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spec_header() {
        let d = parse(r#"{14: [1], 16: ["cbor"]}"#).unwrap();
        let Diag::Map { entries, open } = d else {
            panic!("not a map")
        };
        assert!(!open);
        assert_eq!(
            entries[0],
            (Diag::Uint(14), Diag::Array(vec![Diag::Uint(1)]))
        );
    }

    #[test]
    fn parses_tags_bytes_and_floats() {
        assert_eq!(
            parse("37(h'7d 44')").unwrap(),
            Diag::Tag(37, Box::new(Diag::Bytes(vec![0x7d, 0x44])))
        );
        assert_eq!(
            parse("1(1.5)").unwrap(),
            Diag::Tag(1, Box::new(Diag::Float(1.5)))
        );
        assert_eq!(parse("-1").unwrap(), Diag::Nint(0));
        assert_eq!(
            parse("-18446744073709551616").unwrap(),
            Diag::Nint(u64::MAX)
        );
    }

    #[test]
    fn parses_patterns() {
        let d = parse("{13: *, 19: $session, ...}").unwrap();
        assert!(d.is_pattern());
        assert_eq!(d.refs(), vec!["session"]);
        assert!(d.to_cbor().is_err());
    }

    #[test]
    fn parses_escapes() {
        assert_eq!(
            parse(r#""a\"\\\n\u0001😀""#).unwrap(),
            Diag::Text("a\"\\\n\u{1}\u{1F600}".into())
        );
    }

    #[test]
    fn rejects_trailing_input() {
        assert!(parse("1 2").is_err());
        assert!(parse("[1,").is_err());
    }
}
