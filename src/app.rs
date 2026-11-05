use crate::assessment::{execute_safe_confirmation, render_boot_confirm_service};
use crate::backend::create_backend;
use crate::build::{generate_uki, GeneratePipelineParams};
use crate::cli::GenerateArgs;
use crate::cmd::CommandRunner;
use crate::cmdline::{resolve_cmdline, CmdlineSettings};
use crate::config::AppConfig;
use crate::esp::{dps_root_guid, validate_esp_preflight};
use crate::install::{
    install_uki_to_esp, list_installed_ukis, prune_stale_ukis, rollback_kernel_uki,
};
use crate::kernel::{discover_installed_kernels, resolve_target_kernel};
use crate::model::{BootBackendType, KernelInfo};
use anyhow::{bail, Context, Result};
use log::info;
use std::fs;
use std::path::{Path, PathBuf};

/// Fully resolved runtime settings for generation.
#[derive(Debug, Clone)]
pub struct GenerateSettings {
    pub kernel_version: String,
    pub esp_path: PathBuf,
    pub output_dir: PathBuf,
    pub cmdline_file: PathBuf,
    pub splash: Option<PathBuf>,
    pub os_release: PathBuf,
    pub sysroot: Option<PathBuf>,
    pub backend: BootBackendType,
}

/// Builds resolved settings from config + CLI overrides.
pub fn resolve_generate_settings(
    cfg: &AppConfig,
    args: &GenerateArgs,
    uname: &str,
) -> GenerateSettings {
    let kernel_version = args.kernel_version.clone().unwrap_or_else(|| {
        if cfg.uki.kernel_version.is_empty() {
            uname.to_string()
        } else {
            cfg.uki.kernel_version.clone()
        }
    });

    let splash = args
        .splash
        .clone()
        .or_else(|| (!cfg.uki.splash.is_empty()).then(|| PathBuf::from(&cfg.uki.splash)));

    let backend = args.backend.unwrap_or(cfg.bootloader.backend);

    GenerateSettings {
        kernel_version,
        esp_path: args
            .esp_path
            .clone()
            .unwrap_or_else(|| cfg.effective_esp_path()),
        output_dir: args
            .output_dir
            .clone()
            .unwrap_or_else(|| cfg.effective_output_dir()),
        cmdline_file: args
            .cmdline_file
            .clone()
            .unwrap_or_else(|| cfg.effective_cmdline_file()),
        splash,
        os_release: args
            .os_release
            .clone()
            .unwrap_or_else(|| cfg.effective_os_release()),
        sysroot: args.sysroot.clone().or_else(|| cfg.uki.sysroot.clone()),
        backend,
    }
}

/// Executes UKI generation pipeline.
///
/// Builds and validates the PE binary in staging. If `boot_once` is requested,
/// deploys to ESP and arms a trial boot.
pub fn generate(
    runner: &dyn CommandRunner,
    cfg: &AppConfig,
    settings: &GenerateSettings,
    boot_once: bool,
) -> Result<(PathBuf, String)> {
    let is_offline = settings.sysroot.is_some();
    validate_esp_preflight(&settings.esp_path, &settings.output_dir, is_offline)?;

    let target_kernel = resolve_target_kernel(
        runner,
        if settings.kernel_version.is_empty() {
            None
        } else {
            Some(&settings.kernel_version)
        },
        settings.sysroot.as_deref(),
    )?;

    ensure_required_files(&target_kernel, settings)?;

    let normalized_cmdline = resolve_cmdline(
        runner,
        &CmdlineSettings {
            configured_cmdline: cfg.uki.configured_cmdline.clone(),
            auto_detect: cfg.uki.auto_detect_cmdline,
            cmdline_file: settings.cmdline_file.clone(),
            state_dir: cfg.resolve_path(&cfg.uki.cmdline_state_dir),
            cmdline_min_tokens: cfg.uki.cmdline_min_tokens,
            sysroot: settings.sysroot.clone(),
        },
    )?;

    let staging_dir = cfg.resolve_path(&PathBuf::from("/tmp/rustyuki-staging"));

    let artifact = generate_uki(
        runner,
        cfg,
        &GeneratePipelineParams {
            kernel_version: &target_kernel.version,
            kernel_image: &target_kernel.vmlinuz_path,
            cmdline: &normalized_cmdline,
            os_release: &settings.os_release,
            splash: settings.splash.as_deref(),
            staging_dir: &staging_dir,
            flavor: target_kernel.flavor.clone(),
            sysroot: settings.sysroot.as_deref(),
        },
    )?;

    // Deploy to target ESP output directory
    let installed = install_uki_to_esp(
        &artifact,
        &settings.output_dir,
        settings.backend,
        cfg.bootloader.keep_backup,
    )?;

    let backend = create_backend(settings.backend);
    let activation = backend.activate(
        runner,
        &installed,
        &settings.esp_path,
        boot_once,
        is_offline,
    )?;

    let entry_num = activation.entry_id.unwrap_or_else(|| "0000".to_string());

    info!(
        "UKI operation complete: {} -> entry {}",
        installed.path.display(),
        entry_num
    );

    Ok((installed.path, entry_num))
}

