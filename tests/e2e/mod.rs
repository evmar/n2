//! Support code for e2e tests, which run n2 as a binary.

mod basic;
mod bindings;
mod directories;
mod discovered;
mod missing;
mod regen;
mod validations;

use anyhow::anyhow;

pub fn n2_binary() -> std::path::PathBuf {
    std::env::current_exe()
        .expect("test binary path")
        .parent()
        .expect("test binary directory")
        .parent()
        .expect("binary directory")
        .join("n2")
}

pub fn n2_command(args: Vec<&str>) -> std::process::Command {
    let mut cmd = std::process::Command::new(n2_binary());
    cmd.args(args);
    cmd
}

fn print_output(out: &std::process::Output) {
    // Gross: use print! instead of writing to stdout so Rust test
    // framework can capture it.
    print!("{}", std::str::from_utf8(&out.stdout).unwrap());
    print!("{}", std::str::from_utf8(&out.stderr).unwrap());
}

pub fn assert_output_contains(out: &std::process::Output, text: &str) {
    let out = std::str::from_utf8(&out.stdout).unwrap();
    if !out.contains(text) {
        panic!(
            "assertion failed; expected output to contain {:?} but got:\n{}",
            text, out
        );
    }
}

pub fn assert_stderr_contains(out: &std::process::Output, text: &str) {
    let out = std::str::from_utf8(&out.stderr).unwrap();
    if !out.contains(text) {
        panic!(
            "assertion failed; expected stderr to contain {:?} but got:\n{}",
            text, out
        );
    }
}

pub fn assert_output_not_contains(out: &std::process::Output, text: &str) {
    let out = std::str::from_utf8(&out.stdout).unwrap();
    if out.contains(text) {
        panic!(
            "assertion failed; expected output to not contain {:?} but got:\n{}",
            text, out
        );
    }
}

/// Manages a temporary directory for invoking n2.
pub struct TestSpace {
    dir: tempfile::TempDir,
}
impl TestSpace {
    pub fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        Ok(TestSpace { dir })
    }

    /// Write a file into the working space.
    pub fn write(&self, path: &str, content: &str) -> std::io::Result<()> {
        std::fs::write(self.dir.path().join(path), content)
    }

    /// Read a file from the working space.
    pub fn read(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        let path = self.dir.path().join(path);
        std::fs::read(&path).map_err(|err| anyhow!("read {}: {}", path.display(), err))
    }

    pub fn metadata(&self, path: &str) -> std::io::Result<std::fs::Metadata> {
        std::fs::metadata(self.dir.path().join(path))
    }

    pub fn sub_mtime(&self, path: &str, dur: std::time::Duration) -> anyhow::Result<()> {
        let path = self.dir.path().join(path);
        let t = std::time::SystemTime::now() - dur;
        let f = std::fs::File::options().write(true).open(path)?;
        f.set_modified(t)?;
        Ok(())
    }

    /// Invoke n2, returning process output.
    pub fn run(&self, cmd: &mut std::process::Command) -> std::io::Result<std::process::Output> {
        cmd.current_dir(self.dir.path()).output()
    }

    /// Like run, but also print output if the build failed.
    pub fn run_expect(
        &self,
        cmd: &mut std::process::Command,
    ) -> anyhow::Result<std::process::Output> {
        let out = self.run(cmd)?;
        if !out.status.success() {
            print_output(&out);
            anyhow::bail!("build failed, status {}", out.status);
        }
        Ok(out)
    }

    /// Persist the temp dir locally and abort the test.  Debugging helper.
    #[allow(dead_code)]
    pub fn eject(self) -> ! {
        panic!("ejected at {:?}", self.dir.into_path());
    }
}

// Ensure TOUCH_RULE has the same description and number of lines of text
// on Windows/non-Windows to make tests agnostic to platform.

#[cfg(unix)]
pub const TOUCH_RULE: &str = "
rule touch
  command = touch $out
  description = touch $out
";

#[cfg(windows)]
pub const TOUCH_RULE: &str = "
rule touch
  command = cmd /c type nul > $out
  description = touch $out
";

#[cfg(unix)]
pub const ECHO_RULE: &str = "
rule echo
  command = echo $text
  description = echo $out
";

#[cfg(windows)]
pub const ECHO_RULE: &str = "
rule echo
  command = cmd /c echo $text
  description = echo $out
";
