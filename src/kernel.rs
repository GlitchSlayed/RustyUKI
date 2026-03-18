use crate::cmd::CommandRunner;
use anyhow::{bail, Context, Result};
use log::warn;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CmdlineSettings {
    pub configured_cmdline: String,
    pub auto_detect: bool,
    pub cmdline_file: PathBuf,
    pub state_dir: PathBuf,
    pub cmdline_min_tokens: usize,
}

#[derive(Debug, Clone)]
struct Candidate {
    source: &'static str,
    value: String,
    detected: bool,
}

pub fn list_installed_kernels(runner: &dyn CommandRunner) -> Result<Vec<String>> {
    let output = match runner.run("rpm", &["-q", "kernel"]) {
        Ok(output) => output,
        Err(_) => return Ok(Vec::new()),
    };

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

pub fn resolve_cmdline(runner: &dyn CommandRunner, settings: &CmdlineSettings) -> Result<String> {
    let version_id = os_version_id().unwrap_or_default();
    let current_major = major_version(&version_id);

    if settings.auto_detect {
        if let Some(cached) = load_cached_cmdline(settings, &version_id, current_major)? {
            return validate_final_cmdline(
                runner,
                &cached.value,
                cached.source,
                cached.detected,
                settings,
            );
        }

        if let Ok(proc_cmdline) = fs::read_to_string("/proc/cmdline") {
            let clean = sanitize_cmdline(&proc_cmdline);
            if looks_bootable(&clean) {
                let value =
                    validate_final_cmdline(runner, &clean, "/proc/cmdline", true, settings)?;
                persist_cmdline_metadata(settings, &value, "/proc/cmdline", &version_id)?;
                return Ok(value);
            }
        }

        if let Ok(file_cmdline) = fs::read_to_string(&settings.cmdline_file) {
            let clean = sanitize_cmdline(&file_cmdline);
            if looks_bootable(&clean) {
                let value =
                    validate_final_cmdline(runner, &clean, "/etc/kernel/cmdline", true, settings)?;
                persist_cmdline_metadata(settings, &value, "/etc/kernel/cmdline", &version_id)?;
                return Ok(value);
            }
        }

        if let Some(grub_cmdline) = read_grub_cmdline()? {
            let clean = sanitize_cmdline(&grub_cmdline);
            if looks_bootable(&clean) {
                let value =
                    validate_final_cmdline(runner, &clean, "GRUB configuration", true, settings)?;
                persist_cmdline_metadata(settings, &value, "GRUB configuration", &version_id)?;
                return Ok(value);
            }
        }

        warn!("Auto-detect enabled, but no bootable cmdline was detected. Falling back to configured cmdline.");
    }

    let clean = sanitize_cmdline(&settings.configured_cmdline);
    let final_value =
        validate_final_cmdline(runner, &clean, "configured cmdline", false, settings)?;
    persist_cmdline_metadata(settings, &final_value, "configured cmdline", &version_id)?;
    Ok(final_value)
}

fn load_cached_cmdline(
    settings: &CmdlineSettings,
    current_version: &str,
    current_major: &str,
) -> Result<Option<Candidate>> {
    let metadata_dir = settings.state_dir.join("cmdline");
    let version_path = metadata_dir.join("version-id");
    let cmdline_path = metadata_dir.join("effective-cmdline");

    if !cmdline_path.exists() {
        return Ok(None);
    }

    if version_path.exists() {
        let cached_version = fs::read_to_string(&version_path)
            .with_context(|| format!("failed reading {}", version_path.display()))?;
        let cached_major = major_version(cached_version.trim());
        if !cached_major.is_empty() && !current_major.is_empty() && cached_major != current_major {
            warn!(
                "Fedora major VERSION_ID changed ({} -> {}); forcing fresh cmdline detection.",
                cached_version.trim(),
                current_version
            );
            return Ok(None);
        }
    }

    let value = fs::read_to_string(&cmdline_path)
        .with_context(|| format!("failed reading {}", cmdline_path.display()))?;
    let clean = sanitize_cmdline(&value);
    if clean.is_empty() {
        return Ok(None);
    }

    Ok(Some(Candidate {
        source: "cached metadata",
        value: clean,
        detected: true,
    }))
}

fn validate_final_cmdline(
    runner: &dyn CommandRunner,
    cmdline: &str,
    source: &str,
    detected: bool,
    settings: &CmdlineSettings,
) -> Result<String> {
    let normalized = cmdline.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        bail!("kernel cmdline from {source} resolved to an empty value")
    }

    if normalized
        .split_whitespace()
        .any(|token| token == "root=UUID=REPLACE-ME")
    {
        bail!("refusing to build UKI with placeholder root=UUID=REPLACE-ME")
    }

    let token_count = normalized.split_whitespace().count();
    if token_count < settings.cmdline_min_tokens {
        warn!(
            "Cmdline from {} looks unusually short after sanitization ({} token(s)): {}",
            source, token_count, normalized
        );
    }

    validate_root_uuid_against_blkid(runner, &normalized, source, detected)?;
    Ok(normalized)
}

