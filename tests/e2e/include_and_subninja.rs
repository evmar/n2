use crate::e2e::{n2_command, TestSpace, TOUCH_RULE};

#[cfg(unix)]
#[test]
fn include_creates_new_variable_with_dependency() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write("build.ninja", "
rule write_file
    command = echo $contents > $out

a = foo
include included.ninja
build out: write_file
    contents = $b

")?;
    space.write("included.ninja", "
b = $a bar
")?;
    space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_eq!(space.read("out").unwrap(), b"foo bar\n");
    Ok(())
}

#[cfg(unix)]
#[test]
fn include_creates_edits_existing_variable() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write("build.ninja", "
rule write_file
    command = echo $contents > $out

a = foo
include included.ninja
build out: write_file
    contents = $a

")?;
    space.write("included.ninja", "
a = $a bar
")?;
    space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_eq!(space.read("out").unwrap(), b"foo bar\n");
    Ok(())
}

#[cfg(unix)]
#[test]
fn subninja_doesnt_affect_variables_in_parent_scope() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write("build.ninja", "
rule write_file
    command = echo $contents > $out

a = foo
subninja subninja.ninja
build out: write_file
    contents = $a

")?;
    space.write("subninja.ninja", "
a = bar
build out2: write_file
    contents = $a
")?;
    space.run_expect(&mut n2_command(vec!["out", "out2"]))?;
    assert_eq!(space.read("out").unwrap(), b"foo\n");
    assert_eq!(space.read("out2").unwrap(), b"bar\n");
    Ok(())
}
