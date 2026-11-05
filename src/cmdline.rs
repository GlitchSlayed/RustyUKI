use crate::cmd::CommandRunner;
use anyhow::{bail, Result};
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
    pub sysroot: Option<PathBuf>,
}

impl Default for CmdlineSettings {
    fn default() -> Self {
        Self {
            configured_cmdline: "root=UUID=REPLACE-ME rw quiet rhgb".to_string(),
            auto_detect: true,
            cmdline_file: PathBuf::from("/etc/kernel/cmdline"),
            state_dir: PathBuf::from("/var/lib/uki-build"),
            cmdline_min_tokens: 2,
            sysroot: None,
        }
    }
}

/// Sanitizes kernel command line parameters for UKI embedding.
///
/// Strips only parameters that are strictly bootloader-specific:
/// - `BOOT_IMAGE=...` (GRUB kernel image path)
/// - `initrd=...` (initramfs image path, as initramfs is bundled into the UKI)
///
/// Preserves all user-configured `rd.*` flags, including `rd.driver.blacklist=`.
pub fn sanitize_cmdline(raw: &str) -> String {
    raw.split_whitespace()
        .filter(|token| !token.starts_with("BOOT_IMAGE=") && !token.starts_with("initrd="))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Resolves the effective kernel command line for UKI assembly.
pub fn resolve_cmdline(runner: &dyn CommandRunner, settings: &CmdlineSettings) -> Result<String> {
    let sysroot = settings.sysroot.as_deref();

    if settings.auto_detect {
        // In offline sysroot mode, do NOT inspect the host's /proc/cmdline!
        if sysroot.is_none() {
            // Check cache with content hash validation
            if let Some(cached) = load_cached_cmdline(settings)? {
                return validate_final_cmdline(runner, &cached, "cached metadata", true, settings);
            }

            // Live host /proc/cmdline
            if let Ok(proc_cmdline) = fs::read_to_string("/proc/cmdline") {
                let clean = sanitize_cmdline(&proc_cmdline);
                if looks_bootable(&clean) {
                    let value =
                        validate_final_cmdline(runner, &clean, "/proc/cmdline", true, settings)?;
                    let _ =
                        persist_cmdline_metadata(settings, &value, "/proc/cmdline", &proc_cmdline);
                    return Ok(value);
                }
            }
        }

        // Target cmdline file (/etc/kernel/cmdline)
        let cmdline_path = sysroot_path(sysroot, &settings.cmdline_file);
        if let Ok(file_cmdline) = fs::read_to_string(&cmdline_path) {
            let clean = sanitize_cmdline(&file_cmdline);
            if looks_bootable(&clean) {
                let value = validate_final_cmdline(
                    runner,
                    &clean,
                    &cmdline_path.display().to_string(),
                    true,
                    settings,
                )?;
                let _ = persist_cmdline_metadata(
                    settings,
                    &value,
                    &cmdline_path.display().to_string(),
                    &file_cmdline,
                );
                return Ok(value);
            }
        }

        // Target GRUB configuration (/etc/default/grub)
        if let Some((grub_cmdline, source_name)) = read_grub_cmdline(sysroot)? {
            let clean = sanitize_cmdline(&grub_cmdline);
            if looks_bootable(&clean) {
                let value = validate_final_cmdline(runner, &clean, &source_name, true, settings)?;
                let _ = persist_cmdline_metadata(settings, &value, &source_name, &grub_cmdline);
                return Ok(value);
            }
        }

        warn!("Auto-detect enabled, but no bootable cmdline detected. Falling back to configured cmdline.");
    }

    let clean = sanitize_cmdline(&settings.configured_cmdline);
    let final_value =
        validate_final_cmdline(runner, &clean, "configured cmdline", false, settings)?;
    let _ = persist_cmdline_metadata(
        settings,
        &final_value,
        "configured cmdline",
        &settings.configured_cmdline,
    );
    Ok(final_value)
}

fn sysroot_path(sysroot: Option<&Path>, subpath: &Path) -> PathBuf {
    if let Some(root) = sysroot {
        if subpath.is_absolute() {
            let stripped = subpath.strip_prefix("/").unwrap_or(subpath);
            root.join(stripped)
        } else {
            root.join(subpath)
        }
    } else {
        subpath.to_path_buf()
    }
}

fn load_cached_cmdline(settings: &CmdlineSettings) -> Result<Option<String>> {
    let metadata_dir = settings.state_dir.join("cmdline");
    let cmdline_path = metadata_dir.join("effective-cmdline");
    let hash_path = metadata_dir.join("source-hash");

    if !cmdline_path.exists() || !hash_path.exists() {
        return Ok(None);
    }

    let cached_hash = fs::read_to_string(&hash_path)?.trim().to_string();

    // Verify hash against current source to ensure we don't serve stale data
    let current_source_content = read_primary_source_content(settings)?;
    let current_hash = compute_simple_hash(&current_source_content);

    if cached_hash != current_hash {
        warn!("Kernel command line source content changed; invalidating cached cmdline.");
        return Ok(None);
    }

    let value = fs::read_to_string(&cmdline_path)?;
    let clean = sanitize_cmdline(&value);
    if clean.is_empty() {
        return Ok(None);
    }

    Ok(Some(clean))
}

fn read_primary_source_content(settings: &CmdlineSettings) -> Result<String> {
    if settings.sysroot.is_none() {
        if let Ok(c) = fs::read_to_string("/proc/cmdline") {
            return Ok(c);
        }
    }
    let p = sysroot_path(settings.sysroot.as_deref(), &settings.cmdline_file);
    if let Ok(c) = fs::read_to_string(p) {
        return Ok(c);
    }
    Ok(String::new())
}

fn compute_simple_hash(content: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
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
            "Cmdline from {} looks short ({} token(s)): {}",
            source, token_count, normalized
        );
    }

    // Only validate root UUID against host blkid if running on the live host
    if settings.sysroot.is_none() {
        validate_root_uuid_against_blkid(runner, &normalized, source, detected)?;
    }

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

