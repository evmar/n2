//! Parser for .ninja files.

use crate::eval::{EvalPart, EvalString, LazyVars, Vars};
use crate::scanner::{ParseError, ParseResult, Scanner};

#[derive(Debug)]
pub struct Rule<'a> {
    pub name: &'a str,
    pub vars: LazyVars,
}

#[derive(Debug)]
pub struct Build<'a, Path> {
    pub rule: &'a str,
    pub line: usize,
    pub outs: Vec<Path>,
    pub explicit_outs: usize,
    pub ins: Vec<Path>,
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
pub enum Statement<'a, Path> {
    Rule(Rule<'a>),
    Build(Build<'a, Path>),
    Default(Vec<Path>),
    Include(Path),
    Subninja(Path),
    Pool(Pool<'a>),
}

pub struct Parser<'a> {
    scanner: Scanner<'a>,
    pub vars: Vars<'a>,
}

/// Baseline implementation of is_ident_char.
#[cfg(test)]
fn is_ident_char_baseline(c: u8) -> bool {
    match c as char {
        'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' => true,
        _ => false,
    }
}

/// Lookup table implementation of is_ident_char.  Produces same output as
/// _baseline version.  256-entry table is encoded as 4 64-bit integers.
/// See gen_lookup_table.py for how it was generated.
fn is_ident_char(c: u8) -> bool {
    let lookup: [u64; 4] = [0x3ff600000000000, 0x7fffffe87fffffe, 0x0, 0x0];
    (lookup[(c >> 6) as usize] & ((1 as u64) << (c & 63))) != 0
}

#[cfg(test)]
fn is_path_char_baseline(c: u8) -> bool {
    match c as char {
        'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' | '/' | ',' | '+' => true,
        _ => false,
    }
}

fn is_path_char(c: u8) -> bool {
    let lookup: [u64; 4] = [0x3fff80000000000, 0x7fffffe87fffffe, 0x0, 0x0];
    (lookup[(c >> 6) as usize] & ((1 as u64) << (c & 63))) != 0
}

pub trait Loader {
    type Path;
    fn path(&mut self, path: &str) -> Self::Path;
}

impl<'a> Parser<'a> {
    pub fn new(buf: &'a mut Vec<u8>) -> Parser<'a> {
        Parser {
            scanner: Scanner::new(buf),
            vars: Vars::new(),
        }
    }

    pub fn format_parse_error(&self, filename: &str, err: ParseError) -> String {
        self.scanner.format_parse_error(filename, err)
    }

    pub fn read<L: Loader>(
        &mut self,
        loader: &mut L,
    ) -> ParseResult<Option<Statement<'a, L::Path>>> {
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
                        "build" => return Ok(Some(Statement::Build(self.read_build(loader)?))),
                        "default" => {
                            return Ok(Some(Statement::Default(self.read_default(loader)?)))
                        }
                        "include" => {
                            let id = match self.read_path(loader)? {
                                None => return self.scanner.parse_error("expected path"),
                                Some(p) => p,
                            };
                            return Ok(Some(Statement::Include(id)));
                        }
                        "subninja" => {
                            let id = match self.read_path(loader)? {
                                None => return self.scanner.parse_error("expected path"),
                                Some(p) => p,
                            };
                            return Ok(Some(Statement::Subninja(id)));
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

    fn read_rule(&mut self) -> ParseResult<Rule<'a>> {
        let name = self.read_ident()?;
        self.scanner.expect('\n')?;
        let vars = self.read_scoped_vars()?;
        Ok(Rule { name, vars })
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

    fn read_paths_to<L: Loader>(
        &mut self,
        loader: &mut L,
        v: &mut Vec<L::Path>,
    ) -> ParseResult<()> {
        self.scanner.skip_spaces();
        while let Some(path) = self.read_path(loader)? {
            v.push(path);
            self.scanner.skip_spaces();
        }
        Ok(())
    }

    fn read_build<L: Loader>(&mut self, loader: &mut L) -> ParseResult<Build<'a, L::Path>> {
        let line = self.scanner.line;
        let mut outs = Vec::new();
        self.read_paths_to(loader, &mut outs)?;
        let explicit_outs = outs.len();

        if self.scanner.peek() == '|' {
            self.scanner.next();
            self.read_paths_to(loader, &mut outs)?;
        }

        self.scanner.expect(':')?;
        self.scanner.skip_spaces();
        let rule = self.read_ident()?;

        let mut ins = Vec::new();
        self.read_paths_to(loader, &mut ins)?;
        let explicit_ins = ins.len();

        if self.scanner.peek() == '|' {
            self.scanner.next();
            if self.scanner.peek() == '|' {
                self.scanner.back();
            } else {
                self.read_paths_to(loader, &mut ins)?;
            }
        }
        let implicit_ins = ins.len() - explicit_ins;

        if self.scanner.peek() == '|' {
            self.scanner.next();
            self.scanner.expect('|')?;
            self.read_paths_to(loader, &mut ins)?;
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

    fn read_default<L: Loader>(&mut self, loader: &mut L) -> ParseResult<Vec<L::Path>> {
        let mut defaults = Vec::new();
        while let Some(path) = self.read_path(loader)? {
            defaults.push(path);
            self.scanner.skip_spaces();
        }
        if defaults.is_empty() {
            return self.scanner.parse_error("expected path");
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
        while is_ident_char(self.scanner.read() as u8) {}
        self.scanner.back();
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

    fn read_path<L: Loader>(&mut self, loader: &mut L) -> ParseResult<Option<L::Path>> {
        let mut path = String::with_capacity(64);
        loop {
            let c = self.scanner.read();
            if is_path_char(c as u8) {
                path.push(c);
            } else {
                match c {
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
                    c => {
                        self.scanner.back();
                        return self
                            .scanner
                            .parse_error(format!("unexpected character {:?}", c));
                    }
                }
            }
        }
        if path.is_empty() {
            return Ok(None);
        }
        Ok(Some(loader.path(&path)))
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

struct StringLoader {}
impl Loader for StringLoader {
    type Path = String;
    fn path(&mut self, path: &str) -> Self::Path {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_defaults() {
        let mut buf = "
var = 3
default a b$var c
        "
        .as_bytes()
        .to_vec();
        let mut parser = Parser::new(&mut buf);
        let default = match parser.read(&mut StringLoader {}).unwrap().unwrap() {
            Statement::Default(d) => d,
            s => panic!("expected default, got {:?}", s),
        };
        assert_eq!(default, vec!["a", "b3", "c"]);
        println!("{:?}", default);
    }

    #[test]
    fn lookup_tables_match_baseline() {
        for i in (0 as u8)..=255 {
            assert_eq!(
                is_ident_char(i),
                is_ident_char_baseline(i),
                "mismatch at {}",
                i
            );

            assert_eq!(
                is_path_char(i),
                is_path_char_baseline(i),
                "mismatch at {}",
                i
            );
        }
    }
}
