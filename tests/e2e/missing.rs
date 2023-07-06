//! Tests for behavior around missing files.

use super::*;

#[test]
fn missing_input() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[TOUCH_RULE, "build out: touch in", ""].join("\n"),
    )?;

    let out = space.run(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "input in missing");

    Ok(())
}

#[test]
fn missing_generated() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            TOUCH_RULE,
            "rule echo
  command = echo $out
",
            "build mid: echo",      // never writes output
            "build out: touch mid", // uses never-written output
            "",
        ]
        .join("\n"),
    )?;

    let out = space.run(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "input mid missing");

    // TODO: we'll need to revisit this behavior to fix
    // https://github.com/evmar/n2/issues/69

    Ok(())
}
