//! Parser for .ninja files.
//!
//! See design notes on parsing in doc/design_notes.md.
//!
//! To avoid allocations parsing frequently uses references into the input
//! text, marked with the lifetime `'text`.

use crate::{
    eval::{EvalPart, EvalString}, graph::{Build, BuildId, BuildIns, BuildOuts, FileLoc}, load::{Scope, ScopePosition}, scanner::{ParseError, ParseResult, Scanner}, smallmap::SmallMap
};
use std::{
    cell::UnsafeCell,
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc, Mutex},
};

/// A list of variable bindings, as expressed with syntax like:
///   key = $val
pub type VarList<'text> = SmallMap<&'text str, EvalString<&'text str>>;

#[derive(Debug, PartialEq)]
pub struct Rule<'text> {
    pub name: &'text str,
    pub vars: VarList<'text>,
    pub scope_position: ScopePosition,
}

// #[derive(Debug, PartialEq)]
// pub struct Build<'text> {
//     pub rule: &'text str,
//     pub line: usize,
//     pub outs: Vec<EvalString<&'text str>>,
//     pub explicit_outs: usize,
//     pub ins: Vec<EvalString<&'text str>>,
//     pub explicit_ins: usize,
//     pub implicit_ins: usize,
//     pub order_only_ins: usize,
//     pub validation_ins: usize,
//     pub vars: VarList<'text>,
//     pub scope_position: ScopePosition,
// }

#[derive(Debug, PartialEq)]
pub struct Pool<'text> {
    pub name: &'text str,
    pub depth: usize,
}

#[derive(Debug)]
pub struct VariableAssignment<'text> {
    pub name: &'text str,
    pub unevaluated: EvalString<&'text str>,
    pub scope_position: ScopePosition,
    pub evaluated: UnsafeCell<Option<String>>,
    pub is_evaluated: AtomicBool,
    pub lock: Mutex<()>,
}

unsafe impl Sync for VariableAssignment<'_> {}

impl<'text> VariableAssignment<'text> {
    fn new(name: &'text str, unevaluated: EvalString<&'text str>) -> Self {
        Self {
            name,
            unevaluated,
            scope_position: ScopePosition(0),
            evaluated: UnsafeCell::new(None),
            is_evaluated: AtomicBool::new(false),
            lock: Mutex::new(()),
        }
    }

