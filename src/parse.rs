//! Parser for .ninja files.

use crate::eval::{EvalPart, EvalString, LazyVars, Vars};
use crate::scanner::{ParseError, ParseResult, Scanner};

#[derive(Debug)]
pub struct Rule {
    pub name: String,
    pub vars: LazyVars,
}

#[derive(Debug)]
pub struct Build<'a> {
    pub rule: &'a str,
    pub line: usize,
    pub outs: Vec<String>,
    pub explicit_outs: usize,
    pub ins: Vec<String>,
    pub explicit_ins: usize,
    pub implicit_ins: usize,
    pub order_only_ins: usize,
    pub vars: LazyVars,
}

#[derive(Debug)]
pub struct Pool<'a> {
    pub name: &'a str,
    pub depth: usize,
}

#[derive(Debug)]
pub enum Statement<'a> {
    Rule(Rule),
    Build(Build<'a>),
    Default(Vec<String>),
    Include(String),
    Subninja(String),
    Pool(Pool<'a>),
}

pub struct Parser<'a> {
    scanner: Scanner<'a>,
    pub vars: Vars<'a>,
}

impl<'a> Parser<'a> {
    pub fn new(scanner: Scanner<'a>) -> Parser<'a> {
        Parser {
            scanner,
            vars: Vars::new(),
        }
    }

    pub fn format_parse_error(&self, filename: &str, err: ParseError) -> String {
        self.scanner.format_parse_error(filename, err)
    }

    pub fn read(&mut self) -> ParseResult<Option<Statement<'a>>> {
        loop {
            match self.scanner.peek() {
                '\0' => return Ok(None),
                '\n' => self.scanner.next(),
                '#' => self.skip_comment()?,
                ' ' | '\t' => return self.scanner.parse_error("unexpected whitespace"),
                _ => {
                    let ident = self.read_ident()?;
                    self.scanner.skip_spaces();
                    match ident {
                        "rule" => return Ok(Some(Statement::Rule(self.read_rule()?))),
                        "build" => return Ok(Some(Statement::Build(self.read_build()?))),
                        "default" => return Ok(Some(Statement::Default(self.read_default()?))),
                        "include" => {
                            let path = match self.read_path()? {
                                None => return self.scanner.parse_error("expected path"),
                                Some(p) => p,
                            };
                            return Ok(Some(Statement::Include(path)));
                        }
                        "subninja" => {
                            let path = match self.read_path()? {
                                None => return self.scanner.parse_error("expected path"),
                                Some(p) => p,
                            };
                            return Ok(Some(Statement::Subninja(path)));
                        }
                        "pool" => return Ok(Some(Statement::Pool(self.read_pool()?))),
                        ident => {
                            let val = self.read_vardef()?.evaluate(&[&self.vars]);
                            self.vars.insert(ident, val);
                        }
                    }
                }
            }
        }
    }

