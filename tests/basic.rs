//! Integration test.  Runs n2 binary against a temp directory.

mod e2e;

use crate::e2e::*;

#[test]
fn empty_file() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write("build.ninja", "")?;
    let out = space.run(&mut n2_command(vec![]))?;
    assert_eq!(
        std::str::from_utf8(&out.stdout)?,
        "n2: error: no path specified and no default\n"
    );
    Ok(())
}

#[test]
fn basic_build() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[TOUCH_RULE, "build out: touch in", ""].join("\n"),
    )?;
    space.write("in", "")?;
    space.run_expect(&mut n2_command(vec!["out"]))?;
    assert!(space.read("out").is_ok());

    Ok(())
}

#[test]
fn create_subdir() -> anyhow::Result<()> {
    // Run a build rule that needs a subdir to be automatically created.
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[TOUCH_RULE, "build subdir/out: touch in", ""].join("\n"),
    )?;
    space.write("in", "")?;
    space.run_expect(&mut n2_command(vec!["subdir/out"]))?;
    assert!(space.read("subdir/out").is_ok());

    Ok(())
}

#[cfg(unix)]
#[test]
fn generate_build_file() -> anyhow::Result<()> {
    // Run a project where a build rule generates the build.ninja.
    let space = TestSpace::new()?;
    space.write(
        "gen.sh",
        "
cat >build.ninja <<EOT
rule regen
  command = sh ./gen.sh
  generator = 1
build build.ninja: regen gen.sh
rule touch
  command = touch \\$out
build out: touch
EOT
",
    )?;

    // Generate the initial build.ninja.
    space.run_expect(std::process::Command::new("sh").args(vec!["./gen.sh"]))?;

    // Run: expect to regenerate because we don't know how the file was made.
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");

    // Run: everything should be up to date.
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "no work");

    Ok(())
}

#[cfg(unix)]
#[test]
fn generate_specified_build_file() -> anyhow::Result<()> {
    // Run a project where a build rule generates specified_build.ninja.
    let space = TestSpace::new()?;
    space.write(
        "gen.sh",
        "
cat >specified_build.ninja <<EOT
rule regen
  command = sh ./gen.sh
  generator = 1
build specified_build.ninja: regen gen.sh
rule touch
  command = touch \\$out
build out: touch
EOT
",
    )?;

    // Generate the initial specified_build.ninja.
    space.run_expect(std::process::Command::new("sh").args(vec!["./gen.sh"]))?;

    // Run: expect to regenerate because we don't know how the file was made.
    let out = space.run_expect(&mut n2_command(vec!["-f", "specified_build.ninja", "out"]))?;
    assert_output_contains(&out, "ran 1 task");

    // Run: everything should be up to date.
    let out = space.run_expect(&mut n2_command(vec!["-f", "specified_build.ninja", "out"]))?;
    assert_output_contains(&out, "no work");

    Ok(())
}

#[cfg(unix)]
#[test]
fn generate_rsp_file() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
rule cat
  command = cat ${out}.rsp > ${out}
  rspfile = ${out}.rsp
  rspfile_content = 1 $in 2 $in_newline 3

rule litter
  command = cat make/me/${out}.rsp > ${out}
  rspfile = make/me/${out}.rsp
  rspfile_content = random stuff

rule touch
  command = touch $out

build main: cat foo bar baz in
build foo: litter bar
build bar: touch baz
build baz: touch in
",
    )?;
    space.write("in", "go!")?;

    let _ = space.run_expect(&mut n2_command(vec!["main"]))?;

    // The 'main' and 'foo' targets copy the contents of their rsp file to their
    // output.
    let main_rsp = space.read("main").unwrap();
    assert_eq!(main_rsp, b"1 foo bar baz in 2 foo\nbar\nbaz\nin 3");
    let foo_rsp = space.read("foo").unwrap();
    assert_eq!(foo_rsp, b"random stuff");

    // The 'make/me' directory was created when writing an rsp file.
    // It should still be there.
    let meta = space.metadata("make/me").unwrap();
    assert!(meta.is_dir());

    // Run again: everything should be up to date.
    let out = space.run_expect(&mut n2_command(vec!["main"]))?;
    assert_output_contains(&out, "no work");

    Ok(())
}

/// Run a task that prints something, and verify it shows up.
#[cfg(unix)]
#[test]
fn spam_output() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
rule quiet
  description = quiet $out
  command = touch $out
rule spam
  description = spam $out
  command = echo greetz from $out && touch $out
build a: quiet
build b: spam a
build c: quiet b
",
    )?;
    let out = space.run_expect(&mut n2_command(vec!["c"]))?;
    assert_output_contains(
        &out,
        "quiet a
spam b
greetz from b
quiet c
",
    );
    Ok(())
}

#[test]
fn basic_specify_build_file() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build_specified.ninja",
        &[TOUCH_RULE, "build out: touch in", ""].join("\n"),
    )?;
    space.write("in", "")?;
    space.run_expect(&mut n2_command(vec!["-f", "build_specified.ninja", "out"]))?;
    assert!(space.read("out").is_ok());

    Ok(())
}

/// Regression test for https://github.com/evmar/n2/issues/44
/// and https://github.com/evmar/n2/issues/46 .
/// Build with the same output listed multiple times.
#[test]
fn repeated_out() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            TOUCH_RULE,
            "build dup dup: touch in",
            "build out: touch dup",
            "",
        ]
        .join("\n"),
    )?;
    space.write("in", "")?;
    space.write("dup", "")?;
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "is repeated in output list");

    Ok(())
}

/// Regression test for https://github.com/evmar/n2/issues/55
/// UTF-8 filename.
#[cfg(unix)]
#[test]
fn utf8_filename() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            "
rule echo
  description = unicode variable: $in
  command = echo unicode command line: $in && touch $out
",
            "build out: echo reykjavík.md",
            "",
        ]
        .join("\n"),
    )?;
    space.write("reykjavík.md", "")?;
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "unicode variable: reykjavík.md");
    assert_output_contains(&out, "unicode command line: reykjavík.md");

    Ok(())
}

#[test]
fn explain() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[TOUCH_RULE, "build out: touch in", ""].join("\n"),
    )?;
    space.write("in", "")?;
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "up to date");

    space.write("in", "")?;
    let out = space.run_expect(&mut n2_command(vec!["-d", "explain", "out"]))?;
    // The main "explain" log line:
    assert_output_contains(&out, "explain: build.ninja:5: input changed");
    // The dump of the file manifest after includes mtimes that we don't want
    // to be sensitive to, so just look for some bits we know show up there.
    assert_output_contains(&out, "discovered:");
    assert_output_contains(&out, "cmdline: touch out\n");

    Ok(())
}
