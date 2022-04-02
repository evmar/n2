//! Parsing of Makefile syntax as found in `.d` files emitted by C compilers.

use crate::scanner::{ParseResult, Scanner};

/// Dependency information for a single target.
#[derive(Debug)]
pub struct Deps<'a> {
    /// Output name, as found in the `.d` input.
    pub target: &'a str,
    /// Input names, as found in the `.d` input.
    pub deps: Vec<&'a str>,
}

/// Skip spaces and backslashed newlines.
fn skip_spaces(scanner: &mut Scanner) -> ParseResult<()> {
    loop {
        match scanner.read() {
            ' ' => {}
            '\\' => match scanner.read() {
                '\n' => {}
                _ => return scanner.parse_error("invalid backslash escape"),
            },
            _ => {
                scanner.back();
                break;
            }
        }
    }
    Ok(())
}

/// Read one path from the input scanner.
fn read_path<'a>(scanner: &mut Scanner<'a>) -> ParseResult<Option<&'a str>> {
    skip_spaces(scanner)?;
    let start = scanner.ofs;
    loop {
        match scanner.read() {
            '\0' | ' ' | ':' | '\n' => {
                scanner.back();
                break;
            }
            '\\' => {
                if scanner.peek() == '\n' {
                    scanner.back();
                    break;
                }
            }
            _ => {}
        }
    }
    let end = scanner.ofs;
    if end == start {
        return Ok(None);
    }
    Ok(Some(scanner.slice(start, end)))
}

/// Parse a `.d` file into `Deps`.
pub fn parse<'a>(scanner: &mut Scanner<'a>) -> ParseResult<Deps<'a>> {
    let target = match read_path(scanner)? {
        None => return scanner.parse_error("expected file"),
        Some(o) => o,
    };
    scanner.expect(':')?;
    let mut deps = Vec::new();
    while let Some(p) = read_path(scanner)? {
        deps.push(p);
    }
    scanner.skip('\n');
    scanner.skip_spaces();
    scanner.expect('\0')?;

    Ok(Deps { target, deps })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn must_parse(buf: &mut Vec<u8>) -> Deps {
        let mut scanner = Scanner::new(buf);
        match parse(&mut scanner) {
            Err(err) => {
                println!("{}", scanner.format_parse_error("test", err));
                panic!("failed parse");
            }
            Ok(d) => d,
        }
    }

    #[test]
    fn test_parse() {
        let mut file = b"build/browse.o: src/browse.cc src/browse.h build/browse_py.h\n".to_vec();
        let deps = must_parse(&mut file);
        println!("{:?}", deps);
        assert_eq!(deps.target, "build/browse.o");
        assert_eq!(deps.deps.len(), 3);
    }

    #[test]
    fn test_parse_space_suffix() {
        let mut file = b"build/browse.o: src/browse.cc   ".to_vec();
        let deps = must_parse(&mut file);
        assert_eq!(deps.target, "build/browse.o");
        assert_eq!(deps.deps.len(), 1);
    }

    #[test]
    fn test_parse_multiline() {
        let mut file = b"build/browse.o: src/browse.cc\\\n  build/browse_py.h".to_vec();
        let deps = must_parse(&mut file);
        assert_eq!(deps.target, "build/browse.o");
        assert_eq!(deps.deps.len(), 2);
    }

    #[test]
    fn test_parse_without_final_newline() {
        let mut file = b"build/browse.o: src/browse.cc".to_vec();
        let deps = must_parse(&mut file);
        assert_eq!(deps.target, "build/browse.o");
        assert_eq!(deps.deps.len(), 1);
    }
}
