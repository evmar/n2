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
    let space = TestSpace::new()?;
    space.write(
        "build.ninja",
        "
rule build_slow
  command = sleep 0.3 && touch $out

rule build_fast
  command = sleep 0.1 && touch $out

build out: build_fast regular_input |@ validation_input
build regular_input: build_fast
build validation_input: build_slow
",
    )?;
    let command = n2_command(vec!["out"])
        .current_dir(space.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()?;
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(space.read("out").is_err());
    assert!(space.read("regular_input").is_err());
    assert!(space.read("validation_input").is_err());
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(space.read("out").is_err());
    assert!(space.read("regular_input").is_ok());
    assert!(space.read("validation_input").is_err());
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(space.read("out").is_ok());
    assert!(space.read("regular_input").is_ok());
    assert!(space.read("validation_input").is_err());
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(space.read("out").is_ok());
    assert!(space.read("regular_input").is_ok());
    assert!(space.read("validation_input").is_ok());
    assert!(command.wait_with_output()?.status.success());
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
