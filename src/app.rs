use crate::cli::GenerateArgs;
use crate::cmd::CommandRunner;
use crate::config::AppConfig;
use crate::dracut::build_initramfs;
use crate::efi::{
    promote_current_boot_entry, register_boot_entry, schedule_one_time_boot, validate_esp_preflight,
};
use crate::kernel::{
    list_installed_kernels, prune_stale_uki_artifacts, resolve_cmdline, CmdlineSettings,
};
use crate::ukify::{build_uki, UkifyParams};
use anyhow::{Context, Result};
use log::info;
use std::fs;
use std::path::{Path, PathBuf};

/// Fully resolved runtime settings for generation.
#[derive(Debug, Clone)]
pub struct GenerateSettings {
    /// Kernel version to build.
    pub kernel_version: String,
    /// ESP mount path.
    pub esp_path: PathBuf,
    /// Final output directory.
    pub output_dir: PathBuf,
    /// cmdline file path.
    pub cmdline_file: PathBuf,
    /// Optional splash path.
    pub splash: Option<PathBuf>,
    /// os-release path.
    pub os_release: PathBuf,
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

    GenerateSettings {
        kernel_version,
        esp_path: args
            .esp_path
            .clone()
            .unwrap_or_else(|| cfg.uki.esp_path.clone()),
        output_dir: args
            .output_dir
            .clone()
            .unwrap_or_else(|| cfg.uki.output_dir.clone()),
        cmdline_file: args
            .cmdline_file
            .clone()
            .unwrap_or_else(|| cfg.uki.cmdline_file.clone()),
        splash,
        os_release: args
            .os_release
            .clone()
            .unwrap_or_else(|| cfg.uki.os_release.clone()),
    }
}

/// Executes UKI generation pipeline.
pub fn generate(
    runner: &dyn CommandRunner,
    cfg: &AppConfig,
    settings: &GenerateSettings,
    boot_once: bool,
) -> Result<(PathBuf, String)> {
    validate_esp_preflight(&settings.esp_path, &settings.output_dir)?;
    ensure_required_paths(settings)?;

    let initramfs = PathBuf::from(format!("/tmp/initramfs-{}.img", settings.kernel_version));
    let kernel_image = PathBuf::from(format!("/lib/modules/{}/vmlinuz", settings.kernel_version));

    let _built_initramfs = build_initramfs(
        runner,
        &settings.kernel_version,
        &initramfs,
        &cfg.dracut.extra_args,
    )?;

    let normalized_cmdline = resolve_cmdline(
        runner,
        &CmdlineSettings {
            configured_cmdline: cfg.uki.configured_cmdline.clone(),
            auto_detect: cfg.uki.auto_detect_cmdline,
            cmdline_file: settings.cmdline_file.clone(),
            state_dir: cfg.uki.cmdline_state_dir.clone(),
            cmdline_min_tokens: cfg.uki.cmdline_min_tokens,
        },
    )?;

    let uki_path = settings
        .output_dir
        .join(format!("linux-{}.efi", settings.kernel_version));

    let params = UkifyParams {
        kernel_image: &kernel_image,
        initramfs_image: &initramfs,
        cmdline: &normalized_cmdline,
        os_release: &settings.os_release,
        splash: settings.splash.as_deref(),
        output: &uki_path,
        extra_args: &cfg.ukify.extra_args,
    };

    let built_uki = build_uki(runner, &params)?;

    let label = format!("Linux UKI {}", settings.kernel_version);
    let boot_num = register_boot_entry(runner, &settings.esp_path, &built_uki, &label)?;

    if boot_once {
        schedule_one_time_boot(runner, &boot_num)?;
        info!("Scheduled BootNext for Boot{boot_num}; run `rustyuki confirm` after a successful trial boot to make it permanent");
    }

    info!(
        "UKI generation finished: {} (Boot{})",
        built_uki.display(),
        boot_num
    );
    Ok((built_uki, boot_num))
}

/// Performs install flow: generate UKI, then execute bootloader maintenance.
pub fn install(
    runner: &dyn CommandRunner,
    cfg: &AppConfig,
    settings: &GenerateSettings,
    boot_once: bool,
) -> Result<PathBuf> {
    validate_esp_preflight(&settings.esp_path, &settings.output_dir)?;

    let (path, _boot_num) = generate(runner, cfg, settings, boot_once)?;

    let installed = list_installed_kernels(runner)?;
    let removed = prune_stale_uki_artifacts(&settings.output_dir, &installed)?;
    if !removed.is_empty() {
        info!("Pruned {} stale UKI artifact(s)", removed.len());
    }

    info!("Updating bootloader via bootctl");
    runner
        .run("bootctl", &["update"])
        .context("bootctl update failed")?;

    Ok(path)
}

/// Rebuilds UKIs for all installed kernels and prunes stale artifacts.
pub fn reconcile(
    runner: &dyn CommandRunner,
    cfg: &AppConfig,
    base: &GenerateSettings,
) -> Result<()> {
    let kernels = list_installed_kernels(runner)?;
    for kernel in &kernels {
        let mut settings = base.clone();
        settings.kernel_version = kernel.clone();
        let _ = generate(runner, cfg, &settings, false)?;
    }

    let removed = prune_stale_uki_artifacts(&base.output_dir, &kernels)?;
    if !removed.is_empty() {
        info!("Pruned {} stale UKI artifact(s)", removed.len());
    }

    runner
        .run("bootctl", &["update"])
        .context("bootctl update failed")?;

    Ok(())
}

