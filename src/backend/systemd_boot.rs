use super::{ActivationResult, BootBackend};
use crate::cmd::CommandRunner;
use crate::model::{BootBackendType, InstalledUki};
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
                description: format!("Deployed Type #2 UKI {entry_name} (offline sysroot mode)"),
                is_trial: trial,
            });
        }

        // Run bootctl update to ensure bootloader binaries and Type #2 discovery are in sync
        info!("Synchronizing systemd-boot via bootctl update");
        let _ = runner.run("bootctl", &["update"]);

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
        let output = runner.run("bootctl", &["status"])?;

        // Determine current entry or set-default
        info!("Confirming systemd-boot: verifying active boot and synchronizing default entry");

        // If systemd-bless-boot is available, invoke it
        if runner.run("which", &["systemd-bless-boot"]).is_ok() {
            let _ = runner.run("systemd-bless-boot", &["good"]);
        }

        // Clear any lingering oneshot
        let _ = runner.run("bootctl", &["set-oneshot", ""]);

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
}
