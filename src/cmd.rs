use crate::error::CommandError;
use anyhow::{Context, Result};
use log::debug;
use std::ffi::OsStr;
use std::process::Command;

/// Captured process output.
#[derive(Debug, Clone, Default)]
pub struct ProcessOutput {
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
}

/// Trait abstraction for running external commands.
pub trait CommandRunner {
    /// Runs a command with arguments and returns captured output.
    fn run(&self, program: &str, args: &[&str]) -> Result<ProcessOutput>;
}

/// Real command runner backed by `std::process::Command`.
pub struct RealCommandRunner {
    dry_run: bool,
}

impl RealCommandRunner {
    /// Creates a new command runner.
    pub fn new(dry_run: bool) -> Self {
        Self { dry_run }
    }

    fn format_command(program: &str, args: &[&str]) -> String {
        let mut rendered = String::from(program);
        if !args.is_empty() {
            rendered.push(' ');
            rendered.push_str(&args.join(" "));
        }
        rendered
    }
}

impl CommandRunner for RealCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<ProcessOutput> {
        let full = Self::format_command(program, args);
        debug!("exec: {full}");

        if self.dry_run {
            return Ok(ProcessOutput::default());
        }

        let output = Command::new(program)
            .args(args.iter().map(OsStr::new))
            .output()
            .with_context(|| format!("failed spawning command `{full}`"))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            Ok(ProcessOutput { stdout, stderr })
        } else {
            Err(CommandError::Failed {
                command: full,
                exit_code: output.status.code(),
                stderr,
            }
            .into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RealCommandRunner;
    use crate::cmd::CommandRunner;

    #[test]
    fn dry_run_returns_empty_output() {
        let runner = RealCommandRunner::new(true);
        let output = runner
            .run("echo", &["hello"])
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
}
