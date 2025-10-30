//! Tests for behavior around missing files.
use libc::{sysconf, _SC_PAGESIZE};
use std::{ffi::{OsStr, OsString}, os::unix::ffi::OsStrExt};

use super::*;

#[test]
fn test_null_termination_with_page_sized_file() -> anyhow::Result<()> {
    let page_size = unsafe { sysconf(_SC_PAGESIZE) as usize };
    let line_max_length = 128;
    let lines = page_size / line_max_length;
    let mut content = OsString::with_capacity(page_size);

    // Create a page-sized file
    content.push(
        &[
        TOUCH_RULE,
        "build out: touch | phony_file",
        "build phony_file: phony",
        ""
        ].join("\n")
    );
    let rounding_line_length = line_max_length - content.len();
    let mut fill = Vec::with_capacity(line_max_length);
    fill.extend(std::iter::repeat('#' as u8).take(rounding_line_length));
    fill[rounding_line_length - 1] = '\n' as u8;
    let rounding_line = OsString::from(unsafe { OsStr::from_encoded_bytes_unchecked(fill.as_slice()) });
    content.push(&rounding_line);

    for _ in 1..lines {
        let mut fill = Vec::with_capacity(line_max_length);
        fill.extend(std::iter::repeat('#' as u8).take(line_max_length));
        fill[line_max_length - 1] = '\n' as u8;
        let rounding_line = OsString::from(unsafe { OsStr::from_encoded_bytes_unchecked(fill.as_slice()) });
        content.push(&rounding_line);
    }

    let space = TestSpace::new()?;
    space.write_raw(
        "build.ninja",
        content.as_bytes(),
    )?;

    space.write("phony_file", "")?;

    // Should not fail with a SIGBUS
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");

    Ok(())
}
