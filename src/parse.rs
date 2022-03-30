//! Parser for .ninja files.
//!
//! See design notes on parsing in doc/design_notes.md.
//!
//! To avoid allocations parsing frequently uses references into the input
//! text, marked with the lifetime `'text`.

use crate::eval::{EvalPart, EvalString, LazyVars, Vars};
use crate::scanner::{ParseError, ParseResult, Scanner};

#[derive(Debug)]
pub struct Rule<'text> {
    pub name: &'text str,
    pub vars: LazyVars,
}

#[derive(Debug)]
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

#[derive(Debug)]
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
    path_buf: String,
}

// 256-entry lookup table bitmap encoded as 4 64-bit integers.
type Bitmap = [u64; 4];

/// Returns a (index, mask) tuple for testing/setting the n-th bit in a bitmap.
#[inline(always)]
const fn bitmap_index_and_mask(c: u8) -> (usize, u64) {
    let index = c as usize >> 6;
    let mask = 1u64 << (c & 63);
    (index, mask)
}

/// Baseline implementation of is_ident_char.
const fn is_ident_char_baseline(c: u8) -> bool {
    matches!(c as char, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.')
}

/// Generates a character matching lookup table at compile time.
const fn ident_char_bitmap() -> Bitmap {
    let mut bitmap = [0u64; 4];
    let mut c = 0u8;
    loop {
        if is_ident_char_baseline(c) {
            let (index, mask) = bitmap_index_and_mask(c);
            bitmap[index] |= mask;
        }
        match c {
            u8::MAX => break,
            _ => c += 1,
        }
    }
    bitmap
}

/// Lookup table implementation of is_ident_char. Produces same output as
/// _baseline version.
fn is_ident_char(c: u8) -> bool {
    const BITMAP: Bitmap = ident_char_bitmap();
    let (index, mask) = bitmap_index_and_mask(c);
    (BITMAP[index] & mask) != 0
}

const fn is_path_char_baseline(c: u8) -> bool {
    matches!(
      c as char,
      'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' | '/' | ',' | '+' | '@'
    )
}

const fn path_char_bitmap() -> Bitmap {
    let mut bitmap = [0u64; 4];
    let mut c = 0u8;
    loop {
        if is_path_char_baseline(c) {
            let (index, mask) = bitmap_index_and_mask(c);
            bitmap[index] |= mask;
        }
        match c {
            u8::MAX => break,
            _ => c += 1,
        }
    }
    bitmap
}

fn is_path_char(c: u8) -> bool {
    const BITMAP: Bitmap = path_char_bitmap();
    let (index, mask) = bitmap_index_and_mask(c);
    (BITMAP[index] & mask) != 0
}

pub trait Loader {
    type Path;
    fn path(&mut self, path: &mut String) -> Self::Path;
}

impl<'text> Parser<'text> {
    pub fn new(buf: &'text mut Vec<u8>) -> Parser<'text> {
        Parser {
            scanner: Scanner::new(buf),
            vars: Vars::new(),
            path_buf: String::with_capacity(64),
        }
    }

    pub fn format_parse_error(&self, filename: &str, err: ParseError) -> String {
        self.scanner.format_parse_error(filename, err)
    }

    pub fn read<L: Loader>(
        &mut self,
        loader: &mut L,
    ) -> ParseResult<Option<Statement<'text, L::Path>>> {
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

    fn read_vardef(&mut self) -> ParseResult<EvalString<&'text str>> {
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

    fn read_rule(&mut self) -> ParseResult<Rule<'text>> {
        let name = self.read_ident()?;
        self.scanner.expect('\n')?;
        let vars = self.read_scoped_vars()?;
        Ok(Rule { name, vars })
    }

    fn read_pool(&mut self) -> ParseResult<Pool<'text>> {
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

    fn read_ident(&mut self) -> ParseResult<&'text str> {
        let start = self.scanner.ofs;
        while is_ident_char(self.scanner.read() as u8) {}
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
        self.path_buf.clear();
        loop {
            let c = self.scanner.read();
            if is_path_char(c as u8) {
                self.path_buf.push(c);
            } else {
                match c {
                    '\0' => {
                        self.scanner.back();
                        return self.scanner.parse_error("unexpected EOF");
                    }
                    '$' => {
                        let part = self.read_escape()?;
                        match part {
                            EvalPart::Literal(l) => self.path_buf.push_str(l),
                            EvalPart::VarRef(v) => {
                                if let Some(v) = self.vars.get(v) {
                                    self.path_buf.push_str(v);
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
        if self.path_buf.is_empty() {
            return Ok(None);
        }
        Ok(Some(loader.path(&mut self.path_buf)))
    }

    fn read_escape(&mut self) -> ParseResult<EvalPart<&'text str>> {
        Ok(match self.scanner.read() {
            '\n' => {
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
                self.scanner.back();
                let ident = self.read_ident()?;
                EvalPart::VarRef(ident)
            }
        })
    }
}

struct StringLoader {}
impl Loader for StringLoader {
    type Path = String;
    fn path(&mut self, path: &mut String) -> Self::Path {
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
        for i in 0u8..=255 {
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
