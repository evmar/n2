//! Parser for .ninja files.
//!
//! See design notes on parsing in doc/design_notes.md.
//!
//! To avoid allocations parsing frequently uses references into the input
//! text, marked with the lifetime `'text`.

use crate::{
    eval::{EvalPart, EvalString, Vars},
    scanner::{ParseError, ParseResult, Scanner},
    smallmap::SmallMap,
};
use std::path::Path;

/// A list of variable bindings, as expressed with syntax like:
///   key = $val
pub type VarList<'text> = SmallMap<&'text str, EvalString<&'text str>>;

#[derive(Debug, PartialEq)]
pub struct Rule<'text> {
    pub name: &'text str,
    pub vars: VarList<'text>,
}

#[derive(Debug, PartialEq)]
pub struct Build<'text> {
    pub rule: &'text str,
    pub line: usize,
    pub outs: Vec<EvalString<&'text str>>,
    pub explicit_outs: usize,
    pub ins: Vec<EvalString<&'text str>>,
    pub explicit_ins: usize,
    pub implicit_ins: usize,
    pub order_only_ins: usize,
    pub validation_ins: usize,
    pub vars: VarList<'text>,
}

#[derive(Debug, PartialEq)]
pub struct Pool<'text> {
    pub name: &'text str,
    pub depth: usize,
}

