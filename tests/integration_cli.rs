use anyhow::{anyhow, Result};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn test_esp_mountpoint() -> PathBuf {
    for candidate in ["/dev/shm", "/tmp", "/"] {
        let path = PathBuf::from(candidate);
        if path.is_dir() {
            return path;
        }
    }
    panic!("no writable mount point available for ESP integration tests");
}

#[allow(dead_code)]
#[path = "../src/app.rs"]
mod app;
#[allow(dead_code)]
#[path = "../src/cli.rs"]
mod cli;
#[allow(dead_code)]
#[path = "../src/cmd.rs"]
mod cmd;
#[allow(dead_code)]
#[path = "../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../src/dracut.rs"]
mod dracut;
#[allow(dead_code)]
#[path = "../src/efi.rs"]
mod efi;
#[allow(dead_code)]
#[path = "../src/error.rs"]
mod error;
#[allow(dead_code)]
#[path = "../src/kernel.rs"]
mod kernel;
#[allow(dead_code)]
#[path = "../src/ukify.rs"]
mod ukify;

use app::{confirm, generate, install, resolve_generate_settings, status, GenerateSettings};
use cli::GenerateArgs;
use cmd::{CommandRunner, ProcessOutput};
use config::AppConfig;
use dracut::build_initramfs;
use efi::{make_efi_loader_path, validate_esp_mount};
use kernel::{prune_stale_uki_artifacts, resolve_cmdline, sanitize_cmdline, CmdlineSettings};
use ukify::{build_uki, UkifyParams};

struct ExpectedCall {
    program: String,
    args: Vec<String>,
    output: Result<ProcessOutput>,
}

struct MockRunner {
    expected: RefCell<VecDeque<ExpectedCall>>,
    calls: RefCell<Vec<(String, Vec<String>)>>,
}

impl MockRunner {
    fn new(expected: Vec<ExpectedCall>) -> Self {
        Self {
            expected: RefCell::new(expected.into()),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn assert_no_pending(&self) {
        let pending = self.expected.borrow();
        assert!(pending.is_empty(), "pending mock calls: {}", pending.len());
    }
}

impl CommandRunner for MockRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<ProcessOutput> {
        self.calls.borrow_mut().push((
            program.to_string(),
            args.iter().map(|s| s.to_string()).collect(),
        ));

        let call = self
            .expected
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| anyhow!("missing mocked response for {program}"))?;

        assert_eq!(call.program, program, "unexpected program");
        assert_eq!(call.args, args, "unexpected arguments for {program}");

        if program == "ukify" {
            if let Some(idx) = args.iter().position(|a| *a == "--output") {
                if let Some(output_path) = args.get(idx + 1) {
                    std::fs::write(output_path, b"dummy-uki")
                        .unwrap_or_else(|e| panic!("failed to create mock ukify output: {e}"));
                }
            }
        }

        call.output
    }
}

fn default_args() -> GenerateArgs {
    GenerateArgs {
        kernel_version: None,
        esp_path: None,
        output_dir: None,
        cmdline_file: None,
        splash: None,
        os_release: None,
        boot_once: false,
    }
}

#[test]
fn resolve_settings_cli_override_wins() {
    let cfg = AppConfig::default();
    let args = GenerateArgs {
        kernel_version: Some("6.8.9-custom".to_string()),
        esp_path: Some("/efi".into()),
        output_dir: Some("/override/out".into()),
        cmdline_file: Some("/override/cmdline".into()),
        splash: Some("/override/splash.bmp".into()),
        os_release: Some("/override/os-release".into()),
        boot_once: false,
    };

    let resolved = resolve_generate_settings(&cfg, &args, "ignored");
    assert_eq!(resolved.kernel_version, "6.8.9-custom");
    assert_eq!(resolved.esp_path, PathBuf::from("/efi"));
    assert_eq!(resolved.output_dir, PathBuf::from("/override/out"));
    assert_eq!(resolved.cmdline_file, PathBuf::from("/override/cmdline"));
    assert_eq!(resolved.splash, Some(PathBuf::from("/override/splash.bmp")));
    assert_eq!(resolved.os_release, PathBuf::from("/override/os-release"));
}

