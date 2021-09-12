use std::collections::HashMap;
use std::result::Result;

#[derive(Debug)]
pub struct ParseError {
    msg: String,
    ofs: usize,
}
type ParseResult<T> = Result<T, ParseError>;

struct Scanner<'a> {
    buf: &'a str,
    ofs: usize,
}

impl<'a> Scanner<'a> {
    fn slice(&self, start: usize, end: usize) -> &'a str {
        unsafe { self.buf.get_unchecked(start..end) }
    }
    fn peek(&self) -> char {
        self.buf.as_bytes()[self.ofs] as char
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

pub trait Env {
    fn get_var(&self, var: &str) -> Option<String>;
}

#[derive(Debug)]
enum EvalPart<T: AsRef<str>> {
    Literal(T),
    VarRef(T),
}
#[derive(Debug)]
pub struct EvalString<T: AsRef<str>>(Vec<EvalPart<T>>);

impl<T: AsRef<str>> EvalString<T> {
    pub fn evaluate(&self, envs: &[&dyn Env]) -> String {
        let mut val = String::new();
        for part in &self.0 {
            match part {
                EvalPart::Literal(s) => val.push_str(s.as_ref()),
                EvalPart::VarRef(v) => {
                    for env in envs {
                        if let Some(v) = env.get_var(v.as_ref()) {
                            val.push_str(&v);
                            break;
                        }
                    }
                }
            }
        }
        val
    }
}
impl EvalString<&str> {
    pub fn to_owned(self) -> EvalString<String> {
        EvalString(self.0.into_iter().map(|part| {
            match part {
                EvalPart::Literal(s) => EvalPart::Literal(s.to_owned()),
                EvalPart::VarRef(s) => EvalPart::VarRef(s.to_owned()),
            }
        }).collect())
    }
}

#[derive(Debug)]
pub struct ResolvedEnv<'a>(HashMap<&'a str, String>);
impl<'a> ResolvedEnv<'a> {
    pub fn new() -> ResolvedEnv<'a> {
        ResolvedEnv(HashMap::new())
    }
}
impl<'a> Env for ResolvedEnv<'a> {
    fn get_var(&self, var: &str) -> Option<String> {
        self.0.get(var).map(|val| val.clone())
    }
}

#[derive(Debug)]
pub struct LazyVars(Vec<(String, EvalString<String>)>);
impl LazyVars {
    pub fn new() -> Self {
        LazyVars(Vec::new())
    }
    pub fn get(&self, key: &str) -> Option<&EvalString<String>> {
        for (k, v) in &self.0 {
            if k == key {
                return Some(v);
            }
        }
        None
    }
}
impl Env for LazyVars {
    fn get_var(&self, var: &str) -> Option<String> {
        self.get(var).map(|val| val.evaluate(&[]))
    }
}

#[derive(Debug)]
pub struct Rule {
    pub name: String,
    pub vars: LazyVars,
}

#[derive(Debug)]
pub struct Build<'a> {
    pub rule: &'a str,
    pub outs: Vec<String>,
    pub ins: Vec<String>,
    pub vars: LazyVars,
}

#[derive(Debug)]
pub enum Statement<'a> {
    Rule(Rule),
    Build(Build<'a>),
    Default(&'a str),
    Include(String),
}

pub struct Parser<'a> {
    scanner: Scanner<'a>,
    pub vars: ResolvedEnv<'a>,
}

impl<'a> Parser<'a> {
    pub fn new(text: &'a str) -> Parser<'a> {
        Parser {
            scanner: Scanner { buf: text, ofs: 0 },
            vars: ResolvedEnv::new(),
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
        let lines = self.scanner.buf.split('\n');
        for line in lines {
            if ofs + line.len() >= err.ofs {
                let mut msg = err.msg.clone();
                msg.push('\n');
                msg.push_str(line);
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
                    match ident {
                        "rule" => return Ok(Some(Statement::Rule(self.read_rule()?))),
                        "build" => return Ok(Some(Statement::Build(self.read_build()?))),
                        "default" => return Ok(Some(Statement::Default(self.read_ident()?))),
                        "include" => {
                            let path = match self.read_path()? {
                                None => return self.parse_error("expected path"),
                                Some(p) => p,
                            };
                            return Ok(Some(Statement::Include(path)));
                        }
                        ident => {
                            let val = self.read_vardef()?.evaluate(&[&self.vars]);
                            self.vars.0.insert(ident, val);
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

    fn read_vardef(&mut self) -> ParseResult<EvalString<&'a str>> {
        self.skip_spaces();
        self.expect('=')?;
        self.skip_spaces();
        return self.read_eval();
    }

    fn read_scoped_vars(&mut self) -> ParseResult<LazyVars> {
        let mut vars = LazyVars(Vec::new());
        while self.scanner.peek() == ' ' {
            self.skip_spaces();
            let name = self.read_ident()?;
            self.skip_spaces();
            let val = self.read_vardef()?;
            vars.0.push((name.to_owned(), val.to_owned()));
        }
        Ok(vars)
    }

    fn read_rule(&mut self) -> ParseResult<Rule> {
        let name = self.read_ident()?;
        self.expect('\n')?;
        let vars = self.read_scoped_vars()?;
        Ok(Rule {
            name: name.to_owned(),
            vars: vars,
        })
    }

    fn read_build(&mut self) -> ParseResult<Build<'a>> {
        let mut outs = Vec::new();
        loop {
            self.skip_spaces();
            if self.scanner.peek() == '|' {  // TODO implicit output
                self.scanner.next();
                self.skip_spaces();
            }
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

    fn read_ident(&mut self) -> ParseResult<&'a str> {
        let start = self.scanner.ofs;
        loop {
            match self.scanner.read() {
                'a'..='z' | 'A'..='Z' | '_' => {}
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
        Ok(var)
    }

    fn skip_spaces(&mut self) {
        while self.scanner.peek() == ' ' {
            self.scanner.next();
        }
    }

    fn read_eval(&mut self) -> ParseResult<EvalString<&'a str>> {
        let mut parts = Vec::new();
        let mut ofs = self.scanner.ofs;
        loop {
            match self.scanner.read() {
                '\0' => return self.parse_error("unexpected EOF"),
                '\n' => break,
                '$' => {
                    let end = self.scanner.ofs - 1;
                    if end > ofs {
                        parts.push(EvalPart::Literal(self.scanner.slice(ofs, end)));
                    }
                    parts.push(self.read_escape()?);
                    ofs = self.scanner.ofs;
                }
                _ => {}
            }
        }
        let end = self.scanner.ofs - 1;
        if end > ofs {
            parts.push(EvalPart::Literal(self.scanner.slice(ofs, end)));
        }
        Ok(EvalString(parts))
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
                    let part = self.read_escape()?;
                    match part {
                        EvalPart::Literal(l) => path.push_str(l),
                        EvalPart::VarRef(v) => {
                            if let Some(v) = self.vars.0.get(v) {
                                path.push_str(v);
                            }
                        }
                    }
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

    fn read_escape(&mut self) -> ParseResult<EvalPart<&'a str>> {
        match self.scanner.peek() {
            '\n' => {
                self.scanner.next();
                self.skip_spaces();
                return Ok(EvalPart::Literal(self.scanner.slice(0, 0)));
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
                return Ok(EvalPart::VarRef(self.scanner.slice(start, end)));
            }
            _ => {
                let ident = self.read_ident()?;
                return Ok(EvalPart::VarRef(ident));
            }
        }
    }
}