    pub fn evaluate(&self, result: &mut String, scope: &Scope) {
        unsafe {
            if self.is_evaluated.load(std::sync::atomic::Ordering::Relaxed) {
                result.push_str((*self.evaluated.get()).as_ref().unwrap());
                return;
            }
            let guard = self.lock.lock().unwrap();
            if self.is_evaluated.load(std::sync::atomic::Ordering::Relaxed) {
                result.push_str((*self.evaluated.get()).as_ref().unwrap());
                return;
            }

            let cache = self.unevaluated.evaluate(&[], scope, self.scope_position);
            result.push_str(&cache);
            *self.evaluated.get() = Some(cache);
            self.is_evaluated
                .store(true, std::sync::atomic::Ordering::Relaxed);

            drop(guard);
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct DefaultStmt<'text> {
    pub files: Vec<EvalString<&'text str>>,
    pub scope_position: ScopePosition,
}

#[derive(Debug, PartialEq)]
pub struct IncludeOrSubninja<'text> {
    pub file: EvalString<&'text str>,
    pub scope_position: ScopePosition,
}

#[derive(Debug)]
pub enum Statement<'text> {
    EmptyStatement,
    Rule(Rule<'text>),
    Build(Build),
    Default(DefaultStmt<'text>),
    Include(IncludeOrSubninja<'text>),
    Subninja(IncludeOrSubninja<'text>),
    Pool(Pool<'text>),
    VariableAssignment(VariableAssignment<'text>),
}

#[derive(Default, Debug)]
pub struct Clump<'text> {
    pub assignments: Vec<VariableAssignment<'text>>,
    pub rules: Vec<Rule<'text>>,
    pub pools: Vec<Pool<'text>>,
    pub defaults: Vec<DefaultStmt<'text>>,
    pub builds: Vec<Build>,
    pub subninjas: Vec<IncludeOrSubninja<'text>>,
    pub used_scope_positions: usize,
    pub base_position: ScopePosition,
}

impl<'text> Clump<'text> {
    pub fn is_empty(&self) -> bool {
        self.assignments.is_empty() &&
            self.rules.is_empty() &&
            self.pools.is_empty() &&
            self.defaults.is_empty() &&
            self.builds.is_empty() &&
            self.subninjas.is_empty()
    }
}

#[derive(Debug)]
pub enum ClumpOrInclude<'text> {
    Clump(Clump<'text>),
    Include(EvalString<&'text str>),
}

pub struct Parser<'text> {
    filename: Arc<PathBuf>,
    scanner: Scanner<'text>,
    buf_len: usize,
    /// Reading EvalStrings is very hot when parsing, so we always read into
    /// this buffer and then clone it afterwards.
    eval_buf: Vec<EvalPart<&'text str>>,
}

impl<'text> Parser<'text> {
    pub fn new(buf: &'text [u8], filename: Arc<PathBuf>) -> Parser<'text> {
        Parser {
            filename,
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

    pub fn read_clumps(&mut self) -> ParseResult<Vec<ClumpOrInclude<'text>>> {
        let mut result = Vec::new();
        let mut clump = Clump::default();
        let mut position = ScopePosition(0);
        while let Some(stmt) = self.read()? {
            match stmt {
                Statement::EmptyStatement => {},
                Statement::Rule(mut r) => {
                    r.scope_position = position;
                    position.0 += 1;
                    clump.rules.push(r);
                },
                Statement::Build(mut b) => {
                    b.scope_position = position;
                    position.0 += 1;
                    clump.builds.push(b);
                },
                Statement::Default(mut d) => {
                    d.scope_position = position;
                    position.0 += 1;
                    clump.defaults.push(d);
                },
                Statement::Include(i) => {
                    if !clump.is_empty() {
                        clump.used_scope_positions = position.0;
                        result.push(ClumpOrInclude::Clump(clump));
                        clump = Clump::default();
                        position = ScopePosition(0);
                    }
                    result.push(ClumpOrInclude::Include(i.file));
                },
                Statement::Subninja(mut s) => {
                    s.scope_position = position;
                    position.0 += 1;
                    clump.subninjas.push(s);
                },
                Statement::Pool(p) => {
                    clump.pools.push(p);
                },
                Statement::VariableAssignment(mut v) => {
                    v.scope_position = position;
                    position.0 += 1;
                    clump.assignments.push(v);
                },
            }
        }
        if !clump.is_empty() {
            clump.used_scope_positions = position.0;
            result.push(ClumpOrInclude::Clump(clump));
        }
        Ok(result)
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
                        return Ok(None);
                    }
                    let ident = self.read_ident()?;
                    self.skip_spaces();
                    match ident {
                        "rule" => return Ok(Some(Statement::Rule(self.read_rule()?))),
                        "build" => return Ok(Some(Statement::Build(self.read_build()?))),
                        "default" => return Ok(Some(Statement::Default(self.read_default()?))),
                        "include" => {
                            let result = IncludeOrSubninja {
                                file: self.read_eval(false)?,
                                scope_position: ScopePosition(0),
                            };
                            return Ok(Some(Statement::Include(result)));
                        }
                        "subninja" => {
                            let result = IncludeOrSubninja {
                                file: self.read_eval(false)?,
                                scope_position: ScopePosition(0),
                            };
                            return Ok(Some(Statement::Subninja(result)));
                        }
                        "pool" => return Ok(Some(Statement::Pool(self.read_pool()?))),
                        ident => {
                            let result = VariableAssignment::new(ident, self.read_vardef()?);
                            return Ok(Some(Statement::VariableAssignment(result)));
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
        Ok(Rule {
            name,
            vars,
            scope_position: ScopePosition(0),
        })
    }

    fn read_pool(&mut self) -> ParseResult<Pool<'text>> {
        let name = self.read_ident()?;
        self.scanner.skip('\r');
        self.scanner.expect('\n')?;
        let vars = self.read_scoped_vars(|var| matches!(var, "depth"))?;
        let mut depth = 0;
        if let Some((_, val)) = vars.into_iter().next() {
            match val.maybe_literal() {
                Some(x) => match x.parse::<usize>() {
                    Ok(d) => depth = d,
                    Err(err) => return self.scanner.parse_error(format!("pool depth: {}", err)),
                },
                None => {
                    return self
                        .scanner
                        .parse_error(format!("pool depth must be a literal string"))
                }
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

    fn read_owned_unevaluated_paths_to(
        &mut self,
        v: &mut Vec<EvalString<String>>,
    ) -> ParseResult<()> {
        self.skip_spaces();
        while self.scanner.peek() != ':'
            && self.scanner.peek() != '|'
            && !self.scanner.peek_newline()
        {
            v.push(self.read_eval(true)?.into_owned());
            self.skip_spaces();
        }
        Ok(())
    }

    fn read_build(&mut self) -> ParseResult<Build> {
        let line = self.scanner.line;
        let mut outs = Vec::new();
        self.read_owned_unevaluated_paths_to(&mut outs)?;
        let explicit_outs = outs.len();

        if self.scanner.peek() == '|' {
            self.scanner.next();
            self.read_owned_unevaluated_paths_to(&mut outs)?;
        }

        self.scanner.expect(':')?;
        self.skip_spaces();
        let rule = self.read_ident()?;

        let mut ins = Vec::new();
        self.read_owned_unevaluated_paths_to(&mut ins)?;
        let explicit_ins = ins.len();

        if self.scanner.peek() == '|' {
            self.scanner.next();
            let peek = self.scanner.peek();
            if peek == '|' || peek == '@' {
                self.scanner.back();
            } else {
                self.read_owned_unevaluated_paths_to(&mut ins)?;
            }
        }
        let implicit_ins = ins.len() - explicit_ins;

        if self.scanner.peek() == '|' {
            self.scanner.next();
            if self.scanner.peek() == '@' {
                self.scanner.back();
            } else {
                self.scanner.expect('|')?;
                self.read_owned_unevaluated_paths_to(&mut ins)?;
            }
        }
        let order_only_ins = ins.len() - implicit_ins - explicit_ins;

        if self.scanner.peek() == '|' {
            self.scanner.next();
            self.scanner.expect('@')?;
            self.read_owned_unevaluated_paths_to(&mut ins)?;
        }
        let validation_ins = ins.len() - order_only_ins - implicit_ins - explicit_ins;

        self.scanner.skip('\r');
        self.scanner.expect('\n')?;
        let vars = self.read_scoped_vars(|_| true)?;

        Ok(Build {
            id: BuildId::from(0),
            rule: rule.to_owned(),
            scope_position: ScopePosition(0),
            bindings: vars.to_owned(),
            location: FileLoc {
                filename: self.filename.clone(),
                line,
            },
            desc: None,
            cmdline: None,
            depfile: None,
            parse_showincludes: false,
            rspfile: None,
            pool: None,
            ins: BuildIns {
                ids: Vec::new(),
                unevaluated: ins,
                explicit: explicit_ins,
                implicit: implicit_ins,
                order_only: order_only_ins,
            },
            discovered_ins: Vec::new(),
            outs: BuildOuts {
                ids: Vec::new(),
                unevaluated: outs,
                explicit: explicit_outs,
            },
        })
    }

    fn read_default(&mut self) -> ParseResult<DefaultStmt<'text>> {
        let mut files = Vec::new();
        self.read_unevaluated_paths_to(&mut files)?;
        if files.is_empty() {
            return self.scanner.parse_error("expected path");
        }
        self.scanner.skip('\r');
        self.scanner.expect('\n')?;
        Ok(DefaultStmt {
            files,
            scope_position: ScopePosition(0),
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
            return buf.len();
        };
        idx += nl_index + 1;

        // This newline was escaped, try again. It's possible that this check is too conservative,
        // for example, you could have:
        //  - a comment that ends with a "$": "# $\n"
        //  - an escaped-dollar: "X=$$\n"
        if idx >= 2 && buf[idx - 2] == b'$'
            || idx >= 3 && buf[idx - 2] == b'\r' && buf[idx - 3] == b'$'
        {
            continue;
        }

        // We want chunk boundaries to be at an easy/predictable place for the scanner to stop
        // at. So only stop at an identifier after a newline.
        if idx == buf.len()
            || matches!(
                buf[idx],
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b'.'
            )
        {
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
            let mut parser = Parser::new(&mut buf, Arc::new(PathBuf::from("build.ninja")));
            match parser.read().unwrap().unwrap() {
                Statement::VariableAssignment(_) => {}
                stmt => panic!("expected variable assignment, got {:?}", stmt),
            };
            let default = match parser.read().unwrap().unwrap() {
                Statement::Default(d) => d.files,
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
        let mut parser = Parser::new(&mut buf, Arc::new(PathBuf::from("build.ninja")));
        let Ok(Some(Statement::VariableAssignment(x))) = parser.read() else {
            panic!("Fail");
        };
        assert_eq!(x.name, "x");
        assert_eq!(
            x.unevaluated,
            EvalString::new(vec![EvalPart::VarRef("y"), EvalPart::Literal(".z"),])
        );
    }

    #[test]
    fn parse_dot_in_rule() {
        let mut buf = test_case_buffer("rule x.y\n  command = x\n");
        let mut parser = Parser::new(&mut buf, Arc::new(PathBuf::from("build.ninja")));
        let Ok(Some(Statement::Rule(stmt))) = parser.read() else {
            panic!("Fail");
        };
        assert_eq!(stmt.name, "x.y");
        assert_eq!(stmt.vars.len(), 1);
        assert_eq!(
            stmt.vars.get("command"),
            Some(&EvalString::new(vec![EvalPart::Literal("x"),]))
        );
    }

    #[test]
    fn parse_trailing_newline() {
        let mut buf = test_case_buffer("build$\n foo$\n : $\n  touch $\n\n");
        let mut parser = Parser::new(&mut buf, Arc::new(PathBuf::from("build.ninja")));
        let stmt = parser.read().unwrap().unwrap();
        let rule = "touch".to_owned();
        assert!(matches!(
            stmt,
            Statement::Build(Build { rule, .. })
        ));
    }
}