#[test]
fn resolve_settings_falls_back_to_config_and_uname() {
    let mut cfg = AppConfig::default();
    cfg.uki.kernel_version = String::new();
    cfg.uki.esp_path = PathBuf::from("/config/esp");
    cfg.uki.output_dir = PathBuf::from("/config/out");
    cfg.uki.cmdline_file = PathBuf::from("/config/cmdline");
    cfg.uki.splash = "/config/splash.bmp".to_string();
    cfg.uki.os_release = PathBuf::from("/config/os-release");

    let resolved = resolve_generate_settings(&cfg, &default_args(), "6.12.1-mock");
    assert_eq!(resolved.kernel_version, "6.12.1-mock");
    assert_eq!(resolved.esp_path, PathBuf::from("/config/esp"));
    assert_eq!(resolved.output_dir, PathBuf::from("/config/out"));
    assert_eq!(resolved.cmdline_file, PathBuf::from("/config/cmdline"));
    assert_eq!(resolved.splash, Some(PathBuf::from("/config/splash.bmp")));
    assert_eq!(resolved.os_release, PathBuf::from("/config/os-release"));
}

#[test]
fn status_uses_runner_output_and_renders_config_paths() {
    let runner = MockRunner::new(vec![ExpectedCall {
        program: "uname".to_string(),
        args: vec!["-r".to_string()],
        output: Ok(ProcessOutput {
            stdout: "6.10.7-test\n".to_string(),
            stderr: String::new(),
        }),
    }]);

    let text = status(&runner, &AppConfig::default()).unwrap_or_else(|e| panic!("{e}"));
    assert!(text.contains("kernel: 6.10.7-test"));
    assert!(text.contains("esp_path: /boot/efi"));
    assert!(text.contains("output_dir: /boot/efi/EFI/Linux"));
    assert!(text.contains("cmdline_file: /etc/kernel/cmdline"));
    assert!(text.contains("os_release: /etc/os-release"));
    runner.assert_no_pending();
}

#[test]
fn build_initramfs_passes_kernel_output_and_extra_args_in_order() {
    let output_path = Path::new("/tmp/initramfs-6.11.4.img");
    let runner = MockRunner::new(vec![ExpectedCall {
        program: "dracut".to_string(),
        args: vec![
            "-f".to_string(),
            "/tmp/initramfs-6.11.4.img".to_string(),
            "6.11.4".to_string(),
            "--xz".to_string(),
            "--no-hostonly".to_string(),
        ],
        output: Ok(ProcessOutput::default()),
    }]);

    let result = build_initramfs(
        &runner,
        "6.11.4",
        output_path,
        &["--xz".to_string(), "--no-hostonly".to_string()],
    )
    .unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(result, output_path);
    runner.assert_no_pending();
}

