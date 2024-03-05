//! Scans an input string (source file) character by character.

use std::{io::Read, path::Path};

#[derive(Debug)]
pub struct ParseError {
    msg: String,
    ofs: usize,
    pub chunk_index: usize,
}
pub type ParseResult<T> = Result<T, ParseError>;

pub struct Scanner<'a> {
    buf: &'a [u8],
    pub ofs: usize,
    pub line: usize,
    pub chunk_index: usize,
}

impl<'a> Scanner<'a> {
    pub fn new(buf: &'a [u8], chunk_index: usize) -> Self {
        Scanner {
            buf,
            ofs: 0,
            line: 1,
            chunk_index,
        }
    }

    pub fn slice(&self, start: usize, end: usize) -> &'a str {
        unsafe { std::str::from_utf8_unchecked(self.buf.get_unchecked(start..end)) }
    }
    pub fn peek(&self) -> char {
        unsafe { *self.buf.get_unchecked(self.ofs) as char }
    }
    pub fn peek_newline(&self) -> bool {
        let peek = self.peek();
        if peek == '\n' {
            return true;
        }
        if self.ofs >= self.buf.len() - 1 {
            return false;
        }
        let peek2 = unsafe { *self.buf.get_unchecked(self.ofs + 1) as char };
        peek == '\r' && peek2 == '\n'
    }
    pub fn next(&mut self) {
        if self.peek() == '\n' {
            self.line += 1;
        }
        #[cfg(debug_assertions)]
        if self.ofs == self.buf.len() {
            panic!("scanned past end")
        }
        self.ofs += 1;
    }
    pub fn back(&mut self) {
        #[cfg(debug_assertions)]
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
            chunk_index: self.chunk_index,
        })
    }
}

pub fn format_parse_error(mut ofs: usize, buf: &[u8], filename: &Path, err: ParseError) -> String {
    let lines = buf.split(|&c| c == b'\n');
    for (line_number, line) in lines.enumerate() {
        if ofs + line.len() >= err.ofs {
            let mut msg = "parse error: ".to_string();
            msg.push_str(&err.msg);
            msg.push('\n');

            let prefix = format!("{}:{}: ", filename.display(), line_number + 1);
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

/// Scanner wants its input buffer to end in a trailing nul.
/// This function is like std::fs::read() but appends a nul, efficiently.
pub fn read_file_with_nul(path: &Path) -> std::io::Result<Vec<u8>> {
    // Using std::fs::read() to read the file and then pushing a nul on the end
    // causes us to allocate a buffer the size of the file, then grow it to push
    // the nul, copying the entire file(!).  So instead create a buffer of the
    // right size up front.
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len() as usize;
    let mut bytes = Vec::with_capacity(size + 1);
    unsafe {
        bytes.set_len(size);
    }
    file.read_exact(&mut bytes[..size])?;
    bytes.push(0);
    Ok(bytes)
}
