use thiserror::Error;

/// Domain error for external command execution failures.
#[derive(Debug, Error)]
pub enum CommandError {
    /// The command returned a non-zero exit code.
    #[error("command `{command}` failed with exit code {exit_code:?}: {stderr}")]
    Failed {
        /// Fully formatted command line.
        command: String,
        /// Process exit code.
        exit_code: Option<i32>,
        /// Captured standard error.
        stderr: String,
    },
}