#[derive(Debug, PartialEq)]
pub enum Statement<'text> {
    Rule(Rule<'text>),
    Build(Build<'text>),
    Default(Vec<EvalString<&'text str>>),
    Include(EvalString<&'text str>),
    Subninja(EvalString<&'text str>),
    Pool(Pool<'text>),
    VariableAssignment((&'text str, EvalString<&'text str>)),
}

pub struct Parser<'text> {
    scanner: Scanner<'text>,
    buf_len: usize,
    /// Reading EvalStrings is very hot when parsing, so we always read into
    /// this buffer and then clone it afterwards.
    eval_buf: Vec<EvalPart<&'text str>>,
}

impl<'text> Parser<'text> {
    pub fn new(buf: &'text [u8]) -> Parser<'text> {
        Parser {
            scanner: Scanner::new(buf),
            buf_len: buf.len(),
            eval_buf: Vec::with_capacity(16),
        }
    }

    pub fn format_parse_error(&self, filename: &Path, err: ParseError) -> String {
        self.scanner.format_parse_error(filename, err)
    }

    pub fn read_all(&mut self) -> ParseResult<Vec<Statement<'text>>> {
        let mut result = Vec::new();
        while let Some(stmt) = self.read()? {
            result.push(stmt)
        }
        Ok(result)
    }

    pub fn read_to_channel(&mut self, sender: std::sync::mpsc::Sender<ParseResult<Statement<'text>>>) {
        loop {
            match self.read() {
                Ok(None) => return,
                Ok(Some(stmt)) => sender.send(Ok(stmt)).unwrap(),
                Err(e) => {
                    sender.send(Err(e)).unwrap();
                    return;
                },
            }
        }
    }

    pub fn read(&mut self) -> ParseResult<Option<Statement<'text>>> {
        loop {
            match self.scanner.peek() {
                '\0' => return Ok(None),
                '\n' | '\r' => self.scanner.next(),
                '#' => self.skip_comment()?,
                ' ' | '\t' => return self.scanner.parse_error("unexpected whitespace"),
                _ => {
                    if self.scanner.ofs >= self.buf_len {
                        // The parsing code expects there to be a null byte at the end of the file,
                        // to allow the parsing to be more performant and exclude most checks for
                        // EOF. However, when parsing an individual "chunk" of the manifest, there
                        // won't be a null byte at the end, the scanner will do an out-of-bounds
                        // read past the end of the chunk and into the next chunk. When we split
                        // the file into chunks, we made sure to end all the chunks just before
                        // identifiers at the start of a new line, so that we can easily detect
                        // that here.
                        assert!(self.scanner.ofs == self.buf_len);
                        return Ok(None)
                    }
                    let ident = self.read_ident()?;
                    self.skip_spaces();
                    match ident {
                        "rule" => return Ok(Some(Statement::Rule(self.read_rule()?))),
                        "build" => return Ok(Some(Statement::Build(self.read_build()?))),
                        "default" => return Ok(Some(Statement::Default(self.read_default()?))),
                        "include" => {
                            return Ok(Some(Statement::Include(self.read_eval(false)?)));
                        }
                        "subninja" => {
                            return Ok(Some(Statement::Subninja(self.read_eval(false)?)));
                        }
                        "pool" => return Ok(Some(Statement::Pool(self.read_pool()?))),
                        ident => {
                            return Ok(Some(Statement::VariableAssignment((ident, self.read_vardef()?))))
                        }
                    }
                }
            }
        }
    }

    /// Read the `= ...` part of a variable definition.
    fn read_vardef(&mut self) -> ParseResult<EvalString<&'text str>> {
        self.skip_spaces();
        self.scanner.expect('=')?;
        self.skip_spaces();
        // read_eval will error out if there's nothing to read
        if self.scanner.peek_newline() {
            self.scanner.skip('\r');
            self.scanner.expect('\n')?;
            return Ok(EvalString::new(Vec::new()));
        }
        let result = self.read_eval(false);
        self.scanner.skip('\r');
        self.scanner.expect('\n')?;
        result
    }

    /// Read a collection of `  foo = bar` variables, with leading indent.
    fn read_scoped_vars(
        &mut self,
        variable_name_validator: fn(var: &str) -> bool,
    ) -> ParseResult<VarList<'text>> {
        let mut vars = VarList::default();
        while self.scanner.peek() == ' ' {
            self.scanner.skip_spaces();
            let name = self.read_ident()?;
            if !variable_name_validator(name) {
                self.scanner
                    .parse_error(format!("unexpected variable {:?}", name))?;
            }
            self.skip_spaces();
            let val = self.read_vardef()?;
            vars.insert(name, val);
        }
        Ok(vars)
    }

    fn read_rule(&mut self) -> ParseResult<Rule<'text>> {
        let name = self.read_ident()?;
        self.scanner.skip('\r');
        self.scanner.expect('\n')?;
        let vars = self.read_scoped_vars(|var| {
            matches!(
                var,
                "command"
                    | "depfile"
                    | "dyndep"
                    | "description"
                    | "deps"
                    | "generator"
                    | "pool"
                    | "restat"
                    | "rspfile"
                    | "rspfile_content"
                    | "msvc_deps_prefix"
            )
        })?;
        Ok(Rule { name, vars })
    }

    fn read_pool(&mut self) -> ParseResult<Pool<'text>> {
        let name = self.read_ident()?;
        self.scanner.skip('\r');
        self.scanner.expect('\n')?;
        let vars = self.read_scoped_vars(|var| matches!(var, "depth"))?;
        let mut depth = 0;
        if let Some((_, val)) = vars.into_iter().next() {
            let val = val.evaluate(&[]);
            depth = match val.parse::<usize>() {
                Ok(d) => d,
                Err(err) => return self.scanner.parse_error(format!("pool depth: {}", err)),
            }
        }
        Ok(Pool { name, depth })
    }

    fn read_unevaluated_paths_to(
        &mut self,
        v: &mut Vec<EvalString<&'text str>>,
    ) -> ParseResult<()> {
        self.skip_spaces();
        while self.scanner.peek() != ':'
            && self.scanner.peek() != '|'
            && !self.scanner.peek_newline()
        {
            v.push(self.read_eval(true)?);
            self.skip_spaces();
        }
        Ok(())
    }

    fn read_build(&mut self) -> ParseResult<Build<'text>> {
        let line = self.scanner.line;
        let mut outs = Vec::new();
        self.read_unevaluated_paths_to(&mut outs)?;
        let explicit_outs = outs.len();

        if self.scanner.peek() == '|' {
            self.scanner.next();
            self.read_unevaluated_paths_to(&mut outs)?;
        }

        self.scanner.expect(':')?;
        self.skip_spaces();
        let rule = self.read_ident()?;

        let mut ins = Vec::new();
        self.read_unevaluated_paths_to(&mut ins)?;
        let explicit_ins = ins.len();

        if self.scanner.peek() == '|' {
            self.scanner.next();
            let peek = self.scanner.peek();
            if peek == '|' || peek == '@' {
                self.scanner.back();
            } else {
                self.read_unevaluated_paths_to(&mut ins)?;
            }
        }
        let implicit_ins = ins.len() - explicit_ins;

        if self.scanner.peek() == '|' {
            self.scanner.next();
            if self.scanner.peek() == '@' {
                self.scanner.back();
            } else {
                self.scanner.expect('|')?;
                self.read_unevaluated_paths_to(&mut ins)?;
            }
        }
        let order_only_ins = ins.len() - implicit_ins - explicit_ins;

        if self.scanner.peek() == '|' {
            self.scanner.next();
            self.scanner.expect('@')?;
            self.read_unevaluated_paths_to(&mut ins)?;
        }
        let validation_ins = ins.len() - order_only_ins - implicit_ins - explicit_ins;

        self.scanner.skip('\r');
        self.scanner.expect('\n')?;
        let vars = self.read_scoped_vars(|_| true)?;
        Ok(Build {
            rule,
            line,
            outs,
            explicit_outs,
            ins,
            explicit_ins,
            implicit_ins,
            order_only_ins,
            validation_ins,
            vars,
        })
    }

    fn read_default(&mut self) -> ParseResult<Vec<EvalString<&'text str>>> {
        let mut defaults = Vec::new();
        self.read_unevaluated_paths_to(&mut defaults)?;
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

    /// Reads an EvalString. Stops at either a newline, or ' ', ':', '|' if
    /// stop_at_path_separators is set, without consuming the character that
    /// caused it to stop.
    fn read_eval(&mut self, stop_at_path_separators: bool) -> ParseResult<EvalString<&'text str>> {
        self.eval_buf.clear();
        let mut ofs = self.scanner.ofs;
        // This match block is copied twice, with the only difference being the check for
        // spaces, colons, and pipes in the stop_at_path_separators version. We could remove the
        // duplication by adding a match branch like `' ' | ':' | '|' if stop_at_path_separators =>`
        // or even moving the `if stop_at_path_separators` inside of the match body, but both of
        // those options are ~10% slower on a benchmark test of running the loader on llvm-cmake
        // ninja files.
        let end = if stop_at_path_separators {
            loop {
                match self.scanner.read() {
                    '\0' => return self.scanner.parse_error("unexpected EOF"),
                    ' ' | ':' | '|' | '\n' => {
                        self.scanner.back();
                        break self.scanner.ofs;
                    }
                    '\r' if self.scanner.peek() == '\n' => {
                        self.scanner.back();
                        break self.scanner.ofs;
                    }
                    '$' => {
                        let end = self.scanner.ofs - 1;
                        if end > ofs {
                            self.eval_buf
                                .push(EvalPart::Literal(self.scanner.slice(ofs, end)));
                        }
                        let escape = self.read_escape()?;
                        self.eval_buf.push(escape);
                        ofs = self.scanner.ofs;
                    }
                    _ => {}
                }
            }
        } else {
            loop {
                match self.scanner.read() {
                    '\0' => return self.scanner.parse_error("unexpected EOF"),
                    '\n' => {
                        self.scanner.back();
                        break self.scanner.ofs;
                    }
                    '\r' if self.scanner.peek() == '\n' => {
                        self.scanner.back();
                        break self.scanner.ofs;
                    }
                    '$' => {
                        let end = self.scanner.ofs - 1;
                        if end > ofs {
                            self.eval_buf
                                .push(EvalPart::Literal(self.scanner.slice(ofs, end)));
                        }
                        let escape = self.read_escape()?;
                        self.eval_buf.push(escape);
                        ofs = self.scanner.ofs;
                    }
                    _ => {}
                }
            }
        };
        if end > ofs {
            self.eval_buf
                .push(EvalPart::Literal(self.scanner.slice(ofs, end)));
        }
        if self.eval_buf.is_empty() {
            return self.scanner.parse_error(format!("Expected a string"));
        }
        Ok(EvalString::new(self.eval_buf.clone()))
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

    fn skip_spaces(&mut self) {
        loop {
            match self.scanner.read() {
                ' ' => {}
                '$' => {
                    if self.scanner.peek() != '\n' {
                        self.scanner.back();
                        return;
                    }
                    self.scanner.next();
                }
                _ => {
                    self.scanner.back();
                    return;
                }
            }
        }
    }
}

