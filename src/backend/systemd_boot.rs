use super::{ActivationResult, BootBackend};
use crate::cmd::CommandRunner;
use crate::model::{BackendDiagnostic, BootBackendType, DiagnosticLevel, InstalledUki};
use anyhow::{Context, Result};
use log::info;
use std::path::Path;

pub struct SystemdBootBackend;

impl BootBackend for SystemdBootBackend {
    fn name(&self) -> &'static str {
        "systemd-boot"
    }

    fn activate(
        &self,
        runner: &dyn CommandRunner,
        uki: &InstalledUki,
        _esp_path: &Path,
        trial: bool,
        is_offline: bool,
    ) -> Result<ActivationResult> {
        let entry_name = format!("linux-{}.efi", uki.kernel_version);

        if is_offline {
            info!(
                "Running in offline mode: UKI deployed under /EFI/Linux/{entry_name}. systemd-boot will discover it automatically."
            );
            return Ok(ActivationResult {
                backend: BootBackendType::SystemdBoot,
                entry_id: Some(entry_name.clone()),
                description: format!(
                    "Deployed Type #2 UKI {entry_name} (offline sysroot mode{})",
                    if trial {
                        "; trial arming deferred until live boot"
                    } else {
                        ""
                    }
                ),
                is_trial: false,
            });
        }

        // Run bootctl update to ensure bootloader binaries and Type #2 discovery are in sync
        info!("Synchronizing systemd-boot via bootctl update");
        if let Err(update_err) = runner.run("bootctl", &["update"]) {
            log::warn!("'bootctl update' reported an error: {update_err:#}");
            // Check whether systemd-boot is installed in ESP
            if let Ok(st) = runner.run("bootctl", &["status"]) {
                if st.stdout.contains("systemd-boot not installed") {
                    anyhow::bail!(
                        "systemd-boot is not installed on the EFI System Partition. Run 'bootctl install' before activation."
                    );
                }
            }
        }

        if trial {
            info!("Configuring systemd-boot one-shot trial: {entry_name}");
            runner
                .run("bootctl", &["set-oneshot", &entry_name])
                .with_context(|| format!("failed setting one-shot trial entry to {entry_name}"))?;

            Ok(ActivationResult {
                backend: BootBackendType::SystemdBoot,
                entry_id: Some(entry_name.clone()),
                description: format!("Configured systemd-boot oneshot trial for {entry_name}"),
                is_trial: true,
            })
        } else {
            info!("Configuring systemd-boot default entry: {entry_name}");
            runner
                .run("bootctl", &["set-default", &entry_name])
                .with_context(|| format!("failed setting default boot entry to {entry_name}"))?;

            Ok(ActivationResult {
                backend: BootBackendType::SystemdBoot,
                entry_id: Some(entry_name.clone()),
                description: format!("Configured systemd-boot default entry to {entry_name}"),
                is_trial: false,
            })
        }
    }

    fn confirm(&self, runner: &dyn CommandRunner) -> Result<String> {
        // Query current bootctl status
        let output = runner
            .run("bootctl", &["status"])
            .context("failed querying bootctl status during confirmation")?;

        // Determine current entry or set-default
        info!("Confirming systemd-boot: verifying active boot and synchronizing default entry");

        // If systemd-bless-boot is available, invoke it and propagate any failure
        if runner.run("which", &["systemd-bless-boot"]).is_ok() {
            runner
                .run("systemd-bless-boot", &["good"])
                .context("systemd-bless-boot failed to mark current boot as good")?;
            info!("Successfully marked boot successful via systemd-bless-boot");
        } else {
            info!("systemd-bless-boot not available; relying on standard bootctl entry state");
        }

        // Clear any lingering oneshot entry and propagate any failure
        runner
            .run("bootctl", &["set-oneshot", ""])
            .context("failed clearing lingering oneshot trial entry via bootctl")?;

        let summary = output
            .stdout
            .lines()
            .find(|l| l.contains("default:"))
            .unwrap_or("confirmed systemd-boot entry")
            .trim()
            .to_string();

        Ok(summary)
    }

    fn status(&self, runner: &dyn CommandRunner) -> Result<String> {
        let output = match runner.run("bootctl", &["status"]) {
            Ok(out) => out.stdout,
            Err(e) => return Ok(format!("systemd-boot: unavailable ({e:#})")),
        };

        let is_installed = if output.contains("systemd-boot not installed") {
            "not installed"
        } else {
            "installed"
        };

        let mut lines = Vec::new();
        lines.push(format!("systemd-boot: {is_installed}"));

        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("default:")
                || trimmed.starts_with("selected:")
                || trimmed.starts_with("features:")
            {
                lines.push(format!("  {trimmed}"));
            }
        }

        Ok(lines.join("\n"))
    }

    fn diagnostics(&self, runner: &dyn CommandRunner) -> Vec<BackendDiagnostic> {
        let mut diags = Vec::new();

        // 1. Check bootctl availability
        if runner.run("which", &["bootctl"]).is_ok() {
            diags.push(BackendDiagnostic {
                level: DiagnosticLevel::Ok,
                title: "bootctl CLI Available".to_string(),
                message: "systemd-boot management utility (bootctl) is installed on host."
                    .to_string(),
            });
        } else {
            diags.push(BackendDiagnostic {
                level: DiagnosticLevel::Error,
                title: "bootctl Missing".to_string(),
                message: "bootctl is not installed. Install systemd-boot to use this backend."
                    .to_string(),
            });
            return diags;
        }

        // 2. Check if systemd-boot is installed into ESP
        let status_res = runner.run("bootctl", &["status"]);
        match status_res {
            Ok(out) => {
                if out.stdout.contains("systemd-boot not installed") {
                    diags.push(BackendDiagnostic {
                        level: DiagnosticLevel::Warning,
                        title: "systemd-boot Not Initialized in ESP".to_string(),
                        message: "systemd-boot is not currently active on this ESP. Run 'bootctl install' to initialize.".to_string(),
                    });
                } else {
                    diags.push(BackendDiagnostic {
                        level: DiagnosticLevel::Ok,
                        title: "systemd-boot Installed in ESP".to_string(),
                        message:
                            "Firmware loader is initialized and active on the EFI System Partition."
                                .to_string(),
                    });
                }
            }
            Err(e) => {
                diags.push(BackendDiagnostic {
                    level: DiagnosticLevel::Warning,
                    title: "bootctl Status Check Inconclusive".to_string(),
                    message: format!("Could not query bootctl status: {e:#}"),
                });
            }
        }

        // 3. Check systemd-bless-boot availability for boot assessment
        if runner.run("which", &["systemd-bless-boot"]).is_ok() {
            diags.push(BackendDiagnostic {
                level: DiagnosticLevel::Ok,
                title: "Boot Assessment Support".to_string(),
                message:
                    "systemd-bless-boot is available for automatic boot counting verification."
                        .to_string(),
            });
        } else {
            diags.push(BackendDiagnostic {
                level: DiagnosticLevel::Info,
                title: "systemd-bless-boot Absent".to_string(),
                message: "systemd-bless-boot is not present; standard systemd-boot fallback will be used.".to_string(),
            });
        }

        // 4. Expose architecture & capability
        diags.push(BackendDiagnostic {
            level: DiagnosticLevel::Info,
            title: "Type #2 Specification & NVRAM Isolation".to_string(),
            message: "Discovers UKIs under /EFI/Linux/. Kernel selection and counting handled without per-kernel NVRAM entries.".to_string(),
        });

        diags
    }
}