#[test]
fn build_uki_creates_temp_file_then_atomically_renames() {
    let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let out_dir = temp.path().join("EFI/Linux");
    std::fs::create_dir_all(&out_dir).unwrap_or_else(|e| panic!("{e}"));

    let kernel = temp.path().join("vmlinuz");
    let initramfs = temp.path().join("initramfs.img");
    let os_release = temp.path().join("os-release");
    let splash = temp.path().join("splash.bmp");
    let output = out_dir.join("linux-6.11.4.efi");

    std::fs::write(&kernel, b"kernel").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&initramfs, b"initramfs").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&os_release, b"NAME=TestOS\n").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&splash, b"bmp").unwrap_or_else(|e| panic!("{e}"));

    let expected_temp_out = out_dir.join(".linux-6.11.4.efi.tmp");

    let runner = MockRunner::new(vec![ExpectedCall {
        program: "ukify".to_string(),
        args: vec![
            "build".to_string(),
            "--linux".to_string(),
            kernel.display().to_string(),
            "--initrd".to_string(),
            initramfs.display().to_string(),
            "--cmdline".to_string(),
            "root=UUID=abcd rw quiet".to_string(),
            "--os-release".to_string(),
            os_release.display().to_string(),
            "--output".to_string(),
            expected_temp_out.display().to_string(),
            "--splash".to_string(),
            splash.display().to_string(),
            "--secureboot-private-key".to_string(),
            "/tmp/key.pem".to_string(),
        ],
        output: Ok(ProcessOutput::default()),
    }]);

    let built = build_uki(
        &runner,
        &UkifyParams {
            kernel_image: &kernel,
            initramfs_image: &initramfs,
            cmdline: "root=UUID=abcd rw quiet",
            os_release: &os_release,
            splash: Some(&splash),
            output: &output,
            extra_args: &[
                "--secureboot-private-key".to_string(),
                "/tmp/key.pem".to_string(),
            ],
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(built, output);
    assert!(output.exists(), "final UKI output must exist");
    assert!(
        !expected_temp_out.exists(),
        "temporary UKI output must be cleaned up"
    );
    runner.assert_no_pending();
}

#[test]
fn build_uki_cleans_temp_file_on_command_failure() {
    let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let out_dir = temp.path().join("EFI/Linux");
    std::fs::create_dir_all(&out_dir).unwrap_or_else(|e| panic!("{e}"));

    let kernel = temp.path().join("vmlinuz");
    let initramfs = temp.path().join("initramfs.img");
    let os_release = temp.path().join("os-release");
    let output = out_dir.join("linux-fail.efi");

    std::fs::write(&kernel, b"kernel").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&initramfs, b"initramfs").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&os_release, b"NAME=TestOS\n").unwrap_or_else(|e| panic!("{e}"));

    let expected_temp_out = out_dir.join(".linux-fail.efi.tmp");

    let runner = MockRunner::new(vec![ExpectedCall {
        program: "ukify".to_string(),
        args: vec![
            "build".to_string(),
            "--linux".to_string(),
            kernel.display().to_string(),
            "--initrd".to_string(),
            initramfs.display().to_string(),
            "--cmdline".to_string(),
            "quiet".to_string(),
            "--os-release".to_string(),
            os_release.display().to_string(),
            "--output".to_string(),
            expected_temp_out.display().to_string(),
        ],
        output: Err(anyhow!("ukify failed")),
    }]);

    let err = build_uki(
        &runner,
        &UkifyParams {
            kernel_image: &kernel,
            initramfs_image: &initramfs,
            cmdline: "quiet",
            os_release: &os_release,
            splash: None,
            output: &output,
            extra_args: &[],
        },
    )
    .expect_err("build_uki should fail");

    assert!(format!("{err:#}").contains("ukify invocation failed"));
    assert!(
        !expected_temp_out.exists(),
        "temporary output should be removed when ukify fails"
    );
    assert!(
        !output.exists(),
        "final output must not be present on failure"
    );
    runner.assert_no_pending();
}

