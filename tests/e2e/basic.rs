use crate::e2e::*;

#[test]
fn empty_file() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write("build.ninja", "")?;
    let out = space.run(&mut n2_command(vec![]))?;
    assert_eq!(std::str::from_utf8(&out.stdout)?, "n2: no work to do\n");
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
fn specify_build_file() -> anyhow::Result<()> {
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
    assert_output_contains(&out, "explain: build.ninja:6: manifest changed");
    // The dump of the file manifest after includes mtimes that we don't want
    // to be sensitive to, so just look for some bits we know show up there.
    assert_output_contains(&out, "discovered:");

    Ok(())
}

/// Meson generates a build step that writes to one of its inputs.
#[test]
fn write_to_input() -> anyhow::Result<()> {
    #[cfg(unix)]
    let touch_input_command = "touch out in";
    #[cfg(windows)]
    let touch_input_command = "cmd /c type nul > in && cmd /c type nul > out";
    let touch_input_rule = format!(
        "
rule touch_in
  description = touch out+in
  command = {}
",
        touch_input_command
    );

    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[&touch_input_rule, "build out: touch_in in", ""].join("\n"),
    )?;
    space.write("in", "")?;
    space.sub_mtime("in", std::time::Duration::from_secs(1))?;

    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");

    // TODO: to support meson, we need this second invocation to not build anything.
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");

    Ok(())
}

#[test]
fn showincludes() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            ECHO_RULE,
            "
build out: echo
  text = Note: including file: foo
  deps = msvc
",
        ]
        .join("\n"),
    )?;
    space.write("foo", "")?;

    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");

    space.write("foo", "")?;
    let out = space.run_expect(&mut n2_command(vec!["out"]))?;
    assert_output_contains(&out, "ran 1 task");

    Ok(())
}

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

// Repro for issue #84: phony depending on phony.
#[test]
fn phony_depends() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            TOUCH_RULE,
            "
build out1: touch
build out2: phony out1
build out3: phony out2
",
        ]
        .join("\n"),
    )?;
    space.run_expect(&mut n2_command(vec!["out3"]))?;
    space.read("out1")?;
    Ok(())
}

// builddir controls where .n2_db is written.
#[test]
fn builddir() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        &[
            "builddir = foo",
            TOUCH_RULE,
            "build $builddir/bar: touch",
            "",
        ]
        .join("\n"),
    )?;
    space.run_expect(&mut n2_command(vec!["foo/bar"]))?;
    space.read("foo/.n2_db")?;
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
    assert_output_contains(&out, "unexpected variable \"my_var\"");
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
fn looks_up_values_from_rule() -> anyhow::Result<()> {
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
rule copy_rspfile
    command = cp $rspfile $out
    rspfile = $out.rsp
    rspfile_content = Hello, world!

build foo: copy_rspfile
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
