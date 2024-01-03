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
    space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_eq!(space.read("out")?, b"build.ninja\nfoo\nout\n");
    space.write("foo2", "")?;
    space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_eq!(space.read("out")?, b"build.ninja\nfoo\nfoo2\nout\n");

    Ok(())
}
