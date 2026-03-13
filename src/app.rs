use crate::cli::GenerateArgs;
use crate::cmd::CommandRunner;
use crate::config::AppConfig;
use crate::dracut::build_initramfs;
use crate::efi::{register_boot_entry, validate_esp_mount};
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
) -> Result<PathBuf> {
    validate_esp_mount(&settings.esp_path)?;
    ensure_required_paths(settings)?;

    fs::create_dir_all(&settings.output_dir).with_context(|| {
        format!(
            "failed creating UKI output directory {}",
            settings.output_dir.display()
        )
    })?;

    let initramfs = PathBuf::from(format!("/tmp/initramfs-{}.img", settings.kernel_version));
    let kernel_image = PathBuf::from(format!("/lib/modules/{}/vmlinuz", settings.kernel_version));

    let _built_initramfs = build_initramfs(
        runner,
        &settings.kernel_version,
        &initramfs,
        &cfg.dracut.extra_args,
    )?;

    let cmdline = fs::read_to_string(&settings.cmdline_file).with_context(|| {
        format!(
            "failed reading cmdline file {}",
            settings.cmdline_file.display()
        )
    })?;
    let normalized_cmdline = cmdline.split_whitespace().collect::<Vec<_>>().join(" ");

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
    register_boot_entry(runner, &settings.esp_path, &built_uki, &label)?;

    info!("UKI generation finished: {}", built_uki.display());
    Ok(built_uki)
}

/// Performs install flow: generate UKI, then execute bootloader maintenance.
pub fn install(
    runner: &dyn CommandRunner,
    cfg: &AppConfig,
    settings: &GenerateSettings,
) -> Result<PathBuf> {
    let path = generate(runner, cfg, settings)?;
    info!("Updating bootloader via bootctl");
    runner
        .run("bootctl", &["update"])
        .context("bootctl update failed")?;
    Ok(path)
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
    lines.push(format!("os_release: {}", cfg.uki.os_release.display()));
    Ok(lines.join("\n"))
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
    use super::resolve_generate_settings;
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
        };

        let resolved = resolve_generate_settings(&cfg, &args, "6.9.0-test");
        assert_eq!(resolved.kernel_version, "6.9.0-test");
    }
}
