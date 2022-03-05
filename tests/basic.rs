//! Integration test.  Runs n2 binary against a temp directory.

fn n2_binary() -> std::path::PathBuf {
    std::env::current_exe()
        .expect("test binary path")
        .parent()
        .expect("test binary directory")
        .parent()
        .expect("binary directory")
        .join("n2")
        .to_path_buf()
}

fn n2_command(args: Vec<&str>) -> std::process::Command {
    let mut cmd = std::process::Command::new(n2_binary());
    cmd.args(args);
    cmd
}

fn print_output(out: &std::process::Output) {
    // Gross: use print! instead of writing to stdout so Rust test
    // framework can capture it.
    print!("{}", std::str::from_utf8(&out.stdout).unwrap());
}

fn assert_output_contains(out: &std::process::Output, text: &str) {
    let out = std::str::from_utf8(&out.stdout).unwrap();
    if !out.contains(text) {
        panic!("assertion failed; expected output to contain {:?}", text);
    }
}

/// Manages a temporary directory for invoking n2.
struct TestSpace {
    dir: tempfile::TempDir,
}
impl TestSpace {
    fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        Ok(TestSpace { dir })
    }

    /// Write a file into the working space.
    fn write(&self, path: &str, content: &str) -> std::io::Result<()> {
        std::fs::write(self.dir.path().join(path), content)
    }

    /// Read a file from the working space.
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(self.dir.path().join(path))
    }

    /// Invoke n2, returning process output.
    fn run(&self, cmd: &mut std::process::Command) -> std::io::Result<std::process::Output> {
        cmd.current_dir(self.dir.path()).output()
    }

    /// Like run, but also print output if the build failed.
    fn run_expect(&self, cmd: &mut std::process::Command) -> std::io::Result<std::process::Output> {
        let out = self.run(cmd)?;
        if !out.status.success() {
            print_output(&out);
        }
        Ok(out)
    }

    /// Persist the temp dir locally and abort the test.  Debugging helper.
    #[allow(dead_code)]
    fn eject(self) -> ! {
        panic!("ejected at {:?}", self.dir.into_path());
    }
}

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
        "
rule touch
  command = touch $out
build out: touch in
",
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
        "
rule touch
  command = touch $out
build subdir/out: touch in
",
    )?;
    space.write("in", "")?;
    space.run_expect(&mut n2_command(vec!["subdir/out"]))?;
    assert!(space.read("subdir/out").is_ok());

    Ok(())
}

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
