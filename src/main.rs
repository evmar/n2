use std::result::Result;

#[derive(Debug)]
struct ParseError {
    msg: String,
    ofs: usize,
}
type ParseResult<T> = Result<T, ParseError>;

struct NStr<'a>(&'a [u8]);
impl<'a> std::fmt::Debug for NStr<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.write_fmt(format_args!("{:?}", &String::from_utf8_lossy(self.0)))
    }
}

struct Scanner<'a> {
    buf: &'a [u8],
    ofs: usize,
}

impl<'a> Scanner<'a> {
    fn peek(&self) -> char {
        self.buf[self.ofs] as char
    }
    fn next(&mut self) {
        if self.ofs == self.buf.len() {
            panic!("scanned past end")
        }
        self.ofs += 1;
    }
    fn back(&mut self) {
        if self.ofs == 0 {
            panic!("back at start")
        }
        self.ofs -= 1;
    }
    fn read(&mut self) -> char {
        let c = self.peek();
        self.next();
        c
    }
}

#[derive(Debug)]
enum VarPart<'a> {
    Literal(NStr<'a>),
    VarRef(NStr<'a>),
}
#[derive(Debug)]
struct Var<'a> {
    parts: Vec<VarPart<'a>>,
}

struct Parser<'a> {
    scanner: Scanner<'a>,
}

impl<'a> Parser<'a> {
    fn parse_error<T, S: Into<String>>(&self, msg: S) -> ParseResult<T> {
        Err(ParseError {
            msg: msg.into(),
            ofs: self.scanner.ofs,
        })
    }

    fn format_parse_error(&self, err: ParseError) -> String {
        let mut ofs = 0;
        let lines = self.scanner.buf.split(|c| (*c as char) == '\n');
        for line in lines {
            if ofs + line.len() >= err.ofs {
                let mut msg = err.msg.clone();
                msg.push('\n');
                msg.push_str(&String::from_utf8_lossy(line));
                msg.push('\n');
                msg.push_str(&" ".repeat(err.ofs - ofs));
                msg.push_str("^\n");
                return msg;
            }
            ofs += line.len() + 1;
        }
        panic!("invalid offset when formatting error")
    }

    fn parse(&mut self) -> ParseResult<()> {
        loop {
            match self.scanner.peek() {
                '\0' => break,
                '\n' => self.scanner.next(),
                '#' => self.skip_comment()?,
                ' ' | '\t' => return self.parse_error("unexpected whitespace"),
                _ => {
                    let ident = self.read_ident()?;
                    self.skip_spaces();
                    match ident.0 {
                        b"rule" => self.read_rule()?,
                        b"build" => self.read_build()?,
                        b"default" => self.read_default()?,
                        _ => self.read_vardef(ident)?,
                    }
                }
            }
        }
        Ok(())
    }

    fn expect(&mut self, ch: char) -> ParseResult<()> {
        if self.scanner.read() != ch {
            self.scanner.back();
            return self.parse_error(format!("expected {:?}", ch));
        }
        Ok(())
    }

    fn read_vardef(&mut self, name: NStr) -> ParseResult<()> {
        self.skip_spaces();
        let val = self.read_value()?;
        println!("{:?} is {:?}", name, val);
        Ok(())
    }

    fn read_scoped_vars(&mut self) -> ParseResult<()> {
        while self.scanner.peek() == ' ' {
            self.skip_spaces();
            let varname = self.read_ident()?;
            self.skip_spaces();
            self.read_vardef(varname)?;
        }
        Ok(())
    }

    fn read_rule(&mut self) -> ParseResult<()> {
        let name = self.read_ident()?;
        self.expect('\n')?;
        println!("rule {:?}", name);
        self.read_scoped_vars()?;
        Ok(())
    }

