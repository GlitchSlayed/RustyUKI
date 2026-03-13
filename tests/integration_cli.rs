use anyhow::Result;
use std::cell::RefCell;
use std::collections::VecDeque;

#[path = "../src/app.rs"]
mod app;
#[path = "../src/cli.rs"]
mod cli;
#[path = "../src/cmd.rs"]
mod cmd;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/dracut.rs"]
mod dracut;
#[path = "../src/efi.rs"]
mod efi;
#[path = "../src/error.rs"]
mod error;
#[path = "../src/ukify.rs"]
mod ukify;

use app::{resolve_generate_settings, status};
use cli::GenerateArgs;
use cmd::{CommandRunner, ProcessOutput};
use config::AppConfig;

struct MockRunner {
    responses: RefCell<VecDeque<ProcessOutput>>,
}

impl MockRunner {
    fn new(outputs: Vec<ProcessOutput>) -> Self {
        Self {
            responses: RefCell::new(outputs.into()),
        }
    }
}

impl CommandRunner for MockRunner {
    fn run(&self, _program: &str, _args: &[&str]) -> Result<ProcessOutput> {
        self.responses
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("missing mocked response"))
    }
}

#[test]
fn resolve_settings_cli_override_wins() {
    let cfg = AppConfig::default();
    let args = GenerateArgs {
        kernel_version: Some("6.8.9-custom".to_string()),
        esp_path: Some("/efi".into()),
        output_dir: None,
        cmdline_file: None,
        splash: None,
        os_release: None,
    };

    let resolved = resolve_generate_settings(&cfg, &args, "ignored");
    assert_eq!(resolved.kernel_version, "6.8.9-custom");
    assert_eq!(resolved.esp_path, std::path::PathBuf::from("/efi"));
}

#[test]
fn status_uses_runner_output() {
    let runner = MockRunner::new(vec![ProcessOutput {
        stdout: "6.10.7-test\n".to_string(),
        stderr: String::new(),
    }]);

    let text = status(&runner, &AppConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    assert!(text.contains("kernel: 6.10.7-test"));
}
