use crate::cmd::CommandRunner;
use anyhow::{bail, Context, Result};
use log::info;
use std::collections::HashSet;
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

/// Validates that ESP mountpoint exists.
pub fn validate_esp_mount(esp_path: &Path) -> Result<()> {
    if !esp_path.exists() {
        bail!("ESP mount path does not exist: {}", esp_path.display());
    }
    if !esp_path.is_dir() {
        bail!("ESP path is not a directory: {}", esp_path.display());
    }
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
        detect_new_entry_number, make_efi_loader_path, parse_boot_state, BootEntry, BootState,
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
}
