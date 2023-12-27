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
                '\r' => scanner.expect('\n')?,
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
            '\0' | ' ' | ':' | '\r' | '\n' => {
                scanner.back();
                break;
            }
            '\\' => {
                let peek = scanner.peek();
                if peek == '\n' || peek == '\r' {
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
    scanner.skip_spaces();
    scanner.expect(':')?;
    let mut deps = Vec::new();
    while let Some(p) = read_path(scanner)? {
        deps.push(p);
    }
    scanner.skip('\r');
    scanner.skip('\n');
    scanner.skip_spaces();
    scanner.expect('\0')?;

    Ok(Deps { target, deps })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn must_parse(buf: &mut Vec<u8>) -> Deps {
        buf.push(0);
        let mut scanner = Scanner::new(buf);
        match parse(&mut scanner) {
            Err(err) => {
                println!("{}", scanner.format_parse_error(Path::new("test"), err));
                panic!("failed parse");
            }
            Ok(d) => d,
        }
    }

    fn test_for_crlf(input: &str, test: fn(String)) {
        let crlf = input.replace('\n', "\r\n");
        for test_case in [String::from(input), crlf] {
            test(test_case);
        }
    }

    #[test]
    fn test_parse() {
        test_for_crlf(
            "build/browse.o: src/browse.cc src/browse.h build/browse_py.h\n",
            |text| {
                let mut file = text.into_bytes();
                let deps = must_parse(&mut file);
                assert_eq!(deps.target, "build/browse.o");
                assert_eq!(deps.deps.len(), 3);
            },
        );
    }

    #[test]
    fn test_parse_space_suffix() {
        test_for_crlf("build/browse.o: src/browse.cc   \n", |text| {
            let mut file = text.into_bytes();
            let deps = must_parse(&mut file);
            assert_eq!(deps.target, "build/browse.o");
            assert_eq!(deps.deps.len(), 1);
        });
    }

    #[test]
    fn test_parse_multiline() {
        test_for_crlf(
            "build/browse.o: src/browse.cc\\\n  build/browse_py.h",
            |text| {
                let mut file = text.into_bytes();
                let deps = must_parse(&mut file);
                assert_eq!(deps.target, "build/browse.o");
                assert_eq!(deps.deps.len(), 2);
            },
        );
    }

    #[test]
    fn test_parse_without_final_newline() {
        let mut file = b"build/browse.o: src/browse.cc".to_vec();
        let deps = must_parse(&mut file);
        assert_eq!(deps.target, "build/browse.o");
        assert_eq!(deps.deps.len(), 1);
    }

    #[test]
    fn test_parse_spaces_before_colon() {
        let mut file = b"build/browse.o   : src/browse.cc".to_vec();
        let deps = must_parse(&mut file);
        assert_eq!(deps.target, "build/browse.o");
        assert_eq!(deps.deps.len(), 1);
    }
}