pub fn extract_root_uuid(cmdline: &str) -> Option<String> {
    cmdline
        .split_whitespace()
        .find_map(|token| token.strip_prefix("root=UUID=").map(ToOwned::to_owned))
}

fn persist_cmdline_metadata(
    settings: &CmdlineSettings,
    cmdline: &str,
    source: &str,
    raw_source_content: &str,
) -> Result<()> {
    let metadata_dir = settings.state_dir.join("cmdline");
    if fs::create_dir_all(&metadata_dir).is_ok() {
        let _ = fs::write(
            metadata_dir.join("effective-cmdline"),
            format!("{cmdline}\n"),
        );
        let _ = fs::write(metadata_dir.join("source"), format!("{source}\n"));
        let hash = compute_simple_hash(raw_source_content);
        let _ = fs::write(metadata_dir.join("source-hash"), format!("{hash}\n"));
    }
    Ok(())
}

pub fn looks_bootable(cmdline: &str) -> bool {
    cmdline.contains("root=")
        || cmdline.contains("rd.luks.uuid=")
        || cmdline.contains("rootfstype=")
        || cmdline.contains("rootflags=")
}

fn read_grub_cmdline(sysroot: Option<&Path>) -> Result<Option<(String, String)>> {
    let root = sysroot.unwrap_or_else(|| Path::new("/"));
    let mut files = vec![root.join("etc/default/grub")];
    let grub_d = root.join("etc/default/grub.d");

    if grub_d.is_dir() {
        if let Ok(entries) = fs::read_dir(&grub_d) {
            for entry in entries.flatten() {
                if entry.path().extension().and_then(|s| s.to_str()) == Some("cfg") {
                    files.push(entry.path());
                }
            }
        }
    }

    for file in files {
        if !file.exists() {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&file) {
            for raw in text.lines() {
                let line = raw.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some(val) = parse_grub_cmdline_line(line) {
                    return Ok(Some((val, file.display().to_string())));
                }
            }
        }
    }

    Ok(None)
}

fn parse_grub_cmdline_line(line: &str) -> Option<String> {
    let (key, rhs) = line.split_once('=')?;
    let key = key.trim();
    if key != "GRUB_CMDLINE_LINUX" && key != "GRUB_CMDLINE_LINUX_DEFAULT" {
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

#[cfg(test)]
mod tests {
    use super::{parse_grub_cmdline_line, sanitize_cmdline};

    #[test]
    fn sanitize_preserves_driver_blacklist() {
        let input = "BOOT_IMAGE=/vmlinuz initrd=/initrd.img root=UUID=abcd rw rd.driver.blacklist=nouveau quiet";
        // BOOT_IMAGE and initrd must be removed, but rd.driver.blacklist must be preserved!
        assert_eq!(
            sanitize_cmdline(input),
            "root=UUID=abcd rw rd.driver.blacklist=nouveau quiet"
        );
    }

    #[test]
    fn parse_grub_quotes() {
        assert_eq!(
            parse_grub_cmdline_line("GRUB_CMDLINE_LINUX=\"root=UUID=abcd rw quiet\""),
            Some("root=UUID=abcd rw quiet".to_string())
        );
    }
}
