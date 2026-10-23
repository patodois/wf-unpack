//! The text notation of object attributes and Packages.bin properties.
//!
//! One `Key=Value` per line. A value is a scalar (to the end of the line, or
//! to the next `,` or `}` inside braces), a quoted string, or a block in
//! braces. A block holds either comma-separated values (`{0,0,0,1}`), or
//! `Key=Value` items one per line (`{a=1\nb=2\n}`), and blocks nest
//! (`{{name=X\ntype=Float\n},{name=Y\n}}`).
//!
//! The parser is lenient about whitespace and trailing NULs. It fails only on
//! unbalanced braces and quotes.

use anyhow::{bail, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A scalar; quotes removed.
    Text(String),
    /// Values without keys.
    List(Vec<Value>),
    /// Keyed items in file order. A block that mixes keyed and unkeyed items
    /// is a table too, with an empty key on the unkeyed ones.
    Table(Vec<(String, Value)>),
}

impl Value {
    pub fn text(&self) -> Option<&str> {
        match self {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }

    /// The first item named `key` of a table.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Table(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The items of a list; a table's values; a scalar as a one-item list.
    pub fn items(&self) -> Vec<&Value> {
        match self {
            Value::Text(_) => vec![self],
            Value::List(v) => v.iter().collect(),
            Value::Table(v) => v.iter().map(|(_, v)| v).collect(),
        }
    }

    /// A list of numbers (`{1,2,3}`) or a single number.
    pub fn floats(&self) -> Option<Vec<f32>> {
        match self {
            Value::Text(t) => t.trim().parse().ok().map(|f| vec![f]),
            Value::List(v) => v
                .iter()
                .map(|x| x.text().and_then(|t| t.trim().parse().ok()))
                .collect(),
            Value::Table(_) => None,
        }
    }

    pub fn float(&self) -> Option<f32> {
        self.floats().and_then(|v| v.first().copied())
    }
}

impl std::fmt::Display for Value {
    /// The value in the notation it was read from (whitespace normalised).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Text(t) => write!(f, "{t}"),
            Value::List(v) => {
                write!(f, "{{")?;
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{x}")?;
                }
                write!(f, "}}")
            }
            Value::Table(v) => {
                write!(f, "{{")?;
                for (i, (k, x)) in v.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    if k.is_empty() {
                        write!(f, "{x}")?;
                    } else {
                        write!(f, "{k}={x}")?;
                    }
                }
                write!(f, "}}")
            }
        }
    }
}

/// A parsed attribute text: keyed values in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Attributes(pub Vec<(String, Value)>);

impl Attributes {
    /// The last value set for `key` (later lines override earlier ones).
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::text)
    }

    pub fn float(&self, key: &str) -> Option<f32> {
        self.get(key).and_then(Value::float)
    }

    pub fn floats(&self, key: &str) -> Option<Vec<f32>> {
        self.get(key).and_then(Value::floats)
    }

    /// A flag such as `StartHidden=1`.
    pub fn flag(&self, key: &str) -> Option<bool> {
        self.text(key).map(|v| v != "0" && !v.is_empty())
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut p = Parser {
            s: text.as_bytes(),
            i: 0,
        };
        p.top().map(Attributes)
    }
}

