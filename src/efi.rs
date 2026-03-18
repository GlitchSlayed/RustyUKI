use crate::cmd::CommandRunner;
use anyhow::{bail, Context, Result};
use log::{info, warn};
use std::collections::HashSet;
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootEntry {
    pub num: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootState {
    pub current: Option<String>,
    pub next: Option<String>,
    pub order: Vec<String>,
    pub entries: Vec<BootEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MountInfo {
    target: PathBuf,
    options: Vec<String>,
}

/// Validates that ESP mountpoint exists as a directory.
pub fn validate_esp_mount(esp_path: &Path) -> Result<()> {
    if !esp_path.exists() || !esp_path.is_dir() {
        bail!(
            "ESP path {} does not exist. Is your ESP mounted?",
            esp_path.display()
        );
    }

    Ok(())
}

/// Validates that the ESP and output directory are ready for UKI generation.
pub fn validate_esp_preflight(esp_path: &Path, output_dir: &Path) -> Result<()> {
    validate_esp_mount(esp_path)?;

    let mounts = parse_proc_mounts(
        &fs::read_to_string("/proc/mounts")
            .context("failed reading /proc/mounts while validating ESP mount state")?,
    )?;

    let mount = mounts
        .iter()
        .find(|mount| mount.target == esp_path)
        .with_context(|| {
            format!(
                "ESP path {} is not a mount point. Run: mount {}",
                esp_path.display(),
                esp_path.display()
            )
        })?;

    if mount.options.iter().any(|option| option == "ro") {
        bail!(
            "ESP is mounted read-only. Run: mount -o remount,rw {}",
            esp_path.display()
        );
    }

    let free_bytes = statvfs_free_bytes(esp_path)?;
    let free_mb = free_bytes / (1024 * 1024);
    if free_mb < 50 {
        bail!("ESP has only {free_mb}mb free. At least 50mb is required to write a UKI safely.");
    }
    if free_mb < 150 {
        warn!("ESP has only {free_mb}mb free. Consider freeing space before generating a new UKI.");
    }

    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "failed creating UKI output directory {}",
            output_dir.display()
        )
    })?;

    ensure_path_writable(output_dir).with_context(|| {
        format!(
            "Output directory {} is not writable. Run `ls -la {}` to diagnose permissions.",
            output_dir.display(),
            output_dir.display()
        )
    })?;

    Ok(())
}

/// Registers EFI boot entry for generated UKI and returns the created boot number.
pub fn register_boot_entry(
    runner: &dyn CommandRunner,
    esp_path: &Path,
    uki_path: &Path,
    label: &str,
) -> Result<String> {
    let esp_str = esp_path
        .to_str()
        .with_context(|| format!("non-UTF8 esp path {}", esp_path.display()))?;

    let source = runner
        .run("findmnt", &["-n", "-o", "SOURCE", esp_str])
        .context("failed determining ESP source device")?
        .stdout
        .trim()
        .to_string();

    if source.is_empty() {
        bail!(
            "failed to determine ESP source device for {}",
            esp_path.display()
        );
    }

    let disk = runner
        .run("lsblk", &["-no", "PKNAME", &source])
        .context("failed determining parent disk")?
        .stdout
        .trim()
        .to_string();

    if disk.is_empty() {
        bail!("failed to resolve parent disk for {source}");
    }

    let part_num = runner
        .run("lsblk", &["-no", "PARTNUM", &source])
        .context("failed determining ESP partition number")?
        .stdout
        .trim()
        .to_string();

    if part_num.is_empty() {
        bail!("failed to resolve partition number for {source}");
    }

    let relative = make_efi_loader_path(esp_path, uki_path)?;
    let before = query_boot_state(runner).ok();

    info!("Registering EFI entry label={label}, loader={relative}");
    runner.run(
        "efibootmgr",
        &[
            "--quiet",
            "--create",
            "--disk",
            &format!("/dev/{disk}"),
            "--part",
            &part_num,
            "--label",
            label,
            "--loader",
            &relative,
        ],
    )?;

    let after = query_boot_state(runner).context("failed reading EFI boot entries after create")?;
    detect_new_entry_number(before.as_ref(), &after, label)
}