/// Promotes the currently booted EFI entry to the front of BootOrder.
pub fn confirm(runner: &dyn CommandRunner) -> Result<String> {
    let boot_num = promote_current_boot_entry(runner)?;
    info!("Confirmed Boot{boot_num} as the permanent default boot entry");
    Ok(boot_num)
}

/// Reports current status and resolved paths.
pub fn status(runner: &dyn CommandRunner, cfg: &AppConfig) -> Result<String> {
    let uname = runner
        .run("uname", &["-r"])
        .context("failed running uname")?
        .stdout
        .trim()
        .to_string();

    let mut lines = Vec::new();
    lines.push(format!("kernel: {uname}"));
    lines.push(format!("esp_path: {}", cfg.uki.esp_path.display()));
    lines.push(format!("output_dir: {}", cfg.uki.output_dir.display()));
    lines.push(format!("cmdline_file: {}", cfg.uki.cmdline_file.display()));

    if let Some(advisory) = discoverable_root_advisory(runner, &cfg.uki.cmdline_file)? {
        lines.push(advisory);
    }

    lines.push(format!("os_release: {}", cfg.uki.os_release.display()));
    Ok(lines.join("\n"))
}

fn discoverable_root_advisory(
    runner: &dyn CommandRunner,
    cmdline_file: &Path,
) -> Result<Option<String>> {
    let cmdline = match fs::read_to_string(cmdline_file) {
        Ok(content) => content,
        Err(_) => return Ok(None),
    };

    let has_root = cmdline
        .split_whitespace()
        .any(|token| token.starts_with("root="));

    let mounts = match fs::read_to_string("/proc/mounts") {
        Ok(content) => content,
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

fn discoverable_root_message(has_root: bool, parttype: &str, arch: &str) -> Option<String> {
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

fn parse_root_mount_source(proc_mounts: &str) -> Option<String> {
    proc_mounts.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let source = fields.next()?;
        let target = fields.next()?;
        (target == "/").then(|| source.to_string())
    })
}

fn dps_root_guid(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("4f68bce3-e8cd-4db1-96e7-fbcaf984b709"),
        "x86" | "i386" | "i586" | "i686" => Some("44479540-f297-41b2-9af7-d131d5f0458a"),
        "arm" | "armv7" | "armv7l" => Some("69dad710-2ce4-4e3c-b16c-21a1d49abed3"),
        "aarch64" => Some("b921b045-1df0-41c3-af44-4c6f280d3fae"),
        "riscv64" => Some("72ec70a6-cf74-40e6-bd49-4bda08e8f224"),
        "loongarch64" => Some("77055800-792c-4f94-b39a-98c91b762bb6"),
        _ => None,
    }
}

fn ensure_required_paths(settings: &GenerateSettings) -> Result<()> {
    let kernel_image = PathBuf::from(format!("/lib/modules/{}/vmlinuz", settings.kernel_version));
    require_exists(&kernel_image, "kernel image")?;
    require_exists(&settings.cmdline_file, "cmdline file")?;
    require_exists(&settings.os_release, "os-release file")?;
    if let Some(splash) = &settings.splash {
        require_exists(splash, "splash file")?;
    }
    Ok(())
}

fn require_exists(path: &Path, label: &str) -> Result<()> {
    if path.exists() {
        Ok(())
    } else {
        anyhow::bail!("required {label} does not exist: {}", path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        discoverable_root_message, dps_root_guid, parse_root_mount_source,
        resolve_generate_settings,
    };
    use crate::cli::GenerateArgs;
    use crate::config::AppConfig;

    #[test]
    fn resolve_uses_uname_when_config_empty() {
        let cfg = AppConfig::default();
        let args = GenerateArgs {
            kernel_version: None,
            esp_path: None,
            output_dir: None,
            cmdline_file: None,
            splash: None,
            os_release: None,
            boot_once: false,
        };

        let resolved = resolve_generate_settings(&cfg, &args, "6.9.0-test");
        assert_eq!(resolved.kernel_version, "6.9.0-test");
    }

    #[test]
    fn parse_root_mount_source_finds_root_device() {
        let mounts = "/dev/mapper/root / xfs rw,relatime 0 0\n/dev/nvme0n1p1 /boot vfat rw 0 0\n";
        assert_eq!(
            parse_root_mount_source(mounts).as_deref(),
            Some("/dev/mapper/root")
        );
    }

    #[test]
    fn dps_root_guid_knows_x86_64() {
        assert_eq!(
            dps_root_guid("x86_64"),
            Some("4f68bce3-e8cd-4db1-96e7-fbcaf984b709")
        );
    }

    #[test]
    fn discoverable_root_message_warns_when_root_token_present() {
        let message =
            discoverable_root_message(true, "4f68bce3-e8cd-4db1-96e7-fbcaf984b709", "x86_64")
                .unwrap_or_else(|| panic!("missing message"));
        assert!(message.contains("[INFO] Root partition carries a discoverable GPT GUID."));
        assert!(message.contains("'root=' in your cmdline may be unnecessary"));
    }

    #[test]
    fn discoverable_root_message_confirms_when_root_token_missing() {
        let message =
            discoverable_root_message(false, "4f68bce3-e8cd-4db1-96e7-fbcaf984b709", "x86_64")
                .unwrap_or_else(|| panic!("missing message"));
        assert_eq!(
            message,
            "[OK] Cmdline has no 'root='. GPT autodiscovery will locate the root partition."
        );
    }
}