    fn read_vardef(&mut self) -> ParseResult<EvalString<&'a str>> {
        self.scanner.skip_spaces();
        self.scanner.expect('=')?;
        self.scanner.skip_spaces();
        self.read_eval()
    }

    fn read_scoped_vars(&mut self) -> ParseResult<LazyVars> {
        let mut vars = LazyVars::new();
        while self.scanner.peek() == ' ' {
            self.scanner.skip_spaces();
            let name = self.read_ident()?;
            self.scanner.skip_spaces();
            let val = self.read_vardef()?;
            vars.insert(name.to_owned(), val.into_owned());
        }
        Ok(vars)
    }

    fn read_rule(&mut self) -> ParseResult<Rule> {
        let name = self.read_ident()?;
        self.scanner.expect('\n')?;
        let vars = self.read_scoped_vars()?;
        Ok(Rule {
            name: name.to_owned(),
            vars,
        })
    }

    fn read_pool(&mut self) -> ParseResult<Pool<'a>> {
        let name = self.read_ident()?;
        self.scanner.expect('\n')?;
        let vars = self.read_scoped_vars()?;
        let mut depth = 0;
        for (key, val) in vars.keyvals() {
            match key.as_str() {
                "depth" => {
                    let val = val.evaluate(&[]);
                    depth = match val.parse::<usize>() {
                        Ok(d) => d,
                        Err(err) => {
                            return self.scanner.parse_error(format!("pool depth: {}", err))
                        }
                    }
                }
                _ => {
                    return self
                        .scanner
                        .parse_error(format!("unexpected pool attribute {:?}", key));
                }
            }
        }
        Ok(Pool { name, depth })
    }

    fn read_build(&mut self) -> ParseResult<Build<'a>> {
        let line = self.scanner.line;
        let mut outs = Vec::new();
        loop {
            self.scanner.skip_spaces();
            match self.read_path()? {
                Some(path) => outs.push(path),
                None => break,
            }
        }
        let explicit_outs = outs.len();

        if self.scanner.peek() == '|' {
            self.scanner.next();
            loop {
                self.scanner.skip_spaces();
                match self.read_path()? {
                    Some(path) => outs.push(path),
                    None => break,
                }
            }
        }

        self.scanner.expect(':')?;
        self.scanner.skip_spaces();
        let rule = self.read_ident()?;

        let mut ins = Vec::new();
        loop {
            self.scanner.skip_spaces();
            match self.read_path()? {
                Some(path) => ins.push(path),
                None => break,
            }
        }
        let explicit_ins = ins.len();

        if self.scanner.peek() == '|' {
            self.scanner.next();
            if self.scanner.peek() == '|' {
                self.scanner.back();
            } else {
                loop {
                    self.scanner.skip_spaces();
                    match self.read_path()? {
                        Some(path) => ins.push(path),
                        None => break,
                    }
                }
            }
        }
        let implicit_ins = ins.len() - explicit_ins;

        if self.scanner.peek() == '|' {
            self.scanner.next();
            self.scanner.expect('|')?;
            loop {
                self.scanner.skip_spaces();
                match self.read_path()? {
                    Some(path) => ins.push(path),
                    None => break,
                }
            }
        }
        let order_only_ins = ins.len() - implicit_ins - explicit_ins;

        self.scanner.expect('\n')?;
        let vars = self.read_scoped_vars()?;
        Ok(Build {
            rule,
            line,
            outs,
            explicit_outs,
            ins,
            explicit_ins,
            implicit_ins,
            order_only_ins,
            vars,
        })
    }

    fn read_default(&mut self) -> ParseResult<Vec<String>> {
        let mut defaults = Vec::new();
        loop {
            match self.read_path()? {
                Some(path) => defaults.push(path),
                None => break,
            }
            self.scanner.skip_spaces();
        }
        self.scanner.expect('\n')?;
        Ok(defaults)
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
                'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' => {}
                _ => {
                    self.scanner.back();
                    break;
                }
            }
        }
        let end = self.scanner.ofs;
        if end == start {
            return self.scanner.parse_error("failed to scan ident");
        }
        Ok(self.scanner.slice(start, end))
    }

    fn read_eval(&mut self) -> ParseResult<EvalString<&'a str>> {
        // Guaranteed at least one part.
        let mut parts = Vec::with_capacity(1);
        let mut ofs = self.scanner.ofs;
        loop {
            match self.scanner.read() {
                '\0' => return self.scanner.parse_error("unexpected EOF"),
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
        Ok(EvalString::new(parts))
    }

    fn read_path(&mut self) -> ParseResult<Option<String>> {
        let mut path = String::with_capacity(64);
        loop {
            match self.scanner.read() {
                '\0' => {
                    self.scanner.back();
                    return self.scanner.parse_error("unexpected EOF");
                }
                '$' => {
                    let part = self.read_escape()?;
                    match part {
                        EvalPart::Literal(l) => path.push_str(l),
                        EvalPart::VarRef(v) => {
                            if let Some(v) = self.vars.get(v) {
                                path.push_str(v);
                            }
                        }
                    }
                }
                ':' | '|' | ' ' | '\n' => {
                    self.scanner.back();
                    break;
                }
                c @ ('a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' | '/' | ',' | '+') => {
                    path.push(c);
                }
                c => {
                    self.scanner.back();
                    return self
                        .scanner
                        .parse_error(format!("unexpected character {:?}", c));
                }
            }
        }
        if path.is_empty() {
            return Ok(None);
        }
        Ok(Some(path))
    }

    fn read_escape(&mut self) -> ParseResult<EvalPart<&'a str>> {
        Ok(match self.scanner.peek() {
            '\n' => {
                self.scanner.next();
                self.scanner.skip_spaces();
                EvalPart::Literal(self.scanner.slice(0, 0))
            }
            ' ' | '$' | ':' => {
                self.scanner.next();
                EvalPart::Literal(self.scanner.slice(self.scanner.ofs - 1, self.scanner.ofs))
            }
            '{' => {
                self.scanner.next();
                let start = self.scanner.ofs;
                loop {
                    match self.scanner.read() {
                        '\0' => return self.scanner.parse_error("unexpected EOF"),
                        '}' => break,
                        _ => {}
                    }
                }
                let end = self.scanner.ofs - 1;
                EvalPart::VarRef(self.scanner.slice(start, end))
            }
            _ => {
                let ident = self.read_ident()?;
                EvalPart::VarRef(ident)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_defaults() {
        let mut parser = Parser::new(Scanner::new(
            "
var = 3
default a b$var c
\0",
        ));
        let default = match parser.read().unwrap().unwrap() {
            Statement::Default(d) => d,
            s => panic!("expected default, got {:?}", s),
        };
        assert_eq!(default, vec!["a", "b3", "c"]);
        println!("{:?}", default);
    }
}
