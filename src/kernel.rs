use crate::cmd::CommandRunner;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn list_installed_kernels(runner: &dyn CommandRunner) -> Result<Vec<String>> {
    let output = runner
        .run("rpm", &["-q", "kernel"])
        .context("failed querying installed kernels with rpm")?;

    let mut kernels = output
        .stdout
        .lines()
        .filter_map(|line| line.strip_prefix("kernel-"))
        .map(ToOwned::to_owned)
        .filter(|version| Path::new(&format!("/lib/modules/{version}/vmlinuz")).exists())
        .collect::<Vec<_>>();

    kernels.sort();
    kernels.dedup();
    Ok(kernels)
}

pub fn sanitize_cmdline(raw: &str) -> String {
    raw.split_whitespace()
        .filter(|token| {
            !token.starts_with("BOOT_IMAGE=")
                && !token.starts_with("initrd=")
                && !token.starts_with("rd.driver.blacklist=")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn resolve_cmdline(
    cmdline_file: &Path,
    configured_cmdline: &str,
    auto_detect: bool,
) -> Result<String> {
    if auto_detect {
        if let Ok(value) = fs::read_to_string("/proc/cmdline") {
            let sanitized = sanitize_cmdline(&value);
            if looks_bootable(&sanitized) {
                return validate_cmdline(sanitized);
            }
        }

        if let Ok(value) = fs::read_to_string(cmdline_file) {
            let sanitized = sanitize_cmdline(&value);
            if looks_bootable(&sanitized) {
                return validate_cmdline(sanitized);
            }
        }
    }

    validate_cmdline(sanitize_cmdline(configured_cmdline))
}

fn looks_bootable(cmdline: &str) -> bool {
    cmdline.contains("root=")
        || cmdline.contains("rd.luks.uuid=")
        || cmdline.contains("rootfstype=")
}

fn validate_cmdline(cmdline: String) -> Result<String> {
    if cmdline.trim().is_empty() {
        bail!("kernel cmdline resolved to an empty value")
    }
    if cmdline
        .split_whitespace()
        .any(|t| t == "root=UUID=REPLACE-ME")
    {
        bail!("refusing to build UKI with placeholder root=UUID=REPLACE-ME")
    }
    Ok(cmdline)
}

pub fn prune_stale_uki_artifacts(
    output_dir: &Path,
    installed_kernels: &[String],
) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    if !output_dir.exists() {
        return Ok(removed);
    }

    for entry in fs::read_dir(output_dir)
        .with_context(|| format!("failed reading {}", output_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if !(name.starts_with("linux-") && name.ends_with(".efi")) {
            continue;
        }

        let version = name
            .strip_prefix("linux-")
            .and_then(|v| v.strip_suffix(".efi"))
            .unwrap_or_default();

        if !installed_kernels.iter().any(|k| k == version) {
            fs::remove_file(&path)
                .with_context(|| format!("failed removing stale UKI {}", path.display()))?;
            removed.push(path);
        }
    }

    Ok(removed)
}
