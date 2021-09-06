use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::result::Result;

#[derive(Debug)]
pub struct ParseError {
    msg: String,
    ofs: usize,
}
type ParseResult<T> = Result<T, ParseError>;

#[derive(Eq, PartialEq, Hash, Clone)]
pub struct NString(pub Vec<u8>);
impl<'a> std::fmt::Debug for NString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.write_fmt(format_args!("{:?}", &String::from_utf8_lossy(&self.0)))
    }
}
impl NString {
    pub fn from_str(str: &str) -> NString {
        NString(str.as_bytes().to_vec())
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    pub fn as_nstr(&self) -> NStr {
        NStr(&self.0)
    }
}

#[derive(Eq, PartialEq, Hash)]
pub struct NStr<'a>(&'a [u8]);
impl<'a> std::fmt::Debug for NStr<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.write_fmt(format_args!("{:?}", &String::from_utf8_lossy(self.0)))
    }
}
impl NStr<'_> {
    pub fn to_nstring(&self) -> NString {
        NString(Vec::from(self.0))
    }
    pub fn as_bytes(&self) -> &[u8] {
        self.0
    }
    pub fn as_path(&self) -> &std::path::Path {
        std::path::Path::new(std::ffi::OsStr::from_bytes(self.as_bytes()))
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
enum EvalPart<'a> {
    Literal(NStr<'a>),
    VarRef(NStr<'a>),
}
#[derive(Debug)]
struct EvalString<'a> {
    parts: Vec<EvalPart<'a>>,
}

#[derive(Debug)]
pub struct Env<'a>(HashMap<NStr<'a>, NString>);
impl<'a> Env<'a> {
    pub fn new() -> Env<'a> {
        Env(HashMap::new())
    }

    fn evaluate(&self, eval: &EvalString) -> NString {
        let mut val = Vec::new();
        for part in &eval.parts {
            match part {
                EvalPart::Literal(s) => val.extend_from_slice(s.0),
                EvalPart::VarRef(v) => {
                    if let Some(v) = self.0.get(&v) {
                        val.extend_from_slice(&v.0);
                    }
                }
            }
        }
        NString(val)
    }
}

#[derive(Debug)]
pub struct DelayEnv<'a>(HashMap<NStr<'a>, EvalString<'a>>);

#[derive(Debug)]
pub struct Rule<'a> {
    name: NStr<'a>,
    vars: DelayEnv<'a>,
}

#[derive(Debug)]
pub struct Build<'a> {
    pub rule: NStr<'a>,
    pub outs: Vec<NString>,
    pub ins: Vec<NString>,
    pub vars: DelayEnv<'a>,
}

