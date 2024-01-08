//! Tests for the 'validations' feature, which are build edges marked with |@.

use crate::e2e::*;

#[test]
fn basic_validation() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            TOUCH_RULE,
            "build my_validation: touch",
            "build out: touch |@ my_validation",
            "",
        ]
        .join("\n"),
    )?;
    space.run_expect(&mut n2_command(vec!["out"]))?;
    assert!(space.read("out").is_ok());
    assert!(space.read("my_validation").is_ok());
    Ok(())
}

#[cfg(unix)]
#[test]
fn build_starts_before_validation_finishes() -> anyhow::Result<()> {
    // When a given target has a validation, that validation is part of the
    // overall build.  But despite there being a build edge, the target shouldn't
    // wait for the validation.
    // To verify this, we make the validation command internally wait for the
    // target, effectively reversing the dependency order at runtime.
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
# Waits for the file $wait_for to exist, then touches $out.
rule build_slow
  command = until [ -f $wait_for ]; do sleep 0.1; done; touch $out

rule build_fast
  command = touch $out

build out: build_fast regular_input |@ validation_input
build regular_input: build_fast
build validation_input: build_slow
  wait_for = out
",
    )?;
    space.run_expect(&mut n2_command(vec!["out"]))?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn build_fails_when_validation_fails() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
rule touch
  command = touch $out

rule fail
  command = exit 1

build out: touch |@ validation_input
build validation_input: fail
",
    )?;
    let output = space.run(&mut n2_command(vec!["out"]))?;
    assert!(!output.status.success());
    Ok(())
}

#[test]
fn validation_inputs_break_cycles() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            TOUCH_RULE,
            "build out: touch |@ validation_input",
            "build validation_input: touch out",
            "",
        ]
        .join("\n"),
    )?;
    space.run_expect(&mut n2_command(vec!["out"]))?;
    Ok(())
}
