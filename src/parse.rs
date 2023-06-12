//! Parser for .ninja files.
//!
//! See design notes on parsing in doc/design_notes.md.
//!
//! To avoid allocations parsing frequently uses references into the input
//! text, marked with the lifetime `'text`.

use std::path::Path;

use crate::eval::{EvalPart, EvalString, LazyVars, Vars};
use crate::scanner::{ParseError, ParseResult, Scanner};

pub struct Rule<'text> {
    pub name: &'text str,
    pub vars: LazyVars,
}

pub struct Build<'text, Path> {
    pub rule: &'text str,
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
pub struct Pool<'text> {
    pub name: &'text str,
    pub depth: usize,
}

pub enum Statement<'text, Path> {
    Rule(Rule<'text>),
    Build(Build<'text, Path>),
    Default(Vec<Path>),
    Include(Path),
    Subninja(Path),
    Pool(Pool<'text>),
}

pub struct Parser<'text> {
    scanner: Scanner<'text>,
    pub vars: Vars<'text>,
    /// Reading paths is very hot when parsing, so we always read into this buffer
    /// and then immediately pass in to Loader::path() to canonicalize it in-place.
    path_buf: Vec<u8>,
}

/// Loader maps path strings (as found in build.ninja files) into an arbitrary
/// "Path" type.  This allows us to canonicalize and convert path strings to
/// more efficient integer identifiers while we parse, rather than needing to
/// buffer up many intermediate strings; in fact, parsing uses a single buffer
/// for all of these.
pub trait Loader {
    type Path;
    /// Convert a path string to a Self::Path type.  For performance reasons
    /// this may mutate the 'path' param.
    fn path(&mut self, path: &mut str) -> Self::Path;
}

impl<'text> Parser<'text> {
    pub fn new(buf: &'text mut Vec<u8>) -> Parser<'text> {
        Parser {
            scanner: Scanner::new(buf),
            vars: Vars::default(),
            path_buf: Vec::with_capacity(64),
        }
    }

    pub fn format_parse_error(&self, filename: &Path, err: ParseError) -> String {
        self.scanner.format_parse_error(filename, err)
    }

    pub fn read<L: Loader>(
        &mut self,
        loader: &mut L,
    ) -> ParseResult<Option<Statement<'text, L::Path>>> {
        loop {
            match self.scanner.peek() {
                '\0' => return Ok(None),
                '\n' | '\r' => self.scanner.next(),
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

    fn read_vardef(&mut self) -> ParseResult<EvalString<&'text str>> {
        self.scanner.skip_spaces();
        self.scanner.expect('=')?;
        self.scanner.skip_spaces();
        self.read_eval()
    }

    fn read_scoped_vars(&mut self) -> ParseResult<LazyVars> {
        let mut vars = LazyVars::default();
        while self.scanner.peek() == ' ' {
            self.scanner.skip_spaces();
            let name = self.read_ident()?;
            self.scanner.skip_spaces();
            let val = self.read_vardef()?;
            vars.insert(name.to_owned(), val.into_owned());
        }
        Ok(vars)
    }

    fn read_rule(&mut self) -> ParseResult<Rule<'text>> {
        let name = self.read_ident()?;
        self.scanner.skip('\r');
        self.scanner.expect('\n')?;
        let vars = self.read_scoped_vars()?;
        Ok(Rule { name, vars })
    }

    fn read_pool(&mut self) -> ParseResult<Pool<'text>> {
        let name = self.read_ident()?;
        self.scanner.skip('\r');
        self.scanner.expect('\n')?;
        let vars = self.read_scoped_vars()?;
        let mut depth = 0;
        for (key, val) in vars.iter() {
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

    fn read_build<L: Loader>(&mut self, loader: &mut L) -> ParseResult<Build<'text, L::Path>> {
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

        self.scanner.skip('\r');
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
        self.scanner.skip('\r');
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

    /// Read an identifier -- rule name, pool name, variable name, etc.
    fn read_ident(&mut self) -> ParseResult<&'text str> {
        let start = self.scanner.ofs;
        while matches!(
            self.scanner.read(),
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.'
        ) {}
        self.scanner.back();
        let end = self.scanner.ofs;
        if end == start {
            return self.scanner.parse_error("failed to scan ident");
        }
        Ok(self.scanner.slice(start, end))
    }

    fn read_eval(&mut self) -> ParseResult<EvalString<&'text str>> {
        // Guaranteed at least one part.
        let mut parts = Vec::with_capacity(1);
        let mut ofs = self.scanner.ofs;
        let end = loop {
            match self.scanner.read() {
                '\0' => return self.scanner.parse_error("unexpected EOF"),
                '\n' => break self.scanner.ofs - 1,
                '\r' if self.scanner.peek() == '\n' => {
                    self.scanner.next();
                    break self.scanner.ofs - 2;
                }
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
        };
        if end > ofs {
            parts.push(EvalPart::Literal(self.scanner.slice(ofs, end)));
        }
        Ok(EvalString::new(parts))
    }

    fn read_path<L: Loader>(&mut self, loader: &mut L) -> ParseResult<Option<L::Path>> {
        self.path_buf.clear();
        loop {
            let c = self.scanner.read();
            match c {
                '\0' => {
                    self.scanner.back();
                    return self.scanner.parse_error("unexpected EOF");
                }
                '$' => {
                    let part = self.read_escape()?;
                    match part {
                        EvalPart::Literal(l) => self.path_buf.extend_from_slice(l.as_bytes()),
                        EvalPart::VarRef(v) => {
                            if let Some(v) = self.vars.get(v) {
                                self.path_buf.extend_from_slice(v.as_bytes());
                            }
                        }
                    }
                }
                ':' | '|' | ' ' | '\n' | '\r' => {
                    // Basically any character is allowed in paths, but we want to parse e.g.
                    //   build foo: bar | baz
                    // such that the colon is not part of the 'foo' path and such that '|' is
                    // not read as a path.
                    // Those characters can be embedded by escaping, e.g. "$:".
                    self.scanner.back();
                    break;
                }
                c => {
                    self.path_buf.push(c as u8);
                }
            }
        }
        if self.path_buf.is_empty() {
            return Ok(None);
        }
        // Performance: this is some of the hottest code in n2 so we cut some corners.
        // Safety: see discussion of unicode safety in doc/development.md.
        // I looked into switching this to BStr but it would require changing
        // a lot of other code to BStr too.
        let path_str = unsafe { std::str::from_utf8_unchecked_mut(&mut self.path_buf) };
        let path = loader.path(path_str);
        Ok(Some(path))
    }

    /// Read a variable name as found after a '$' in an eval.
    /// Ninja calls this a "simple" varname and it is the same as read_ident without
    /// period allowed(!), I guess because we expect things like
    ///   foo = $bar.d
    /// to parse as a reference to $bar.
    fn read_simple_varname(&mut self) -> ParseResult<&'text str> {
        let start = self.scanner.ofs;
        while matches!(self.scanner.read(), 'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-') {}
        self.scanner.back();
        let end = self.scanner.ofs;
        if end == start {
            return self.scanner.parse_error("failed to scan variable name");
        }
        Ok(self.scanner.slice(start, end))
    }

    /// Read and interpret the text following a '$' escape character.
    fn read_escape(&mut self) -> ParseResult<EvalPart<&'text str>> {
        Ok(match self.scanner.read() {
            '\n' | '\r' => {
                self.scanner.skip_spaces();
                EvalPart::Literal(self.scanner.slice(0, 0))
            }
            ' ' | '$' | ':' => {
                EvalPart::Literal(self.scanner.slice(self.scanner.ofs - 1, self.scanner.ofs))
            }
            '{' => {
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
                // '$' followed by some other text.
                self.scanner.back();
                let var = self.read_simple_varname()?;
                EvalPart::VarRef(var)
            }
        })
    }
}

