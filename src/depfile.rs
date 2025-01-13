//! Parsing of Makefile syntax as found in `.d` files emitted by C compilers.

use crate::{
    scanner::{ParseResult, Scanner},
    smallmap::SmallMap,
};

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
/// Note: treats colon as a valid character in a path because of Windows-style
/// paths, but this means that the inital `output: ...` path will include the
/// trailing colon.
fn read_path<'a>(scanner: &mut Scanner<'a>) -> ParseResult<Option<&'a str>> {
    skip_spaces(scanner)?;
    let start = scanner.ofs;
    loop {
        match scanner.read() {
            '\0' | ' ' | '\n' => {
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
pub fn parse<'a>(scanner: &mut Scanner<'a>) -> ParseResult<SmallMap<&'a str, Vec<&'a str>>> {
    let mut result = SmallMap::default();
    loop {
        while matches!(scanner.peek(), ' ' | '\n') {
            scanner.next();
        }
        let target = match read_path(scanner)? {
            None => break,
            Some(o) => o,
        };
        scanner.skip_spaces();
        let target = match target.strip_suffix(':') {
            None => {
                scanner.expect(':')?;
                target
            }
            Some(target) => target,
        };
        let mut deps = Vec::new();
        while let Some(p) = read_path(scanner)? {
            deps.push(p);
        }
        result.insert(target, deps);
    }
    scanner.expect('\0')?;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn try_parse(buf: &mut Vec<u8>) -> Result<SmallMap<&str, Vec<&str>>, String> {
        buf.push(0);
        let mut scanner = Scanner::new(buf);
        parse(&mut scanner).map_err(|err| scanner.format_parse_error(Path::new("test"), err))
    }

    fn must_parse(buf: &mut Vec<u8>) -> SmallMap<&str, Vec<&str>> {
        match try_parse(buf) {
            Err(err) => {
                println!("{}", err);
                panic!("failed parse");
            }
            Ok(d) => d,
        }
    }

    fn test_for_crlf(input: &str, test: fn(String)) {
        test(input.to_string());
        if cfg!(feature = "crlf") {
            let crlf = input.replace('\n', "\r\n");
            test(crlf);
        }
    }

    #[test]
    fn test_parse_simple() {
        test_for_crlf(
            "build/browse.o: src/browse.cc src/browse.h build/browse_py.h\n",
            |text| {
                let mut file = text.into_bytes();
                let deps = must_parse(&mut file);
                assert_eq!(
                    deps,
                    SmallMap::from([(
                        "build/browse.o",
                        vec!["src/browse.cc", "src/browse.h", "build/browse_py.h",]
                    )])
                );
            },
        );
    }

    #[test]
    fn test_parse_space_suffix() {
        test_for_crlf("build/browse.o: src/browse.cc   \n", |text| {
            let mut file = text.into_bytes();
            let deps = must_parse(&mut file);
            assert_eq!(
                deps,
                SmallMap::from([("build/browse.o", vec!["src/browse.cc",])])
            );
        });
    }

    #[test]
    fn test_parse_multiline() {
        test_for_crlf(
            "build/browse.o: src/browse.cc\\\n  build/browse_py.h",
            |text| {
                let mut file = text.into_bytes();
                let deps = must_parse(&mut file);
                assert_eq!(
                    deps,
                    SmallMap::from([(
                        "build/browse.o",
                        vec!["src/browse.cc", "build/browse_py.h",]
                    )])
                );
            },
        );
    }

    #[test]
    fn test_parse_without_final_newline() {
        let mut file = b"build/browse.o: src/browse.cc".to_vec();
        let deps = must_parse(&mut file);
        assert_eq!(
            deps,
            SmallMap::from([("build/browse.o", vec!["src/browse.cc",])])
        );
    }

    #[test]
    fn test_parse_spaces_before_colon() {
        let mut file = b"build/browse.o   : src/browse.cc".to_vec();
        let deps = must_parse(&mut file);
        assert_eq!(
            deps,
            SmallMap::from([("build/browse.o", vec!["src/browse.cc",])])
        );
    }

    #[test]
    fn test_parse_windows_dep_path() {
        let mut file = b"odd/path.o: C:/odd\\path.c".to_vec();
        let deps = must_parse(&mut file);
        assert_eq!(
            deps,
            SmallMap::from([("odd/path.o", vec!["C:/odd\\path.c",])])
        );
    }

    #[test]
    fn test_parse_multiple_targets() {
        let mut file = b"
out/a.o: src/a.c \\
  src/b.c

out/b.o :
"
        .to_vec();
        let deps = must_parse(&mut file);
        assert_eq!(
            deps,
            SmallMap::from([
                ("out/a.o", vec!["src/a.c", "src/b.c",]),
                ("out/b.o", vec![])
            ])
        );
    }

    #[test]
    fn test_parse_missing_colon() {
        let mut file = b"foo bar".to_vec();
        let err = try_parse(&mut file).unwrap_err();
        assert!(
            err.starts_with("parse error: expected ':'"),
            "expected parse error, got {:?}",
            err
        );
    }
}
