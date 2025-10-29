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
    assert_stderr_contains(&out, "input in missing");

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

#[test]
fn phony_missing_file() -> anyhow::Result<()> {
    // https://ninja-build.org/manual.html#_the_literal_phony_literal_rule
    //   build foo: phony
    // means "don't fail the build if foo doesn't exist, even in inputs"

    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            TOUCH_RULE,
            "build out: touch | phony_file",
            "build phony_file: phony",
            "",
        ]
        .join("\n"),
    )?;

    // Expect the first build to generate some state...
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");
    // ...but a second one should be up to date.
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    // BUG: this should be
    // assert_output_contains(&out, "no work to do");
    // but it is currently
    assert_output_contains(&out, "ran 1 task");

    Ok(())
}

#[test]
fn phony_existing_file() -> anyhow::Result<()> {
    // Like phony_missing_file, but the file exists on disk.
    // https://github.com/evmar/n2/issues/40
    // CMake uses a phony rule targeted at a real file as a way of marking
    // "don't fail the build if this file is missing", but it had the consequence
    // of confusing our dirty-checking logic.

    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            TOUCH_RULE,
            "build out: touch | phony_file",
            "build phony_file: phony",
            "",
        ]
        .join("\n"),
    )?;

    // Despite being a target of a phony rule, the file exists on disk.
    space.write("phony_file", "")?;

    // Expect the first build to generate some state...
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");
    // ...but a second one should be up to date (#40 was that this ran again).
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "no work to do");

    Ok(())
}