#[test]
fn efi_helpers_validate_mount_and_convert_loader_path() {
    let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    validate_esp_mount(temp.path()).unwrap_or_else(|e| panic!("{e}"));

    let loader = make_efi_loader_path(temp.path(), &temp.path().join("EFI/Linux/linux-6.11.4.efi"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(loader, "\\EFI\\Linux\\linux-6.11.4.efi");
}

#[test]
fn cmdline_sanitization_and_resolution_match_legacy_expectations() {
    let sanitized = sanitize_cmdline(
        "BOOT_IMAGE=/vmlinuz-foo initrd=/initramfs.img root=UUID=abcd rw rd.driver.blacklist=nouveau quiet",
    );
    assert_eq!(sanitized, "root=UUID=abcd rw quiet");

    let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let cmdline_file = temp.path().join("cmdline");
    std::fs::write(&cmdline_file, "root=UUID=file rw").unwrap_or_else(|e| panic!("{e}"));

    let runner = MockRunner::new(vec![ExpectedCall {
        program: "blkid".to_string(),
        args: vec!["-t".to_string(), "UUID=fallback".to_string()],
        output: Ok(ProcessOutput::default()),
    }]);

    let resolved = resolve_cmdline(
        &runner,
        &CmdlineSettings {
            configured_cmdline: "root=UUID=fallback rw".to_string(),
            auto_detect: false,
            cmdline_file: cmdline_file.clone(),
            state_dir: temp.path().join("state"),
            cmdline_min_tokens: 3,
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(resolved, "root=UUID=fallback rw");
    runner.assert_no_pending();
}

#[test]
fn prune_removes_only_unknown_kernel_efis() {
    let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let out = temp.path();

    let keep = out.join("linux-6.11.0.efi");
    let prune = out.join("linux-6.10.0.efi");
    let other = out.join("README.txt");

    std::fs::write(&keep, b"keep").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&prune, b"remove").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&other, b"other").unwrap_or_else(|e| panic!("{e}"));

    let removed =
        prune_stale_uki_artifacts(out, &["6.11.0".to_string()]).unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(removed, vec![prune.clone()]);
    assert!(keep.exists());
    assert!(!prune.exists());
    assert!(other.exists());
}

#[test]
fn generate_with_boot_once_sets_bootnext_immediately() {
    let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let esp = test_esp_mountpoint();
    let out = esp.join("EFI/Linux");
    let cmdline = temp.path().join("cmdline");
    let os_release = temp.path().join("os-release");
    std::fs::create_dir_all(&out).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&cmdline, "root=UUID=test rw quiet").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        &os_release,
        "NAME=TestOS
",
    )
    .unwrap_or_else(|e| panic!("{e}"));

    let kernel_dir = PathBuf::from("/lib/modules/6.11.5-test");
    std::fs::create_dir_all(&kernel_dir).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(kernel_dir.join("vmlinuz"), b"kernel").unwrap_or_else(|e| panic!("{e}"));

    let expected_temp_out = out.join(".linux-6.11.5-test.efi.tmp");
    let final_out = out.join("linux-6.11.5-test.efi");

    let runner = MockRunner::new(vec![
        ExpectedCall {
            program: "dracut".to_string(),
            args: vec![
                "-f".to_string(),
                "/tmp/initramfs-6.11.5-test.img".to_string(),
                "6.11.5-test".to_string(),
            ],
            output: Ok(ProcessOutput::default()),
        },
        ExpectedCall {
            program: "ukify".to_string(),
            args: vec![
                "build".to_string(),
                "--linux".to_string(),
                kernel_dir.join("vmlinuz").display().to_string(),
                "--initrd".to_string(),
                "/tmp/initramfs-6.11.5-test.img".to_string(),
                "--cmdline".to_string(),
                "root=/dev/test rw quiet".to_string(),
                "--os-release".to_string(),
                os_release.display().to_string(),
                "--output".to_string(),
                expected_temp_out.display().to_string(),
            ],
            output: Ok(ProcessOutput::default()),
        },
        ExpectedCall {
            program: "findmnt".to_string(),
            args: vec![
                "-n".to_string(),
                "-o".to_string(),
                "SOURCE".to_string(),
                esp.display().to_string(),
            ],
            output: Ok(ProcessOutput {
                stdout: "/dev/nvme0n1p1
"
                .to_string(),
                stderr: String::new(),
            }),
        },
        ExpectedCall {
            program: "lsblk".to_string(),
            args: vec![
                "-no".to_string(),
                "PKNAME".to_string(),
                "/dev/nvme0n1p1".to_string(),
            ],
            output: Ok(ProcessOutput {
                stdout: "nvme0n1
"
                .to_string(),
                stderr: String::new(),
            }),
        },
        ExpectedCall {
            program: "lsblk".to_string(),
            args: vec![
                "-no".to_string(),
                "PARTNUM".to_string(),
                "/dev/nvme0n1p1".to_string(),
            ],
            output: Ok(ProcessOutput {
                stdout: "1
"
                .to_string(),
                stderr: String::new(),
            }),
        },
        ExpectedCall {
            program: "efibootmgr".to_string(),
            args: vec!["--verbose".to_string()],
            output: Ok(ProcessOutput {
                stdout: "BootCurrent: 0001
BootOrder: 0001
Boot0001* Fedora	HD(...)
"
                .to_string(),
                stderr: String::new(),
            }),
        },
        ExpectedCall {
            program: "efibootmgr".to_string(),
            args: vec![
                "--quiet".to_string(),
                "--create".to_string(),
                "--disk".to_string(),
                "/dev/nvme0n1".to_string(),
                "--part".to_string(),
                "1".to_string(),
                "--label".to_string(),
                "Linux UKI 6.11.5-test".to_string(),
                "--loader".to_string(),
                r"\EFI\Linux\linux-6.11.5-test.efi".to_string(),
            ],
            output: Ok(ProcessOutput::default()),
        },
        ExpectedCall {
            program: "efibootmgr".to_string(),
            args: vec!["--verbose".to_string()],
            output: Ok(ProcessOutput {
                stdout: "BootCurrent: 0001
BootOrder: 0001,0008
Boot0001* Fedora	HD(...)
Boot0008* Linux UKI 6.11.5-test	HD(...)
"
                .to_string(),
                stderr: String::new(),
            }),
        },
        ExpectedCall {
            program: "efibootmgr".to_string(),
            args: vec!["--bootnext".to_string(), "0008".to_string()],
            output: Ok(ProcessOutput::default()),
        },
    ]);

    let mut cfg = AppConfig::default();
    cfg.uki.auto_detect_cmdline = false;
    cfg.uki.configured_cmdline = "root=/dev/test rw quiet".to_string();
    let settings = GenerateSettings {
        kernel_version: "6.11.5-test".to_string(),
        esp_path: esp.clone(),
        output_dir: out.clone(),
        cmdline_file: cmdline,
        splash: None,
        os_release,
    };

    let (built, boot_num) =
        generate(&runner, &cfg, &settings, true).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(built, final_out);
    assert_eq!(boot_num, "0008");
    runner.assert_no_pending();

    std::fs::remove_file(&final_out).ok();
    std::fs::remove_file(kernel_dir.join("vmlinuz")).ok();
    std::fs::remove_dir(kernel_dir).ok();
}

