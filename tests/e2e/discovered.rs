use crate::e2e::*;

const GENDEP_RULE: &str = "
rule gendep
  description = gendep $out
  command = echo \"$dep_content\" > $out.d && touch $out
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