#[cfg(test)]
struct StringLoader {}
#[cfg(test)]
impl Loader for StringLoader {
    type Path = String;
    fn path(&mut self, path: &mut str) -> Self::Path {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_for_line_endings(input: &[&str], test: fn(&str)) {
        let test_case_lf = input.join("\n");
        let test_case_crlf = input.join("\r\n");
        for test_case in [test_case_lf, test_case_crlf] {
            test(&test_case);
        }
    }

    #[test]
    fn parse_defaults() {
        test_for_line_endings(&["var = 3", "default a b$var c", ""], |test_case| {
            let mut buf = test_case.as_bytes().to_vec();
            let mut parser = Parser::new(&mut buf);
            let default = match parser.read(&mut StringLoader {}).unwrap().unwrap() {
                Statement::Default(d) => d,
                _ => panic!("expected default"),
            };
            assert_eq!(default, vec!["a", "b3", "c"]);
        });
    }

    #[test]
    fn parse_dot_in_eval() {
        let mut buf = "x = $y.z\n".as_bytes().to_vec();
        let mut parser = Parser::new(&mut buf);
        parser.read(&mut StringLoader {}).unwrap();
        let x = parser.vars.get("x").unwrap();
        assert_eq!(x, ".z");
    }

    #[test]
    fn parse_dot_in_rule() {
        let mut buf = "rule x.y\n  command = x\n".as_bytes().to_vec();
        let mut parser = Parser::new(&mut buf);
        let stmt = parser.read(&mut StringLoader {}).unwrap().unwrap();
        assert!(matches!(
            stmt,
            Statement::Rule(Rule {
                name: "x.y",
                vars: _
            })
        ));
    }
}