    fn read_build(&mut self) -> ParseResult<()> {
        let mut outs = Vec::new();
        loop {
            self.skip_spaces();
            match self.read_path()? {
                Some(path) => outs.push(path),
                None => break,
            }
        }
        self.skip_spaces();
        self.expect(':')?;
        self.skip_spaces();
        let rule = self.read_ident()?;
        let mut ins = Vec::new();
        loop {
            self.skip_spaces();
            if self.scanner.peek() == '|' {
                self.scanner.next();
                if self.scanner.peek() == '|' {
                    self.scanner.next();
                }
                self.skip_spaces();
            }
            match self.read_path()? {
                Some(path) => ins.push(path),
                None => break,
            }
        }
        self.expect('\n')?;
        println!("build {:?} {:?} {:?}", outs, rule, ins);
        self.read_scoped_vars()?;
        Ok(())
    }

    fn read_default(&mut self) -> ParseResult<()> {
        let name = self.read_ident()?;
        println!("default: {:?}", name);
        Ok(())
    }

    fn skip_comment(&mut self) -> ParseResult<()> {
        loop {
            match self.scanner.read() {
                '\0' => {
                    self.scanner.back();
                    return Ok(());
                }
                '\n' => return Ok(()),
                _ => {}
            }
        }
    }

    fn read_ident(&mut self) -> ParseResult<NStr<'a>> {
        let start = self.scanner.ofs;
        loop {
            match self.scanner.read() {
                'a'..='z' | '_' => {}
                _ => {
                    self.scanner.back();
                    break;
                }
            }
        }
        let end = self.scanner.ofs;
        if end == start {
            return self.parse_error("failed to scan ident");
        }
        let var = &self.scanner.buf[start..end];
        Ok(NStr(var))
    }

    fn skip_spaces(&mut self) {
        while self.scanner.peek() == ' ' {
            self.scanner.next();
        }
    }

    fn read_value(&mut self) -> ParseResult<Var<'a>> {
        let mut parts = Vec::new();
        let mut ofs = self.scanner.ofs;
        loop {
            match self.scanner.read() {
                '\0' => return self.parse_error("unexpected EOF"),
                '\n' => break,
                '$' => {
                    let end = self.scanner.ofs - 1;
                    if end > ofs {
                        parts.push(VarPart::Literal(NStr(&self.scanner.buf[ofs..end])));
                    }
                    parts.push(self.read_escape()?);
                    ofs = self.scanner.ofs;
                }
                _ => {}
            }
        }
        let end = self.scanner.ofs - 1;
        if end > ofs {
            parts.push(VarPart::Literal(NStr(&self.scanner.buf[ofs..end])));
        }
        Ok(Var { parts: parts })
    }

    fn read_path(&mut self) -> ParseResult<Option<String>> {
        let mut path = String::new();
        loop {
            match self.scanner.read() {
                '\0' => {
                    self.scanner.back();
                    return self.parse_error("unexpected EOF");
                }
                '$' => {
                    self.read_escape()?;
                    path.push_str("$TODO");
                }
                ':' | '|' | ' ' | '\n' => {
                    self.scanner.back();
                    break;
                }
                c => {
                    path.push(c);
                }
            }
        }
        if path.len() == 0 {
            return Ok(None);
        }
        Ok(Some(path))
    }

    fn read_escape(&mut self) -> ParseResult<VarPart<'a>> {
        match self.scanner.peek() {
            '\n' => {
                self.scanner.next();
                self.skip_spaces();
                return Ok(VarPart::Literal(NStr(&self.scanner.buf[0..0])));
            }
            '{' => {
                self.scanner.next();
                let start = self.scanner.ofs;
                loop {
                    match self.scanner.read() {
                        '\0' => return self.parse_error("unexpected EOF"),
                        '}' => break,
                        _ => {}
                    }
                }
                let end = self.scanner.ofs - 1;
                return Ok(VarPart::VarRef(NStr(&self.scanner.buf[start..end])));
            }
            _ => {
                let ident = self.read_ident()?;
                return Ok(VarPart::VarRef(ident));
            }
        }
    }
}

fn read() -> std::io::Result<()> {
    let mut bytes = std::fs::read("build.ninja")?;
    bytes.push(0);
    let mut p = Parser {
        scanner: Scanner {
            buf: &bytes,
            ofs: 0,
        },
    };
    match p.parse() {
        Err(err) => println!("{}", p.format_parse_error(err)),
        Ok(p) => println!("parsed as {:?}", p),
    }
    Ok(())
}

fn main() {
    read().unwrap();
}
