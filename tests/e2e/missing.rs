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
            ECHO_RULE,
            "build mid: echo",      // never writes output
            "build out: touch mid", // uses never-written output
            "",
        ]
        .join("\n"),
    )?;

    // https://github.com/evmar/n2/issues/69

    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "echo mid");
    assert_output_contains(&out, "touch out");

    Ok(())
}

#[test]
fn missing_phony() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            TOUCH_RULE,
            "build order_only: phony",        // never writes output
            "build out: touch || order_only", // uses never-written output
            "",
        ]
        .join("\n"),
    )?;

    // https://github.com/evmar/n2/issues/69

    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "touch out");

    Ok(())
}

// Ensure we don't regress on
// https://github.com/ninja-build/ninja/issues/1779
// I can't remember the specific code CMake generates that relies on this;
// I wonder if we can tighten the behavior at all.
#[test]
fn missing_phony_input() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[TOUCH_RULE, "build out: phony || no_such_file", ""].join("\n"),
    )?;

    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "no work to do");

    Ok(())
}