/// Executes full install flow: generate, deploy, enable confirmation service, activate.
pub fn install(
    runner: &dyn CommandRunner,
    cfg: &AppConfig,
    settings: &GenerateSettings,
) -> Result<PathBuf> {
    let is_offline = settings.sysroot.is_some();

    // In live mode, install systemd confirmation service with safe late-userspace ordering
    if !is_offline {
        let current_exe =
            std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/usr/local/bin/rustyuki"));
        let unit_path = PathBuf::from("/etc/systemd/system/rustyuki-boot-confirm.service");
        if let Some(parent) = unit_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(unit_content) = render_boot_confirm_service(&current_exe) {
            let _ = fs::write(&unit_path, unit_content);
            let _ = runner.run("systemctl", &["daemon-reload"]);
            let _ = runner.run("systemctl", &["enable", "rustyuki-boot-confirm.service"]);
        }
    }

    let (path, _entry) = generate(runner, cfg, settings, cfg.bootloader.trial_boot)?;

    // Safe prune: only delete stale artifacts that belong to RustyUKI
    let kernels = discover_installed_kernels(runner, settings.sysroot.as_deref())?;
    let active_versions: Vec<String> = kernels.into_iter().map(|k| k.version).collect();

    let running_kernel = if is_offline {
        None
    } else {
        runner
            .run("uname", &["-r"])
            .ok()
            .map(|o| o.stdout.trim().to_string())
    };

    let removed = prune_stale_ukis(
        &settings.output_dir,
        &active_versions,
        running_kernel.as_deref(),
    )?;

    if !removed.is_empty() {
        info!("Pruned {} stale RustyUKI image(s)", removed.len());
    }

    Ok(path)
}

/// Reconciles all installed kernels and prunes stale artifacts.
pub fn reconcile(
    runner: &dyn CommandRunner,
    cfg: &AppConfig,
    base: &GenerateSettings,
) -> Result<()> {
    let kernels = discover_installed_kernels(runner, base.sysroot.as_deref())?;
    info!("Reconciling UKIs for {} installed kernel(s)", kernels.len());

    for kernel in &kernels {
        let mut settings = base.clone();
        settings.kernel_version = kernel.version.clone();
        let _ = generate(runner, cfg, &settings, false)?;
    }

    let active_versions: Vec<String> = kernels.into_iter().map(|k| k.version).collect();
    let running = if base.sysroot.is_some() {
        None
    } else {
        runner
            .run("uname", &["-r"])
            .ok()
            .map(|o| o.stdout.trim().to_string())
    };

    let removed = prune_stale_ukis(&base.output_dir, &active_versions, running.as_deref())?;

    if !removed.is_empty() {
        info!("Pruned {} stale image(s) during reconcile", removed.len());
    }

    Ok(())
}

/// Restores the fallback backup slot for a kernel.
pub fn rollback(cfg: &AppConfig, kernel_version: &str) -> Result<PathBuf> {
    let output_dir = cfg.effective_output_dir();
    rollback_kernel_uki(&output_dir, kernel_version)
}

/// Promotes the booted trial image after multi-stage health assessment.
pub fn confirm(runner: &dyn CommandRunner) -> Result<String> {
    let backend = create_backend(BootBackendType::Direct);
    execute_safe_confirmation(runner, backend.as_ref())
}