pub fn split_manifest_into_chunks(buf: &[u8], num_threads: usize) -> Vec<&[u8]> {
    let min_chunk_size = 1024 * 1024;
    let chunk_count = num_threads * 2;
    let chunk_size = std::cmp::max(min_chunk_size, buf.len() / chunk_count + 1);
    let mut result = Vec::with_capacity(chunk_count);
    let mut start = 0;
    while start < buf.len() {
        let next = std::cmp::min(start + chunk_size, buf.len());
        let next = find_start_of_next_manifest_chunk(buf, next);
        result.push(&buf[start..next]);
        start = next;
    }
    result
}

fn find_start_of_next_manifest_chunk(buf: &[u8], prospective_start: usize) -> usize {
    let mut idx = prospective_start;
    loop {
        // TODO: Replace the search with something that uses SIMD instructions like the memchr crate
        let Some(nl_index) = &buf[idx..].iter().position(|&b| b == b'\n') else {
            return buf.len()
        };
        idx += nl_index + 1;

        // This newline was escaped, try again. It's possible that this check is too conservative,
        // for example, you could have:
        //  - a comment that ends with a "$": "# $\n"
        //  - an escaped-dollar: "X=$$\n"
        if idx >= 2 && buf[idx-2] == b'$' ||
            idx >= 3 && buf[idx-2] == b'\r' && buf[idx-3] == b'$' {
            continue;
        }

        // We want chunk boundaries to be at an easy/predictable place for the scanner to stop
        // at. So only stop at an identifier after a newline.
        if idx == buf.len() || matches!(
            buf[idx],
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b'.'
        ) {
            return idx;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_case_buffer(test_case: &str) -> Vec<u8> {
        let mut buf = test_case.as_bytes().to_vec();
        buf.push(0);
        buf
    }

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
            let mut buf = test_case_buffer(test_case);
            let mut parser = Parser::new(&mut buf);
            match parser.read().unwrap().unwrap() {
                Statement::VariableAssignment(_) => {},
                stmt => panic!("expected variable assignment, got {:?}", stmt),
            };
            let default = match parser.read().unwrap().unwrap() {
                Statement::Default(d) => d,
                stmt => panic!("expected default, got {:?}", stmt),
            };
            assert_eq!(
                default,
                vec![
                    EvalString::new(vec![EvalPart::Literal("a")]),
                    EvalString::new(vec![EvalPart::Literal("b"), EvalPart::VarRef("var")]),
                    EvalString::new(vec![EvalPart::Literal("c")]),
                ]
            );
        });
    }

    #[test]
    fn parse_dot_in_eval() {
        let mut buf = test_case_buffer("x = $y.z\n");
        let mut parser = Parser::new(&mut buf);
        assert_eq!(parser.read(), Ok(Some(Statement::VariableAssignment(("x",  EvalString::new(vec![
            EvalPart::VarRef("y"),
            EvalPart::Literal(".z"),
        ]))))));
    }

    #[test]
    fn parse_dot_in_rule() {
        let mut buf = test_case_buffer("rule x.y\n  command = x\n");
        let mut parser = Parser::new(&mut buf);
        let stmt = parser.read().unwrap().unwrap();
        assert!(matches!(
            stmt,
            Statement::Rule(Rule {
                name: "x.y",
                vars: _
            })
        ));
    }

    #[test]
    fn parse_trailing_newline() {
        let mut buf = test_case_buffer("build$\n foo$\n : $\n  touch $\n\n");
        let mut parser = Parser::new(&mut buf);
        let stmt = parser.read().unwrap().unwrap();
        assert!(matches!(
            stmt,
            Statement::Build(Build { rule: "touch", .. })
        ));
    }
}
