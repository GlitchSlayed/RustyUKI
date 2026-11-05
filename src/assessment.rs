use crate::backend::BootBackend;
use crate::cmd::CommandRunner;
use crate::model::BootHealthStatus;
use anyhow::{bail, Context, Result};
use log::{info, warn};

/// Evaluates current system health using systemd unit states.
pub fn assess_system_health(runner: &dyn CommandRunner) -> BootHealthStatus {
    let out = match runner.run("systemctl", &["is-system-running"]) {
        Ok(res) => res.stdout.trim().to_string(),
        Err(e) => {
            warn!("Could not evaluate systemd status: {e:#}");
            return BootHealthStatus::Unknown;
        }
    };

    match out.as_str() {
        "running" => BootHealthStatus::Healthy,
        "degraded" => {
            // Inspect failed units to see if system is fundamentally broken
            if let Ok(failed) = runner.run("systemctl", &["--failed", "--no-legend"]) {
                warn!(
                    "System is running in degraded state with failed units:\n{}",
                    failed.stdout.trim()
                );
            }
            BootHealthStatus::Degraded
        }
        "emergency" | "rescue" | "maintenance" => {
            warn!("System is running in emergency/rescue mode ('{out}')");
            BootHealthStatus::Emergency
        }
        "initializing" | "starting" => {
            warn!("System has not yet finished starting ('{out}')");
            BootHealthStatus::Unknown
        }
        _ => BootHealthStatus::Unknown,
    }
}

/// Executes safe boot confirmation after verifying multi-stage system health.
///
/// Refuses to promote the trial boot if system is in emergency mode
/// or failed critical initialization targets.
pub fn execute_safe_confirmation(
    runner: &dyn CommandRunner,
    backend: &dyn BootBackend,
) -> Result<String> {
    info!("Assessing system health prior to permanent boot confirmation...");

    let health = assess_system_health(runner);
    match health {
        BootHealthStatus::Emergency => {
            bail!("Refusing to confirm boot: system is in emergency or rescue mode. Keep existing fallback boot order!");
        }
        BootHealthStatus::Degraded => {
            warn!("Warning: system reached targets with degraded units; proceeding with cautious confirmation.");
        }
        BootHealthStatus::Healthy => {
            info!("System health verified: systemd reported clean 'running' state.");
        }
        BootHealthStatus::Unknown => {
            info!("System health assessment returned 'unknown'; proceeding with backend confirmation.");
        }
    }

    let confirmation_summary = backend
        .confirm(runner)
        .context("bootloader backend confirmation failed")?;

    info!("Boot confirmation successful: {confirmation_summary}");
    Ok(confirmation_summary)
}

/// Generates systemd service unit rendered with appropriate late-userspace ordering.
pub fn render_boot_confirm_service(binary_path: &std::path::Path) -> Result<String> {
    let bin = binary_path
        .to_str()
        .context("binary path contains invalid UTF-8")?;

    Ok(format!(
        r#"[Unit]
Description=Assess and Confirm Successful RustyUKI Trial Boot
Documentation=https://github.com/GlitchSlayed/RustyUKI
After=multi-user.target graphical.target systemd-boot-check-no-failures.service
Wants=multi-user.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart={bin} confirm

[Install]
WantedBy=multi-user.target
"#
    ))
}