/// Parses the text of one value, as it follows `Key=` in an attribute text.
pub fn parse_value(text: &str) -> Result<Value> {
    let mut p = Parser {
        s: text.as_bytes(),
        i: 0,
    };
    p.value(false)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

fn is_key_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b':' | b'-' | b'[' | b']')
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn skip_space(&mut self, newlines: bool) {
        while let Some(b) = self.peek() {
            match b {
                b' ' | b'\t' | b'\r' | 0 => {}
                b'\n' if newlines => {}
                _ => break,
            }
            self.i += 1;
        }
    }

    fn string(&self, a: usize, b: usize) -> String {
        String::from_utf8_lossy(&self.s[a..b]).trim().to_string()
    }

    fn top(&mut self) -> Result<Vec<(String, Value)>> {
        let mut out = Vec::new();
        loop {
            self.skip_space(true);
            if self.peek().is_none() {
                return Ok(out);
            }
            let start = self.i;
            while self.peek().is_some_and(|b| b != b'=' && b != b'\n') {
                self.i += 1;
            }
            let key = self.string(start, self.i);
            if self.peek() == Some(b'=') {
                self.i += 1;
                let value = self.value(false)?;
                out.push((key, value));
            } else if !key.is_empty() {
                out.push((key, Value::Text(String::new())));
            }
        }
    }

    fn value(&mut self, nested: bool) -> Result<Value> {
        self.skip_space(false);
        match self.peek() {
            Some(b'{') => {
                self.i += 1;
                self.block()
            }
            Some(b'"') => {
                self.i += 1;
                let start = self.i;
                loop {
                    match self.peek() {
                        None => bail!("unterminated quote"),
                        Some(b'\\') => self.i += 2,
                        Some(b'"') => break,
                        _ => self.i += 1,
                    }
                }
                let text = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
                self.i += 1;
                Ok(Value::Text(text))
            }
            _ => {
                let start = self.i;
                while self
                    .peek()
                    .is_some_and(|b| b != b'\n' && !(nested && (b == b',' || b == b'}')))
                {
                    self.i += 1;
                }
                Ok(Value::Text(self.string(start, self.i)))
            }
        }
    }

    /// After the opening brace.
    fn block(&mut self) -> Result<Value> {
        let mut items: Vec<(String, Value)> = Vec::new();
        loop {
            while self
                .peek()
                .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b',' | 0))
            {
                self.i += 1;
            }
            match self.peek() {
                None => bail!("unterminated block"),
                Some(b'}') => {
                    self.i += 1;
                    break;
                }
                _ => {}
            }
            // `ident=` starts a keyed item
            let mut j = self.i;
            while self.s.get(j).is_some_and(|&b| is_key_byte(b)) {
                j += 1;
            }
            let key = if j > self.i && self.s.get(j) == Some(&b'=') {
                let key = self.string(self.i, j);
                self.i = j + 1;
                key
            } else {
                String::new()
            };
            let value = self.value(true)?;
            items.push((key, value));
        }
        Ok(if items.iter().all(|(k, _)| k.is_empty()) {
            Value::List(items.into_iter().map(|(_, v)| v).collect())
        } else {
            Value::Table(items)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Value {
        Value::Text(s.to_string())
    }

    #[test]
    fn scalars_lists_and_tables() {
        let a = Attributes::parse(
            "Instance=3\nLightMap={\"\",\"\"}\nCoords={0,0.5,-1,1}\nTag=Some Name\nMesh=\n\0",
        )
        .unwrap();
        assert_eq!(a.text("Instance"), Some("3"));
        assert_eq!(a.get("LightMap"), Some(&Value::List(vec![t(""), t("")])));
        assert_eq!(a.floats("Coords"), Some(vec![0.0, 0.5, -1.0, 1.0]));
        assert_eq!(a.text("Tag"), Some("Some Name"));
        assert_eq!(a.text("Mesh"), Some(""));
        assert!(a.get("Missing").is_none());
    }

    #[test]
    fn nested_blocks() {
        let a = Attributes::parse(
            "MaterialParams={{name=TintColor\ntype=Color\nslot=-1\nvalue={0.38,0.29,0.19,0.73}\n},{name=B\ntype=Float\nvalue=2\n}}\nAfter=1\n",
        )
        .unwrap();
        let params = a.get("MaterialParams").unwrap().items();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].get("name"), Some(&t("TintColor")));
        assert_eq!(
            params[0].get("value").unwrap().floats(),
            Some(vec![0.38, 0.29, 0.19, 0.73])
        );
        assert_eq!(params[1].get("value").unwrap().float(), Some(2.0));
        assert_eq!(a.text("After"), Some("1"));
    }

    #[test]
    fn keyed_block_across_lines() {
        let a = Attributes::parse("Ports={_a=/Lotus/X\n_b=2\n}\nFlag=1\n").unwrap();
        let p = a.get("Ports").unwrap();
        assert_eq!(p.get("_a"), Some(&t("/Lotus/X")));
        assert_eq!(p.get("_b"), Some(&t("2")));
        assert_eq!(a.flag("Flag"), Some(true));
    }

    #[test]
    fn later_lines_override_and_errors() {
        let a = Attributes::parse("K=1\nK=0\n").unwrap();
        assert_eq!(a.flag("K"), Some(false));
        assert!(Attributes::parse("K={1,2\n").is_err());
        assert!(Attributes::parse("K=\"open\n").is_err());
    }
}