#[derive(Debug)]
pub enum Statement<'a> {
    Rule(Rule<'a>),
    Build(Build<'a>),
    Default(NStr<'a>),
}

pub struct Parser<'a> {
    scanner: Scanner<'a>,
    vars: Env<'a>,
}

impl<'a> Parser<'a> {
    pub fn new(text: &'a [u8]) -> Parser<'a> {
        Parser {
            scanner: Scanner { buf: text, ofs: 0 },
            vars: Env(HashMap::new()),
        }
    }
    fn parse_error<T, S: Into<String>>(&self, msg: S) -> ParseResult<T> {
        Err(ParseError {
            msg: msg.into(),
            ofs: self.scanner.ofs,
        })
    }

    pub fn format_parse_error(&self, err: ParseError) -> String {
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

    pub fn read(&mut self) -> ParseResult<Option<Statement<'a>>> {
        loop {
            match self.scanner.peek() {
                '\0' => return Ok(None),
                '\n' => self.scanner.next(),
                '#' => self.skip_comment()?,
                ' ' | '\t' => return self.parse_error("unexpected whitespace"),
                _ => {
                    let ident = self.read_ident()?;
                    self.skip_spaces();
                    match ident.0 {
                        b"rule" => return Ok(Some(Statement::Rule(self.read_rule()?))),
                        b"build" => return Ok(Some(Statement::Build(self.read_build()?))),
                        b"default" => return Ok(Some(Statement::Default(self.read_ident()?))),
                        ident => {
                            let valvar = self.read_vardef()?;
                            let val = self.vars.evaluate(&valvar);
                            self.vars.0.insert(NStr(ident), val);
                        }
                    }
                }
            }
        }
    }

    fn expect(&mut self, ch: char) -> ParseResult<()> {
        if self.scanner.read() != ch {
            self.scanner.back();
            return self.parse_error(format!("expected {:?}", ch));
        }
        Ok(())
    }

    fn read_vardef(&mut self) -> ParseResult<EvalString<'a>> {
        self.skip_spaces();
        self.expect('=')?;
        self.skip_spaces();
        return self.read_eval();
    }

    fn read_scoped_vars(&mut self) -> ParseResult<DelayEnv<'a>> {
        let mut vars = DelayEnv(HashMap::new());
        while self.scanner.peek() == ' ' {
            self.skip_spaces();
            let name = self.read_ident()?;
            self.skip_spaces();
            let val = self.read_vardef()?;
            vars.0.insert(name, val);
        }
        Ok(vars)
    }

    fn read_rule(&mut self) -> ParseResult<Rule<'a>> {
        let name = self.read_ident()?;
        self.expect('\n')?;
        let vars = self.read_scoped_vars()?;
        Ok(Rule {
            name: name,
            vars: vars,
        })
    }

    fn read_build(&mut self) -> ParseResult<Build<'a>> {
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
        let vars = self.read_scoped_vars()?;
        Ok(Build {
            rule: rule,
            outs: outs,
            ins: ins,
            vars: vars,
        })
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

    fn read_eval(&mut self) -> ParseResult<EvalString<'a>> {
        let mut parts = Vec::new();
        let mut ofs = self.scanner.ofs;
        loop {
            match self.scanner.read() {
                '\0' => return self.parse_error("unexpected EOF"),
                '\n' => break,
                '$' => {
                    let end = self.scanner.ofs - 1;
                    if end > ofs {
                        parts.push(EvalPart::Literal(NStr(&self.scanner.buf[ofs..end])));
                    }
                    parts.push(self.read_escape()?);
                    ofs = self.scanner.ofs;
                }
                _ => {}
            }
        }
        let end = self.scanner.ofs - 1;
        if end > ofs {
            parts.push(EvalPart::Literal(NStr(&self.scanner.buf[ofs..end])));
        }
        Ok(EvalString { parts: parts })
    }

    fn read_path(&mut self) -> ParseResult<Option<NString>> {
        let mut path = Vec::new();
        loop {
            match self.scanner.read() {
                '\0' => {
                    self.scanner.back();
                    return self.parse_error("unexpected EOF");
                }
                '$' => {
                    let part = self.read_escape()?;
                    match part {
                        EvalPart::Literal(l) => path.extend_from_slice(l.0),
                        EvalPart::VarRef(v) => {
                            if let Some(v) = &self.vars.0.get(&v) {
                                path.extend_from_slice(&v.0);
                            }
                        }
                    }
                }
                ':' | '|' | ' ' | '\n' => {
                    self.scanner.back();
                    break;
                }
                c => {
                    path.push(c as u8);
                }
            }
        }
        if path.len() == 0 {
            return Ok(None);
        }
        Ok(Some(NString(path)))
    }

    fn read_escape(&mut self) -> ParseResult<EvalPart<'a>> {
        match self.scanner.peek() {
            '\n' => {
                self.scanner.next();
                self.skip_spaces();
                return Ok(EvalPart::Literal(NStr(&self.scanner.buf[0..0])));
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
                return Ok(EvalPart::VarRef(NStr(&self.scanner.buf[start..end])));
            }
            _ => {
                let ident = self.read_ident()?;
                return Ok(EvalPart::VarRef(ident));
            }
        }
    }
}
