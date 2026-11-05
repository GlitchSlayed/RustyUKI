use super::{ActivationResult, BootBackend};
use crate::cmd::CommandRunner;
use crate::esp::make_efi_loader_path;
use crate::model::{BootBackendType, InstalledUki};
use anyhow::{bail, Context, Result};
use log::info;
use std::collections::HashSet;
use std::path::Path;

pub struct DirectUefiBackend;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootState {
    pub current: Option<String>,
    pub next: Option<String>,
    pub order: Vec<String>,
    pub entries: Vec<BootEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootEntry {
    pub num: String,
    pub label: String,
}

pub fn promote_current_boot_entry(runner: &dyn CommandRunner) -> Result<String> {
    let backend = DirectUefiBackend;
    backend.confirm(runner)
}

impl BootBackend for DirectUefiBackend {
    fn name(&self) -> &'static str {
        "direct-uefi"
    }

    fn activate(
        &self,
        runner: &dyn CommandRunner,
        uki: &InstalledUki,
        esp_path: &Path,
        trial: bool,
        is_offline: bool,
    ) -> Result<ActivationResult> {
        if is_offline {
            info!("Running in offline/sysroot mode: skipping host UEFI NVRAM entry registration.");
            return Ok(ActivationResult {
                backend: BootBackendType::Direct,
                entry_id: None,
                description: format!(
                    "Deferred direct boot entry for {} (offline sysroot mode)",
                    uki.kernel_version
                ),
                is_trial: trial,
            });
        }

        let label = format!("Linux UKI {}", uki.kernel_version);
        let loader = make_efi_loader_path(esp_path, &uki.path)?;

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

        // Duplicate prevention: check if an identical entry already exists
        let before_state = query_boot_state(runner).ok();
        if let Some(state) = &before_state {
            if let Some(existing) = state.entries.iter().find(|e| e.label == label) {
                info!(
                    "Found existing EFI boot entry Boot{} for `{label}`",
                    existing.num
                );
                if trial {
                    schedule_one_time_boot(runner, &existing.num)?;
                }
                return Ok(ActivationResult {
                    backend: BootBackendType::Direct,
                    entry_id: Some(existing.num.clone()),
                    description: format!("Reused existing entry Boot{}", existing.num),
                    is_trial: trial,
                });
            }
        }

        info!("Registering EFI entry label={label}, loader={loader}");
        runner.run(
            "efibootmgr",
            &[
                "--quiet",
                "--create",
                "--create-only",
                "--disk",
                &format!("/dev/{disk}"),
                "--part",
                &part_num,
                "--label",
                &label,
                "--loader",
                &loader,
            ],
        )?;

        let after_state = query_boot_state(runner)
            .context("failed querying EFI boot manager state after creation")?;

        let boot_num = detect_new_entry_number(before_state.as_ref(), &after_state, &label)?;

        if trial {
            schedule_one_time_boot(runner, &boot_num)?;
            info!("Scheduled BootNext for Boot{boot_num}");
        }

        Ok(ActivationResult {
            backend: BootBackendType::Direct,
            entry_id: Some(boot_num.clone()),
            description: format!("Created Boot{boot_num} ({label})"),
            is_trial: trial,
        })
    }

    fn confirm(&self, runner: &dyn CommandRunner) -> Result<String> {
        let state = query_boot_state(runner)?;
        let current = state.current.clone().context(
            "firmware did not report BootCurrent; trial UKI must be booted before confirmation",
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

        if state.next.is_some() {
            let _ = runner.run("efibootmgr", &["--delete-bootnext"]);
        }

        Ok(current)
    }

    fn status(&self, runner: &dyn CommandRunner) -> Result<String> {
        let state = match query_boot_state(runner) {
            Ok(s) => s,
            Err(e) => return Ok(format!("direct-uefi: unavailable ({e:#})")),
        };

        let mut lines = Vec::new();
        lines.push("direct-uefi: active".to_string());
        lines.push(format!(
            "  BootCurrent: {}",
            state.current.as_deref().unwrap_or("none")
        ));
        lines.push(format!(
            "  BootNext:    {}",
            state.next.as_deref().unwrap_or("none")
        ));
        lines.push(format!("  BootOrder:   {}", state.order.join(",")));
        lines.push(format!("  Entries:     {} registered", state.entries.len()));

        Ok(lines.join("\n"))
    }
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

pub fn detect_new_entry_number(
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

pub fn parse_boot_state(text: &str) -> Result<BootState> {
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
