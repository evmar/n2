use crate::scanner::{ParseResult, Scanner};

#[derive(Debug)]
pub struct Deps<'a> {
    pub target: &'a str,
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

fn read_path<'a>(scanner: &mut Scanner<'a>) -> ParseResult<Option<&'a str>> {
    skip_spaces(scanner)?;
    let start = scanner.ofs;
    loop {
        match scanner.read() {
            '\0' | ' ' | ':' | '\n' => {
                scanner.back();
                break;
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

pub fn parse<'a>(scanner: &mut Scanner<'a>) -> ParseResult<Deps<'a>> {
    let target = match read_path(scanner)? {
        None => return scanner.parse_error("expected file"),
        Some(o) => o,
    };
    scanner.expect(':')?;
    let mut deps = Vec::new();
    loop {
        match read_path(scanner)? {
            None => break,
            Some(p) => deps.push(p),
        }
    }
    scanner.expect('\n')?;
    scanner.expect('\0')?;

    Ok(Deps {
        target: target,
        deps: deps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_parse() {
        let mut scanner =
            Scanner::new("build/browse.o: src/browse.cc src/browse.h build/browse_py.h\n\0");
        let deps = match parse(&mut scanner) {
            Err(err) => {
                println!("{}", scanner.format_parse_error(err));
                panic!("failed parse");
            }
            Ok(d) => d,
        };
        println!("{:?}", deps);
        assert_eq!(deps.target, "build/browse.o");
        assert_eq!(deps.deps.len(), 3);
    }
}
