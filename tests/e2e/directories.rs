use crate::e2e::*;

#[cfg(unix)]
#[test]
fn dep_on_current_directory() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
rule list_files
    command = ls $in > $out

build out: list_files .
",
    )?;
    space.write("foo", "")?;

    let out = space.run_expect(&mut n2_command(vec!["-d", "explain", "out"]))?;
    assert_output_contains(&out, "ran 1 task");
    assert_eq!(space.read("out")?, b"build.ninja\nfoo\nout\n");

    let out = space.run_expect(&mut n2_command(vec!["-d", "explain", "out"]))?;
    assert_output_contains(&out, "no work to do");

    // Expect: writing a file modifies the current directory's mtime, triggering a build.
    space.write("foo2", "")?;
    let out = space.run_expect(&mut n2_command(vec!["-d", "explain", "out"]))?;
    assert_output_contains(&out, "ran 1 task");
    assert_eq!(space.read("out")?, b"build.ninja\nfoo\nfoo2\nout\n");

    Ok(())
}