pub fn query_boot_state(runner: &dyn CommandRunner) -> Result<BootState> {
    let output = runner
        .run("efibootmgr", &["--verbose"])
        .context("failed querying EFI boot manager state")?;
    parse_boot_state(&output.stdout)
}

pub fn schedule_one_time_boot(runner: &dyn CommandRunner, boot_num: &str) -> Result<()> {
    info!("Scheduling one-time boot via BootNext={boot_num}");
    runner
        .run("efibootmgr", &["--bootnext", boot_num])
        .with_context(|| format!("failed setting BootNext to {boot_num}"))?;
    Ok(())
}

pub fn promote_current_boot_entry(runner: &dyn CommandRunner) -> Result<String> {
    let state = query_boot_state(runner)?;
    let current = state.current.clone().context(
        "firmware did not report BootCurrent; boot into the trial UKI first, then run `rustyuki confirm`",
    )?;

    if !state.entries.iter().any(|entry| entry.num == current) {
        bail!("BootCurrent {current} is not present in efibootmgr output");
    }

    let mut boot_order = Vec::with_capacity(state.order.len().saturating_add(1));
    boot_order.push(current.clone());
    for entry in &state.order {
        if entry != &current {
            boot_order.push(entry.clone());
        }
    }

    let boot_order_arg = boot_order.join(",");
    info!("Promoting current boot entry {current} to the front of BootOrder");
    runner
        .run("efibootmgr", &["--bootorder", &boot_order_arg])
        .with_context(|| format!("failed setting BootOrder to {boot_order_arg}"))?;
    Ok(current)
}

fn detect_new_entry_number(
    before: Option<&BootState>,
    after: &BootState,
    label: &str,
) -> Result<String> {
    let prior: HashSet<&str> = before
        .map(|state| {
            state
                .entries
                .iter()
                .map(|entry| entry.num.as_str())
                .collect()
        })
        .unwrap_or_default();

    let mut created = after
        .entries
        .iter()
        .filter(|entry| entry.label == label && !prior.contains(entry.num.as_str()));

    if let Some(entry) = created.next() {
        if created.next().is_none() {
            return Ok(entry.num.clone());
        }
    }

    let matching: Vec<&BootEntry> = after
        .entries
        .iter()
        .filter(|entry| entry.label == label)
        .collect();
    if matching.len() == 1 {
        return Ok(matching[0].num.clone());
    }

    bail!("failed to determine EFI boot number for newly created entry `{label}`")
}

fn parse_boot_state(text: &str) -> Result<BootState> {
    let mut state = BootState::default();

    for line in text.lines() {
        if let Some(value) = line.strip_prefix("BootCurrent: ") {
            state.current = Some(value.trim().to_string());
            continue;
        }
        if let Some(value) = line.strip_prefix("BootNext: ") {
            let value = value.trim();
            if !value.is_empty() {
                state.next = Some(value.to_string());
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("BootOrder: ") {
            state.order = value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(ToOwned::to_owned)
                .collect();
            continue;
        }
        if let Some(rest) = line.strip_prefix("Boot") {
            let Some((num, remainder)) = rest.split_once('*') else {
                continue;
            };
            let num = num.trim();
            if num.len() != 4 || !num.chars().all(|c| c.is_ascii_hexdigit()) {
                continue;
            }
            let label = remainder
                .split('\t')
                .next()
                .map(str::trim)
                .unwrap_or_default();
            if !label.is_empty() {
                state.entries.push(BootEntry {
                    num: num.to_string(),
                    label: label.to_string(),
                });
            }
        }
    }

    if state.entries.is_empty() {
        bail!("no EFI boot entries found in efibootmgr output");
    }

    Ok(state)
}

fn parse_proc_mounts(text: &str) -> Result<Vec<MountInfo>> {
    text.lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let _source = fields
                .next()
                .context("malformed /proc/mounts entry: missing source")?;
            let target = fields
                .next()
                .context("malformed /proc/mounts entry: missing target")?;
            let _fstype = fields
                .next()
                .context("malformed /proc/mounts entry: missing fs type")?;
            let options = fields
                .next()
                .context("malformed /proc/mounts entry: missing mount options")?;

            Ok(MountInfo {
                target: PathBuf::from(unescape_mount_field(target)),
                options: options.split(',').map(ToOwned::to_owned).collect(),
            })
        })
        .collect()
}

