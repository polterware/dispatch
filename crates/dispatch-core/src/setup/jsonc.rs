/// A value's byte range in the source, end exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
}

/// What cutting an entry out of the plugins list needs of a JSON or JSONC
/// object: the plugins list's range and its elements' ranges.
#[derive(Debug, Default)]
pub(crate) struct JsoncDoc {
    pub plugins_array: Option<Span>,
    pub plugins: Vec<Span>,
}

struct Scanner<'a> {
    src: &'a [u8],
    pos: usize,
}

/// Reads the top-level object of src, which may hold comments and trailing
/// commas.
pub(crate) fn parse_jsonc(src: &[u8]) -> Result<JsoncDoc, String> {
    let mut s = Scanner { src, pos: 0 };
    let mut doc = JsoncDoc::default();
    s.space();
    if s.peek() != b'{' {
        return Err("the config is not a JSON object".into());
    }
    s.pos += 1;
    loop {
        s.space();
        if s.peek() == b'}' {
            return Ok(doc);
        }
        let key = s.str()?;
        s.space();
        if s.peek() != b':' {
            return Err(s.error("expected ':'"));
        }
        s.pos += 1;
        s.space();
        if key == b"\"plugins\"" && s.peek() == b'[' {
            let (arr, els) = s.array()?;
            doc.plugins_array = Some(arr);
            doc.plugins = els;
        } else {
            s.value()?;
        }
        s.space();
        match s.peek() {
            b',' => s.pos += 1,
            b'}' => {}
            _ => return Err(s.error("expected ',' or '}'")),
        }
    }
}

impl<'a> Scanner<'a> {
    fn error(&self, msg: &str) -> String {
        format!("invalid JSON at byte {}: {msg}", self.pos)
    }

    fn peek(&self) -> u8 {
        self.src.get(self.pos).copied().unwrap_or(0)
    }

    fn at(&self, i: usize) -> Option<u8> {
        self.src.get(i).copied()
    }

    // space skips whitespace and comments.
    fn space(&mut self) {
        while let Some(c) = self.at(self.pos) {
            match c {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                b'/' if self.at(self.pos + 1) == Some(b'/') => {
                    while self.at(self.pos).is_some_and(|c| c != b'\n') {
                        self.pos += 1;
                    }
                }
                b'/' if self.at(self.pos + 1) == Some(b'*') => {
                    self.pos += 2;
                    while self.pos + 1 < self.src.len()
                        && (self.src[self.pos] != b'*' || self.src[self.pos + 1] != b'/')
                    {
                        self.pos += 1;
                    }
                    self.pos += 2;
                }
                _ => return,
            }
        }
    }

    // str reads a string and returns it with its quotes.
    fn str(&mut self) -> Result<&'a [u8], String> {
        if self.peek() != b'"' {
            return Err(self.error("expected a string"));
        }
        let start = self.pos;
        self.pos += 1;
        while let Some(c) = self.at(self.pos) {
            match c {
                b'\\' => self.pos += 2,
                b'"' => {
                    self.pos += 1;
                    return Ok(&self.src[start..self.pos]);
                }
                _ => self.pos += 1,
            }
        }
        Err(self.error("unterminated string"))
    }

    // value skips one value and returns its range.
    fn value(&mut self) -> Result<Span, String> {
        let start = self.pos;
        match self.peek() {
            b'"' => {
                self.str()?;
                Ok(Span {
                    start,
                    end: self.pos,
                })
            }
            b'[' => Ok(self.array()?.0),
            b'{' => {
                self.pos += 1;
                loop {
                    self.space();
                    if self.peek() == b'}' {
                        self.pos += 1;
                        return Ok(Span {
                            start,
                            end: self.pos,
                        });
                    }
                    self.str()?;
                    self.space();
                    if self.peek() != b':' {
                        return Err(self.error("expected ':'"));
                    }
                    self.pos += 1;
                    self.space();
                    self.value()?;
                    self.space();
                    match self.peek() {
                        b',' => self.pos += 1,
                        b'}' => {}
                        _ => return Err(self.error("expected ',' or '}'")),
                    }
                }
            }
            0 => Err(self.error("unexpected end")),
            _ => {
                while let Some(c) = self.at(self.pos) {
                    if matches!(c, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r' | b'/') {
                        break;
                    }
                    self.pos += 1;
                }
                if self.pos == start {
                    return Err(self.error("expected a value"));
                }
                Ok(Span {
                    start,
                    end: self.pos,
                })
            }
        }
    }

    // array reads an array and returns its range and its elements' ranges.
    fn array(&mut self) -> Result<(Span, Vec<Span>), String> {
        let start = self.pos;
        self.pos += 1;
        let mut els = Vec::new();
        loop {
            self.space();
            if self.peek() == b']' {
                self.pos += 1;
                return Ok((
                    Span {
                        start,
                        end: self.pos,
                    },
                    els,
                ));
            }
            els.push(self.value()?);
            self.space();
            match self.peek() {
                b',' => self.pos += 1,
                b']' => {}
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
    }
}

/// Drops comments and trailing commas so a JSON parser reads a value.
pub(crate) fn strip_jsonc(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    let mut s = Scanner { src, pos: 0 };
    while let Some(c) = s.at(s.pos) {
        match c {
            b'"' => {
                let start = s.pos;
                if s.str().is_err() {
                    out.extend_from_slice(&src[start..]);
                    return out;
                }
                out.extend_from_slice(&src[start..s.pos]);
            }
            b'/' if matches!(s.at(s.pos + 1), Some(b'/' | b'*')) => s.space(),
            b',' => {
                s.pos += 1;
                s.space();
                let next = s.peek();
                if next != b'}' && next != b']' {
                    out.push(b',');
                }
            }
            _ => {
                out.push(c);
                s.pos += 1;
            }
        }
    }
    out
}