#[test]
fn install_with_boot_once_sets_bootnext_after_bootctl_update() {
    let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let esp = test_esp_mountpoint();
    let out = esp.join("EFI/Linux");
    let cmdline = temp.path().join("cmdline");
    let os_release = temp.path().join("os-release");
    std::fs::create_dir_all(&out).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&cmdline, "root=UUID=test rw quiet").unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        &os_release,
        "NAME=TestOS
",
    )
    .unwrap_or_else(|e| panic!("{e}"));

    let kernel_dir = PathBuf::from("/lib/modules/6.11.4-test");
    std::fs::create_dir_all(&kernel_dir).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(kernel_dir.join("vmlinuz"), b"kernel").unwrap_or_else(|e| panic!("{e}"));

    let expected_temp_out = out.join(".linux-6.11.4-test.efi.tmp");
    let final_out = out.join("linux-6.11.4-test.efi");

    let runner = MockRunner::new(vec![
        ExpectedCall {
            program: "dracut".to_string(),
            args: vec![
                "-f".to_string(),
                "/tmp/initramfs-6.11.4-test.img".to_string(),
                "6.11.4-test".to_string(),
            ],
            output: Ok(ProcessOutput::default()),
        },
        ExpectedCall {
            program: "ukify".to_string(),
            args: vec![
                "build".to_string(),
                "--linux".to_string(),
                kernel_dir.join("vmlinuz").display().to_string(),
                "--initrd".to_string(),
                "/tmp/initramfs-6.11.4-test.img".to_string(),
                "--cmdline".to_string(),
                "root=/dev/test rw quiet".to_string(),
                "--os-release".to_string(),
                os_release.display().to_string(),
                "--output".to_string(),
                expected_temp_out.display().to_string(),
            ],
            output: Ok(ProcessOutput::default()),
        },
        ExpectedCall {
            program: "findmnt".to_string(),
            args: vec![
                "-n".to_string(),
                "-o".to_string(),
                "SOURCE".to_string(),
                esp.display().to_string(),
            ],
            output: Ok(ProcessOutput {
                stdout: "/dev/nvme0n1p1
"
                .to_string(),
                stderr: String::new(),
            }),
        },
        ExpectedCall {
            program: "lsblk".to_string(),
            args: vec![
                "-no".to_string(),
                "PKNAME".to_string(),
                "/dev/nvme0n1p1".to_string(),
            ],
            output: Ok(ProcessOutput {
                stdout: "nvme0n1
"
                .to_string(),
                stderr: String::new(),
            }),
        },
        ExpectedCall {
            program: "lsblk".to_string(),
            args: vec![
                "-no".to_string(),
                "PARTNUM".to_string(),
                "/dev/nvme0n1p1".to_string(),
            ],
            output: Ok(ProcessOutput {
                stdout: "1
"
                .to_string(),
                stderr: String::new(),
            }),
        },
        ExpectedCall {
            program: "efibootmgr".to_string(),
            args: vec!["--verbose".to_string()],
            output: Ok(ProcessOutput {
                stdout: "BootCurrent: 0001
BootOrder: 0001
Boot0001* Fedora	HD(...)
"
                .to_string(),
                stderr: String::new(),
            }),
        },
        ExpectedCall {
            program: "efibootmgr".to_string(),
            args: vec![
                "--quiet".to_string(),
                "--create".to_string(),
                "--disk".to_string(),
                "/dev/nvme0n1".to_string(),
                "--part".to_string(),
                "1".to_string(),
                "--label".to_string(),
                "Linux UKI 6.11.4-test".to_string(),
                "--loader".to_string(),
                r"\EFI\Linux\linux-6.11.4-test.efi".to_string(),
            ],
            output: Ok(ProcessOutput::default()),
        },
        ExpectedCall {
            program: "efibootmgr".to_string(),
            args: vec!["--verbose".to_string()],
            output: Ok(ProcessOutput {
                stdout: "BootCurrent: 0001
BootOrder: 0001,0007
Boot0001* Fedora	HD(...)
Boot0007* Linux UKI 6.11.4-test	HD(...)
"
                .to_string(),
                stderr: String::new(),
            }),
        },
        ExpectedCall {
            program: "efibootmgr".to_string(),
            args: vec!["--bootnext".to_string(), "0007".to_string()],
            output: Ok(ProcessOutput::default()),
        },
        ExpectedCall {
            program: "rpm".to_string(),
            args: vec!["-q".to_string(), "kernel".to_string()],
            output: Ok(ProcessOutput {
                stdout: "kernel-6.11.4-test
"
                .to_string(),
                stderr: String::new(),
            }),
        },
        ExpectedCall {
            program: "bootctl".to_string(),
            args: vec!["update".to_string()],
            output: Ok(ProcessOutput::default()),
        },
    ]);

    let mut cfg = AppConfig::default();
    cfg.uki.auto_detect_cmdline = false;
    cfg.uki.configured_cmdline = "root=/dev/test rw quiet".to_string();
    let settings = GenerateSettings {
        kernel_version: "6.11.4-test".to_string(),
        esp_path: esp.clone(),
        output_dir: out.clone(),
        cmdline_file: cmdline,
        splash: None,
        os_release,
    };

    let installed = install(&runner, &cfg, &settings, true).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(installed, final_out);
    runner.assert_no_pending();

    std::fs::remove_file(&final_out).ok();
    std::fs::remove_file(kernel_dir.join("vmlinuz")).ok();
    std::fs::remove_dir(kernel_dir).ok();
}

#[test]
fn confirm_promotes_current_boot_entry_to_front() {
    let runner = MockRunner::new(vec![
        ExpectedCall {
            program: "efibootmgr".to_string(),
            args: vec!["--verbose".to_string()],
            output: Ok(ProcessOutput {
                stdout: "BootCurrent: 0007
BootNext: 0007
BootOrder: 0001,0007,0003
Boot0001* Fedora	HD(...)
Boot0003* Rescue	HD(...)
Boot0007* Linux UKI 6.11.4-test	HD(...)
"
                .to_string(),
                stderr: String::new(),
            }),
        },
        ExpectedCall {
            program: "efibootmgr".to_string(),
            args: vec!["--bootorder".to_string(), "0007,0001,0003".to_string()],
            output: Ok(ProcessOutput::default()),
        },
    ]);

    let boot_num = confirm(&runner).unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(boot_num, "0007");
    runner.assert_no_pending();
}