fn unescape_mount_field(field: &str) -> String {
    field
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn statvfs_free_bytes(path: &Path) -> Result<u64> {
    let path_cstr = CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("path contains interior null byte: {}", path.display()))?;
    let mut stats = std::mem::MaybeUninit::<nix::libc::statvfs>::uninit();

    let rc = unsafe { nix::libc::statvfs(path_cstr.as_ptr(), stats.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed reading free space for ESP path {}", path.display()));
    }

    let stats = unsafe { stats.assume_init() };
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

fn ensure_path_writable(path: &Path) -> Result<()> {
    let path_cstr = CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("path contains interior null byte: {}", path.display()))?;
    let rc = unsafe { nix::libc::access(path_cstr.as_ptr(), nix::libc::W_OK) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()).context("write access check failed")
    }
}

/// Converts absolute UKI path under ESP into EFI loader path using backslashes.
pub fn make_efi_loader_path(esp_path: &Path, uki_path: &Path) -> Result<String> {
    let rel = uki_path.strip_prefix(esp_path).with_context(|| {
        format!(
            "UKI path {} is not under ESP path {}",
            uki_path.display(),
            esp_path.display()
        )
    })?;
    let rel_unix = PathBuf::from("/").join(rel);
    Ok(rel_unix.to_string_lossy().replace('/', "\\"))
}

#[cfg(test)]
mod tests {
    use super::{
        detect_new_entry_number, make_efi_loader_path, parse_boot_state, parse_proc_mounts,
        unescape_mount_field, BootEntry, BootState,
    };
    use std::path::Path;

    #[test]
    fn efi_loader_path_converts_to_backslashes() {
        let loader = make_efi_loader_path(
            Path::new("/boot/efi"),
            Path::new("/boot/efi/EFI/Linux/linux-6.8.efi"),
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(loader, "\\EFI\\Linux\\linux-6.8.efi");
    }

    #[test]
    fn parse_boot_state_extracts_current_next_order_and_entries() {
        let parsed = parse_boot_state(
            "BootCurrent: 0003\nBootNext: 0007\nBootOrder: 0003,0001,0007\nBoot0001* Fedora\tHD(...)\nBoot0007* Linux UKI 6.11.4\tHD(...)\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(parsed.current.as_deref(), Some("0003"));
        assert_eq!(parsed.next.as_deref(), Some("0007"));
        assert_eq!(parsed.order, vec!["0003", "0001", "0007"]);
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.entries[1].label, "Linux UKI 6.11.4");
    }

    #[test]
    fn detect_new_entry_prefers_bootnum_not_seen_before() {
        let before = BootState {
            current: Some("0001".to_string()),
            next: None,
            order: vec!["0001".to_string()],
            entries: vec![BootEntry {
                num: "0001".to_string(),
                label: "Fedora".to_string(),
            }],
        };
        let after = BootState {
            current: Some("0001".to_string()),
            next: None,
            order: vec!["0001".to_string(), "0007".to_string()],
            entries: vec![
                BootEntry {
                    num: "0001".to_string(),
                    label: "Fedora".to_string(),
                },
                BootEntry {
                    num: "0007".to_string(),
                    label: "Linux UKI 6.11.4".to_string(),
                },
            ],
        };

        let boot_num = detect_new_entry_number(Some(&before), &after, "Linux UKI 6.11.4")
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(boot_num, "0007");
    }

    #[test]
    fn parse_proc_mounts_tracks_target_and_options() {
        let mounts = parse_proc_mounts("/dev/nvme0n1p1 /boot/efi vfat rw,nosuid,nodev 0 0\n")
            .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].target, Path::new("/boot/efi"));
        assert_eq!(mounts[0].options, vec!["rw", "nosuid", "nodev"]);
    }

    #[test]
    fn unescape_mount_field_decodes_proc_escapes() {
        assert_eq!(unescape_mount_field("/boot/My\\040ESP"), "/boot/My ESP");
    }
}
