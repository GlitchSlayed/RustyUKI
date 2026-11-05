use anyhow::Result;
use rustyuki::assessment::execute_safe_confirmation;
use rustyuki::backend::direct::DirectUefiBackend;
use rustyuki::backend::systemd_boot::SystemdBootBackend;
use rustyuki::backend::BootBackend;
use rustyuki::build::signing::{sign_uki_if_configured, SigningConfig};
use rustyuki::cmd::{CommandRunner, ProcessOutput};
use rustyuki::install::{install_uki_to_esp, rollback_kernel_uki};
use rustyuki::model::{BootBackendType, InstalledUki, KernelFlavor, UkiArtifact};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct MockCall {
    program: String,
    args: Vec<String>,
    output: Result<ProcessOutput>,
}

struct TestMockRunner {
    expected: RefCell<VecDeque<MockCall>>,
    calls: RefCell<Vec<(String, Vec<String>)>>,
}

impl TestMockRunner {
    fn new(calls: Vec<MockCall>) -> Self {
        Self {
            expected: RefCell::new(calls.into()),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn assert_complete(&self) {
        let pending = self.expected.borrow();
        assert!(
            pending.is_empty(),
            "expected call queue was not empty: {} remaining",
            pending.len()
        );
    }
}

impl CommandRunner for TestMockRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<ProcessOutput> {
        self.calls.borrow_mut().push((
            program.to_string(),
            args.iter().map(|s| s.to_string()).collect(),
        ));

        if program == "sbsign" {
            if let Some(idx) = args.iter().position(|a| *a == "--output") {
                if let Some(output_path) = args.get(idx + 1) {
                    std::fs::write(output_path, b"MZ-signed-uki-content").unwrap();
                }
            }
        }

        let call = self
            .expected
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected command executed: {} {:?}", program, args));

        assert_eq!(call.program, program, "program mismatch");
        assert_eq!(call.args, args, "arguments mismatch for {}", program);
        call.output
    }
}

#[test]
fn test_systemd_boot_backend_activation() {
    let runner = TestMockRunner::new(vec![
        MockCall {
            program: "bootctl".to_string(),
            args: vec!["update".to_string()],
            output: Ok(ProcessOutput::default()),
        },
        MockCall {
            program: "bootctl".to_string(),
            args: vec![
                "set-oneshot".to_string(),
                "linux-6.17.4-cachyos.efi".to_string(),
            ],
            output: Ok(ProcessOutput::default()),
        },
        MockCall {
            program: "bootctl".to_string(),
            args: vec!["update".to_string()],
            output: Ok(ProcessOutput::default()),
        },
        MockCall {
            program: "bootctl".to_string(),
            args: vec![
                "set-default".to_string(),
                "linux-6.17.4-cachyos.efi".to_string(),
            ],
            output: Ok(ProcessOutput::default()),
        },
    ]);

    let backend = SystemdBootBackend;
    let installed = InstalledUki {
        kernel_version: "6.17.4-cachyos".to_string(),
        path: PathBuf::from("/efi/EFI/Linux/linux-6.17.4-cachyos.efi"),
        manifest_path: PathBuf::from("/efi/EFI/Linux/linux-6.17.4-cachyos.efi.manifest.json"),
        manifest: None,
        has_backup_slot: false,
        backup_slot_path: PathBuf::from("/efi/EFI/Linux/linux-6.17.4-cachyos.prev.efi"),
    };

    // Test trial boot (set-oneshot)
    let res_trial = backend
        .activate(&runner, &installed, Path::new("/efi"), true, false)
        .unwrap();
    assert_eq!(
        res_trial.entry_id,
        Some("linux-6.17.4-cachyos.efi".to_string())
    );

    // Test default activation (set-default)
    let res_default = backend
        .activate(&runner, &installed, Path::new("/efi"), false, false)
        .unwrap();
    assert_eq!(
        res_default.entry_id,
        Some("linux-6.17.4-cachyos.efi".to_string())
    );

    runner.assert_complete();
}

#[test]
fn test_systemd_boot_backend_offline_skips_commands() {
    // When is_offline = true, no bootctl commands should be invoked on host!
    let runner = TestMockRunner::new(vec![]);
    let backend = SystemdBootBackend;
    let installed = InstalledUki {
        kernel_version: "6.17.4-cachyos".to_string(),
        path: PathBuf::from("/target/efi/EFI/Linux/linux-6.17.4-cachyos.efi"),
        manifest_path: PathBuf::from(
            "/target/efi/EFI/Linux/linux-6.17.4-cachyos.efi.manifest.json",
        ),
        manifest: None,
        has_backup_slot: false,
        backup_slot_path: PathBuf::from("/target/efi/EFI/Linux/linux-6.17.4-cachyos.prev.efi"),
    };

    let res = backend
        .activate(&runner, &installed, Path::new("/target/efi"), true, true)
        .unwrap();
    assert_eq!(res.entry_id, Some("linux-6.17.4-cachyos.efi".to_string()));
    runner.assert_complete();
}

#[test]
fn test_direct_uefi_backend_offline_skips_nvram() {
    // When is_offline = true, no efibootmgr commands should be invoked on host!
    let runner = TestMockRunner::new(vec![]);
    let backend = DirectUefiBackend;
    let installed = InstalledUki {
        kernel_version: "6.17.4-fc44".to_string(),
        path: PathBuf::from("/target/efi/EFI/Linux/linux-6.17.4-fc44.efi"),
        manifest_path: PathBuf::from("/target/efi/EFI/Linux/linux-6.17.4-fc44.efi.manifest.json"),
        manifest: None,
        has_backup_slot: false,
        backup_slot_path: PathBuf::from("/target/efi/EFI/Linux/linux-6.17.4-fc44.prev.efi"),
    };

    let res = backend
        .activate(&runner, &installed, Path::new("/target/efi"), true, true)
        .unwrap();
    assert_eq!(res.entry_id, None);
    runner.assert_complete();
}

#[test]
fn test_boot_assessment_blocks_emergency_mode() {
    let runner = TestMockRunner::new(vec![MockCall {
        program: "systemctl".to_string(),
        args: vec!["is-system-running".to_string()],
        output: Ok(ProcessOutput {
            stdout: "emergency\n".to_string(),
            stderr: String::new(),
        }),
    }]);

    let backend = DirectUefiBackend;
    let err = execute_safe_confirmation(&runner, &backend);
    assert!(err.is_err());
    assert!(err.unwrap_err().to_string().contains("emergency"));
    runner.assert_complete();
}

#[test]
fn test_boot_assessment_allows_healthy_mode() {
    let runner = TestMockRunner::new(vec![
        MockCall {
            program: "systemctl".to_string(),
            args: vec!["is-system-running".to_string()],
            output: Ok(ProcessOutput {
                stdout: "running\n".to_string(),
                stderr: String::new(),
            }),
        },
        MockCall {
            program: "bootctl".to_string(),
            args: vec!["status".to_string()],
            output: Ok(ProcessOutput {
                stdout: "default: linux-6.17.4-cachyos.efi\n".to_string(),
                stderr: String::new(),
            }),
        },
        MockCall {
            program: "which".to_string(),
            args: vec!["systemd-bless-boot".to_string()],
            output: Ok(ProcessOutput {
                stdout: "/usr/bin/systemd-bless-boot\n".to_string(),
                stderr: String::new(),
            }),
        },
        MockCall {
            program: "systemd-bless-boot".to_string(),
            args: vec!["good".to_string()],
            output: Ok(ProcessOutput::default()),
        },
        MockCall {
            program: "bootctl".to_string(),
            args: vec!["set-oneshot".to_string(), "".to_string()],
            output: Ok(ProcessOutput::default()),
        },
    ]);

    let backend = SystemdBootBackend;
    let res = execute_safe_confirmation(&runner, &backend).unwrap();
    assert_eq!(res, "default: linux-6.17.4-cachyos.efi");
    runner.assert_complete();
}

#[test]
fn test_install_and_rollback_flow() {
    let temp = TempDir::new().unwrap();
    let esp_out = temp.path().join("EFI/Linux");
    std::fs::create_dir_all(&esp_out).unwrap();

    let staging_dir = temp.path().join("staging");
    std::fs::create_dir_all(&staging_dir).unwrap();
    let staging_uki = staging_dir.join("linux-6.12.0.efi");
    std::fs::write(&staging_uki, b"MZ-v1-initial-content-bytes").unwrap();

    let artifact_v1 = UkiArtifact {
        kernel_version: "6.12.0".to_string(),
        path: staging_uki.clone(),
        sha256: "hash-v1".to_string(),
        size_bytes: 28,
        cmdline_hash: "cmd-v1".to_string(),
        flavor: KernelFlavor::Fedora,
    };

    // Install version 1
    let installed1 =
        install_uki_to_esp(&artifact_v1, &esp_out, BootBackendType::Direct, true).unwrap();
    assert_eq!(
        std::fs::read(&installed1.path).unwrap(),
        b"MZ-v1-initial-content-bytes"
    );
    assert!(installed1.manifest_path.exists());
    assert!(!installed1.backup_slot_path.exists());

    // Update with version 2
    std::fs::write(&staging_uki, b"MZ-v2-updated-content-bytes").unwrap();
    let artifact_v2 = UkiArtifact {
        kernel_version: "6.12.0".to_string(),
        path: staging_uki.clone(),
        sha256: "hash-v2".to_string(),
        size_bytes: 28,
        cmdline_hash: "cmd-v2".to_string(),
        flavor: KernelFlavor::Fedora,
    };

    let installed2 =
        install_uki_to_esp(&artifact_v2, &esp_out, BootBackendType::Direct, true).unwrap();
    assert_eq!(
        std::fs::read(&installed2.path).unwrap(),
        b"MZ-v2-updated-content-bytes"
    );
    assert!(installed2.has_backup_slot);
    assert_eq!(
        std::fs::read(&installed2.backup_slot_path).unwrap(),
        b"MZ-v1-initial-content-bytes"
    );

    // Perform rollback
    let restored = rollback_kernel_uki(&esp_out, "6.12.0").unwrap();
    assert_eq!(restored, installed2.path);
    assert_eq!(
        std::fs::read(&restored).unwrap(),
        b"MZ-v1-initial-content-bytes"
    );
}

#[test]
fn test_signing_with_sbsign() {
    let temp = TempDir::new().unwrap();
    let uki_file = temp.path().join("linux-test.efi");
    let signed_path = temp.path().join("linux-test.signed.efi");
    std::fs::write(&uki_file, b"MZ-unsigned-uki-content").unwrap();

    let runner = TestMockRunner::new(vec![MockCall {
        program: "sbsign".to_string(),
        args: vec![
            "--key".to_string(),
            "/etc/pki/sb/db.key".to_string(),
            "--cert".to_string(),
            "/etc/pki/sb/db.crt".to_string(),
            "--output".to_string(),
            signed_path.display().to_string(),
            uki_file.display().to_string(),
        ],
        output: Ok(ProcessOutput::default()),
    }]);

    let signing_cfg = SigningConfig {
        enabled: true,
        tool: "sbsign".to_string(),
        cert_path: Some(PathBuf::from("/etc/pki/sb/db.crt")),
        key_path: Some(PathBuf::from("/etc/pki/sb/db.key")),
        token: None,
    };

    let signed = sign_uki_if_configured(&runner, &uki_file, &signing_cfg).unwrap();
    assert_eq!(signed, uki_file);
    assert_eq!(std::fs::read(&signed).unwrap(), b"MZ-signed-uki-content");
    runner.assert_complete();
}
