//! Tests for behavior around variable bindings.

use super::*;

// Repro for issue #83.
#[cfg(unix)]
#[test]
fn eval_twice() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            TOUCH_RULE,
            "
var = 123
rule custom
  command = $cmd $var
build out: custom
  cmd = echo $var hello
",
        ]
        .join("\n"),
    )?;

    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "echo 123 hello 123");
    Ok(())
}

#[test]
fn bad_rule_variable() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
rule my_rule
    command = touch $out
    my_var = foo

build out: my_rule
",
    )?;

    let out = space.run(&mut n2_command(vec!["out"]))?;
    assert_stderr_contains(&out, "unexpected variable \"my_var\"");
    Ok(())
}

#[cfg(unix)]
#[test]
fn deps_evaluate_build_bindings() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
rule touch
    command = touch $out
rule copy
    command = cp $in $out
build foo: copy ${my_dep}
    my_dep = bar
build bar: touch
",
    )?;
    space.run_expect(&mut n2_command(vec!["foo"]))?;
    space.read("foo")?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn looks_up_values_from_build() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
rule copy_rspfile
    command = cp $out.rsp $out
    rspfile = $out.rsp

build foo: copy_rspfile
    rspfile_content = Hello, world!
",
    )?;
    space.run_expect(&mut n2_command(vec!["foo"]))?;
    assert_eq!(space.read("foo")?, b"Hello, world!");
    Ok(())
}

#[cfg(unix)]
#[test]
fn build_bindings_arent_recursive() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
rule write_file
    command = echo $my_var > $out

build foo: write_file
    my_var = Hello,$my_var_2 world!
    my_var_2 = my_var_2_value
",
    )?;
    space.run_expect(&mut n2_command(vec!["foo"]))?;
    assert_eq!(space.read("foo")?, b"Hello, world!\n");
    Ok(())
}

#[cfg(unix)]
#[test]
fn empty_variable_binding() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
empty_var =

rule write_file
    command = echo $my_var > $out

build foo: write_file
    my_var = Hello,$empty_var world!
",
    )?;
    space.run_expect(&mut n2_command(vec!["foo"]))?;
    assert_eq!(space.read("foo")?, b"Hello, world!\n");
    Ok(())
}

#[cfg(unix)]
#[test]
fn empty_build_variable() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
rule write_file
    command = echo $my_var > $out

build foo: write_file
    empty =
    my_var = Hello, world!
",
    )?;
    space.run_expect(&mut n2_command(vec!["foo"]))?;
    assert_eq!(space.read("foo")?, b"Hello, world!\n");
    Ok(())
}

// Test for https://github.com/evmar/n2/issues/145: a variable in one file is visible
// in an included file.
#[test]
fn across_files() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write("world.txt", "<t>")?;
    space.write(
        "build.ninja",
        &[
            ECHO_RULE,
            "
ext = txt
include other.ninja
",
        ]
        .join("\n"),
    )?;
    space.write(
        "other.ninja",
        "
build hello: echo world.$ext
    text = what a beautiful day
",
    )?;

    let out = space.run_expect(&mut n2_command(vec!["hello"]))?;
    assert_output_contains(&out, "what a beautiful day");

    Ok(())
}