/// Reports operational status, boot environment, and GPT autodiscovery advisory.
pub fn status(runner: &dyn CommandRunner, cfg: &AppConfig) -> Result<String> {
    let uname = runner
        .run("uname", &["-r"])
        .map(|o| o.stdout.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let mut lines = Vec::new();
    lines.push(format!("kernel: {uname}"));
    lines.push(format!("backend: {}", cfg.bootloader.backend));
    lines.push(format!("esp_path: {}", cfg.effective_esp_path().display()));
    lines.push(format!(
        "output_dir: {}",
        cfg.effective_output_dir().display()
    ));
    lines.push(format!(
        "cmdline_file: {}",
        cfg.effective_cmdline_file().display()
    ));

    if let Some(advisory) = discoverable_root_advisory(runner, &cfg.effective_cmdline_file())? {
        lines.push(advisory);
    }

    lines.push(format!(
        "os_release: {}",
        cfg.effective_os_release().display()
    ));

    // Backend status
    let backend = create_backend(cfg.bootloader.backend);
    if let Ok(b_status) = backend.status(runner) {
        lines.push(b_status);
    }

    // List installed UKIs & provenance
    if let Ok(ukis) = list_installed_ukis(&cfg.effective_output_dir()) {
        lines.push(format!("installed_ukis: {} found", ukis.len()));
        for uki in &ukis {
            let prov = if uki.manifest.is_some() {
                "rustyuki-managed"
            } else {
                "foreign/unmanaged"
            };
            let backup_str = if uki.has_backup_slot {
                " (+backup)"
            } else {
                ""
            };
            lines.push(format!("  - {}: {prov}{backup_str}", uki.kernel_version));
        }
    }

    // Report failure registry if any failed hooks occurred
    let failure_dir = cfg.resolve_path(&PathBuf::from("/var/lib/uki-build/pending-failures"));
    if failure_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&failure_dir) {
            let failures: Vec<_> = entries
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            if !failures.is_empty() {
                lines.push(format!(
                    "[WARN] Pending kernel update failures registered in {}: {}",
                    failure_dir.display(),
                    failures.join(", ")
                ));
            }
        }
    }

    Ok(lines.join("\n"))
}

fn discoverable_root_advisory(
    runner: &dyn CommandRunner,
    cmdline_file: &Path,
) -> Result<Option<String>> {
    let cmdline = match fs::read_to_string(cmdline_file) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };

    let has_root = cmdline
        .split_whitespace()
        .any(|token| token.starts_with("root="));

    let mounts = match fs::read_to_string("/proc/mounts") {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };

    let Some(root_device) = parse_root_mount_source(&mounts) else {
        return Ok(None);
    };

    if !root_device.starts_with("/dev/") {
        return Ok(None);
    }

    let parttype = runner
        .run("lsblk", &["-no", "PARTTYPE", &root_device])
        .context("failed determining root partition GPT type GUID")?
        .stdout
        .trim()
        .to_ascii_lowercase();

    Ok(discoverable_root_message(
        has_root,
        &parttype,
        std::env::consts::ARCH,
    ))
}

pub fn discoverable_root_message(has_root: bool, parttype: &str, arch: &str) -> Option<String> {
    let expected_guid = dps_root_guid(arch)?;
    if parttype != expected_guid {
        return None;
    }

    Some(if has_root {
        [
            "[INFO] Root partition carries a discoverable GPT GUID.",
            "       'root=' in your cmdline may be unnecessary on this system.",
            "       Consider removing it and testing with: rustyuki generate --dry-run",
        ]
        .join("\n")
    } else {
        "[OK] Cmdline has no 'root='. GPT autodiscovery will locate the root partition.".to_string()
    })
}

pub fn parse_root_mount_source(proc_mounts: &str) -> Option<String> {
    proc_mounts.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let source = fields.next()?;
        let target = fields.next()?;
        (target == "/").then(|| source.to_string())
    })
}

fn ensure_required_files(kernel: &KernelInfo, settings: &GenerateSettings) -> Result<()> {
    if !kernel.vmlinuz_path.exists() {
        bail!(
            "kernel binary does not exist at {}",
            kernel.vmlinuz_path.display()
        );
    }
    if !settings.cmdline_file.exists() && settings.sysroot.is_none() {
        bail!(
            "cmdline file does not exist: {}",
            settings.cmdline_file.display()
        );
    }
    if !settings.os_release.exists() {
        bail!(
            "os-release file does not exist: {}",
            settings.os_release.display()
        );
    }
    if let Some(splash) = &settings.splash {
        if !splash.exists() {
            bail!("splash image does not exist: {}", splash.display());
        }
    }
    Ok(())
}