fn validate_root_uuid_against_blkid(
    runner: &dyn CommandRunner,
    cmdline: &str,
    source: &str,
    detected: bool,
) -> Result<()> {
    let Some(uuid) = extract_root_uuid(cmdline) else {
        return Ok(());
    };

    if uuid == "REPLACE-ME" {
        bail!("refusing to continue: cmdline from {source} still contains root=UUID=REPLACE-ME")
    }

    if runner
        .run("blkid", &["-t", &format!("UUID={uuid}")])
        .is_err()
    {
        if detected {
            bail!(
                "detected cmdline from {source} references root UUID '{uuid}', but blkid cannot find it"
            );
        }
        bail!("configured cmdline references root UUID '{uuid}', but blkid cannot find it");
    }

    Ok(())
}

fn extract_root_uuid(cmdline: &str) -> Option<String> {
    cmdline
        .split_whitespace()
        .find_map(|token| token.strip_prefix("root=UUID=").map(ToOwned::to_owned))
}

fn persist_cmdline_metadata(
    settings: &CmdlineSettings,
    cmdline: &str,
    source: &str,
    version_id: &str,
) -> Result<()> {
    let metadata_dir = settings.state_dir.join("cmdline");
    fs::create_dir_all(&metadata_dir)
        .with_context(|| format!("failed creating {}", metadata_dir.display()))?;

    fs::write(
        metadata_dir.join("effective-cmdline"),
        format!("{cmdline}\n"),
    )?;
    fs::write(metadata_dir.join("source"), format!("{source}\n"))?;
    fs::write(metadata_dir.join("version-id"), format!("{version_id}\n"))?;
    Ok(())
}

fn major_version(version: &str) -> &str {
    version.split('.').next().unwrap_or_default()
}

fn os_version_id() -> Result<String> {
    let text = fs::read_to_string("/etc/os-release").context("failed reading /etc/os-release")?;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("VERSION_ID=") {
            return Ok(value.trim_matches('"').to_string());
        }
    }
    Ok(String::new())
}

fn looks_bootable(cmdline: &str) -> bool {
    cmdline.contains("root=")
        || cmdline.contains("rd.luks.uuid=")
        || cmdline.contains("rootfstype=")
}

fn read_grub_cmdline() -> Result<Option<String>> {
    let mut files = vec![PathBuf::from("/etc/default/grub")];
    let grub_d = Path::new("/etc/default/grub.d");
    if grub_d.is_dir() {
        for entry in
            fs::read_dir(grub_d).with_context(|| format!("failed reading {}", grub_d.display()))?
        {
            let entry = entry?;
            if entry.path().extension().and_then(|s| s.to_str()) == Some("cfg") {
                files.push(entry.path());
            }
        }
    }

    for file in files {
        if !file.exists() {
            continue;
        }
        let text = fs::read_to_string(&file)
            .with_context(|| format!("failed reading {}", file.display()))?;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(value) = parse_grub_cmdline_line(line) {
                return Ok(Some(value));
            }
        }
    }

    Ok(None)
}

fn parse_grub_cmdline_line(line: &str) -> Option<String> {
    let (_, rhs) = line.split_once('=')?;
    let key = line.split('=').next()?.trim();
    if key != "GRUB_CMDLINE_LINUX" {
        return None;
    }

    let value = rhs.trim();
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        return Some(value[1..value.len() - 1].to_string());
    }
    if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        return Some(value[1..value.len() - 1].to_string());
    }
    None
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

#[cfg(test)]
mod tests {
    use super::{major_version, parse_grub_cmdline_line, sanitize_cmdline};

    #[test]
    fn sanitize_matches_legacy_behavior() {
        let input = "BOOT_IMAGE=/vmlinuz initrd=/initrd.img root=UUID=abcd rw rd.driver.blacklist=nouveau quiet";
        assert_eq!(sanitize_cmdline(input), "root=UUID=abcd rw quiet");
    }

    #[test]
    fn parse_grub_cmdline_double_and_single_quotes() {
        assert_eq!(
            parse_grub_cmdline_line("GRUB_CMDLINE_LINUX=\"root=UUID=abcd rw quiet\""),
            Some("root=UUID=abcd rw quiet".to_string())
        );
        assert_eq!(
            parse_grub_cmdline_line("GRUB_CMDLINE_LINUX='root=UUID=abcd rw quiet'"),
            Some("root=UUID=abcd rw quiet".to_string())
        );
    }

    #[test]
    fn major_version_extracts_prefix() {
        assert_eq!(major_version("40.20240501"), "40");
    }
}
