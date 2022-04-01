//! Scans an input string (source file) character by character.

#[derive(Debug)]
pub struct ParseError {
    msg: String,
    ofs: usize,
}
pub type ParseResult<T> = Result<T, ParseError>;

pub struct Scanner<'a> {
    buf: &'a [u8],
    pub ofs: usize,
    pub line: usize,
}

impl<'a> Scanner<'a> {
    pub fn new(buf: &'a mut Vec<u8>) -> Self {
        buf.push(0);
        Scanner {
            buf,
            ofs: 0,
            line: 1,
        }
    }

    pub fn slice(&self, start: usize, end: usize) -> &'a str {
        unsafe { std::str::from_utf8_unchecked(self.buf.get_unchecked(start..end)) }
    }
    pub fn peek(&self) -> char {
        unsafe { *self.buf.get_unchecked(self.ofs) as char }
    }
    pub fn next(&mut self) {
        if self.peek() == '\n' {
            self.line += 1;
        }
        if self.ofs == self.buf.len() {
            panic!("scanned past end")
        }
        self.ofs += 1;
    }
    pub fn back(&mut self) {
        if self.ofs == 0 {
            panic!("back at start")
        }
        self.ofs -= 1;
        if self.peek() == '\n' {
            self.line -= 1;
        }
    }
    pub fn read(&mut self) -> char {
        let c = self.peek();
        self.next();
        c
    }
    pub fn skip(&mut self, ch: char) -> bool {
        if self.peek() == ch {
            self.next();
            return true;
        }
        false
    }

    pub fn skip_spaces(&mut self) {
        while self.skip(' ') {}
    }

    pub fn expect(&mut self, ch: char) -> ParseResult<()> {
        let r = self.read();
        if r != ch {
            self.back();
            return self.parse_error(format!("expected {:?}, got {:?}", ch, r));
        }
        Ok(())
    }

    pub fn parse_error<T, S: Into<String>>(&self, msg: S) -> ParseResult<T> {
        Err(ParseError {
            msg: msg.into(),
            ofs: self.ofs,
        })
    }

    pub fn format_parse_error(&self, filename: &str, err: ParseError) -> String {
        let mut ofs = 0;
        let lines = self.buf.split(|&c| c == b'\n');
        for (line_number, line) in lines.enumerate() {
            if ofs + line.len() >= err.ofs {
                let mut msg = "parse error: ".to_string();
                msg.push_str(&err.msg);
                msg.push('\n');

                let prefix = format!("{}:{}: ", filename, line_number + 1);
                msg.push_str(&prefix);

                let mut context = unsafe { std::str::from_utf8_unchecked(line) };
                let mut col = err.ofs - ofs;
                if col > 40 {
                    // Trim beginning of line to fit it on screen.
                    msg.push_str("...");
                    context = &context[col - 20..];
                    col = 3 + 20;
                }
                if context.len() > 40 {
                    context = &context[0..40];
                    msg.push_str(context);
                    msg.push_str("...");
                } else {
                    msg.push_str(context);
                }
                msg.push('\n');

                msg.push_str(&" ".repeat(prefix.len() + col));
                msg.push_str("^\n");
                return msg;
            }
            ofs += line.len() + 1;
        }
        panic!("invalid offset when formatting error")
    }
}
