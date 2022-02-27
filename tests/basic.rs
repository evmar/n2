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
    fn run(&self, mut args: Vec<String>) -> std::io::Result<std::process::Output> {
        args.push("-C".to_string());
        args.push(self.dir.path().to_string_lossy().to_string());
        std::process::Command::new(n2_binary()).args(args).output()
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
    let out = space.run(vec![])?;
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
    assert!(space.run(vec!["out".to_string()])?.status.success());
    assert!(space.read("out").is_ok());

    Ok(())
}
