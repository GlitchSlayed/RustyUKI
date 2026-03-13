use crate::cmd::CommandRunner;
use anyhow::{bail, Context, Result};
use log::info;
use std::path::{Path, PathBuf};

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

/// Registers EFI boot entry for generated UKI.
pub fn register_boot_entry(
    runner: &dyn CommandRunner,
    esp_path: &Path,
    uki_path: &Path,
    label: &str,
) -> Result<()> {
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

    let dev_name = Path::new(&source)
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("invalid source device path {source}"))?;

    let part_num = std::fs::read_to_string(format!("/sys/class/block/{dev_name}/partition"))
        .with_context(|| format!("failed reading partition number for {dev_name}"))?
        .trim()
        .to_string();

    let relative = make_efi_loader_path(esp_path, uki_path)?;

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

    Ok(())
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
    use super::make_efi_loader_path;
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
}
