use crate::e2e::*;

#[cfg(unix)]
const GENDEP_RULE: &str = "
rule gendep
  description = gendep $out
  command = echo \"$dep_content\" > $out.d && touch $out
  depfile = $out.d
";

#[cfg(windows)]
const GENDEP_RULE: &str = "
rule gendep
  description = gendep $out
  command = cmd /c echo $dep_content > $out.d && type nul > $out
  depfile = $out.d
";

/// depfile contains invalid syntax.
#[test]
fn bad_depfile() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            GENDEP_RULE,
            "
build out: gendep
  dep_content = garbage text
",
            "",
        ]
        .join("\n"),
    )?;

    let out = space.run(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "parse error:");
    Ok(())
}

/// depfile contains reference to missing file.
#[test]
fn depfile_missing_file() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            GENDEP_RULE,
            "
build out: gendep
  dep_content = out: missing_file
",
            "",
        ]
        .join("\n"),
    )?;

    let out = space.run(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "depfile references nonexistent missing_file");
    Ok(())
}

/// depfile contains reference to existing order-only dep.
#[test]
fn discover_existing_dep() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            GENDEP_RULE,
            TOUCH_RULE,
            "build in: touch",
            "
build out: gendep || in
  dep_content = out: in
",
            "",
        ]
        .join("\n"),
    )?;

    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 2 tasks");

    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "no work");

    // Even though out only has an order-only dep on 'in' in the build file,
    // we still should rebuild it due to the discovered dep on 'in'.
    space.write("in", "x")?;
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "gendep out");

    Ok(())
}

#[cfg(unix)]
#[test]
fn multi_output_depfile() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
rule myrule
    command = echo \"out: foo\" > out.d && echo \"out2: foo2\" >> out.d && echo >> out.d && echo >> out.d && touch out out2
    depfile = out.d

build out out2: myrule
",
    )?;
    space.write("foo", "")?;
    space.write("foo2", "")?;

    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "no work");
    space.write("foo", "x")?;
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");
    space.write("foo2", "x")?;
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "no work");
    Ok(())
}

#[cfg(unix)]
#[test]
fn escaped_newline_in_depfile() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
rule myrule
    command = echo \"out: foo \\\\\" > out.d && echo \"  foo2\" >> out.d && touch out
    depfile = out.d

build out: myrule
",
    )?;
    space.write("foo", "")?;
    space.write("foo2", "")?;

    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "no work");
    space.write("foo", "x")?;
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");
    space.write("foo2", "x")?;
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "no work");
    Ok(())
}
