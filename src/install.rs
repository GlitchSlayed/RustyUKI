use crate::model::{BootBackendType, InstalledUki, UkiArtifact, UkiManifest};
use anyhow::{bail, Context, Result};
use log::{info, warn};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Deploys a validated UKI artifact to the ESP with durability and fallback guarantees.
///
/// 1. If an existing UKI is present, preserves it as a `.prev.efi` fallback slot.
/// 2. Stages write to `.tmp` file and calls `sync_all()` before rename.
/// 3. Renames staging file to final `linux-<version>.efi`.
/// 4. Syncs the parent directory to commit directory entries on FAT32.
/// 5. Writes `<uki>.manifest.json` sidecar documenting provenance and ownership.
pub fn install_uki_to_esp(
    artifact: &UkiArtifact,
    output_dir: &Path,
    backend: BootBackendType,
    keep_backup: bool,
) -> Result<InstalledUki> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "failed creating destination directory {}",
            output_dir.display()
        )
    })?;

    let final_uki_name = format!("linux-{}.efi", artifact.kernel_version);
    let final_uki_path = output_dir.join(&final_uki_name);
    let backup_path = output_dir.join(format!("linux-{}.prev.efi", artifact.kernel_version));
    let manifest_path = output_dir.join(format!(
        "linux-{}.efi.manifest.json",
        artifact.kernel_version
    ));

    // Preserve previous working UKI as backup slot before overwriting
    let mut has_backup = false;
    if final_uki_path.exists() && keep_backup {
        info!(
            "Preserving existing UKI as fallback slot -> {}",
            backup_path.display()
        );
        let src_metadata = fs::metadata(&final_uki_path).with_context(|| {
            format!(
                "failed reading metadata of existing UKI {}",
                final_uki_path.display()
            )
        })?;
        let src_len = src_metadata.len();

        let bytes_copied = fs::copy(&final_uki_path, &backup_path).with_context(|| {
            format!(
                "failed creating backup slot at {} before replacing {}",
                backup_path.display(),
                final_uki_path.display()
            )
        })?;

        if bytes_copied != src_len {
            let _ = fs::remove_file(&backup_path);
            bail!(
                "backup copy incomplete for {}: wrote {bytes_copied} bytes, expected {src_len} bytes",
                final_uki_path.display()
            );
        }

        let b_file = File::open(&backup_path).with_context(|| {
            format!(
                "failed opening backup file {} for sync",
                backup_path.display()
            )
        })?;
        b_file
            .sync_all()
            .with_context(|| format!("fsync failed on backup slot {}", backup_path.display()))?;

        has_backup = true;
    }

    // Stage write to hidden temporary sibling file on the same filesystem
    let staging_path = output_dir.join(format!(".{final_uki_name}.staging"));
    if staging_path.exists() {
        let _ = fs::remove_file(&staging_path);
    }

    // Copy artifact bytes to staging
    {
        let mut reader = File::open(&artifact.path)
            .with_context(|| format!("failed reading artifact {}", artifact.path.display()))?;
        let mut writer = File::create(&staging_path)
            .with_context(|| format!("failed creating staging file {}", staging_path.display()))?;

        std::io::copy(&mut reader, &mut writer)?;

        // Ensure all bytes and file allocation metadata are committed to physical storage
        writer
            .sync_all()
            .with_context(|| format!("fsync failed on {}", staging_path.display()))?;
    }

    // Atomically rename staging file to final target
    fs::rename(&staging_path, &final_uki_path).with_context(|| {
        format!(
            "failed atomic installation rename {} -> {}",
            staging_path.display(),
            final_uki_path.display()
        )
    })?;

    // Commit directory entry on FAT32
    let dir_file = File::open(output_dir).with_context(|| {
        format!(
            "failed opening directory {} for durability sync",
            output_dir.display()
        )
    })?;
    if let Err(e) = dir_file.sync_all() {
        let is_unsupported = e.kind() == std::io::ErrorKind::Unsupported
            || e.raw_os_error() == Some(22)  // EINVAL
            || e.raw_os_error() == Some(95)  // ENOTSUP / EOPNOTSUPP
            || e.raw_os_error() == Some(38); // ENOSYS
        if is_unsupported {
            log::debug!(
                "Directory sync not supported by filesystem at {}: {e}",
                output_dir.display()
            );
        } else {
            return Err(e).with_context(|| {
                format!(
                    "directory durability sync failed on {}",
                    output_dir.display()
                )
            });
        }
    }

    // Record sidecar provenance manifest
    let manifest = UkiManifest::new(
        &artifact.kernel_version,
        &artifact.sha256,
        &artifact.cmdline_hash,
        backend,
        artifact.flavor.clone(),
    );

    let manifest_json = serde_json::to_string_pretty(&manifest)
        .context("failed serializing UKI provenance manifest")?;

    let mut m_file = File::create(&manifest_path)
        .with_context(|| format!("failed creating manifest {}", manifest_path.display()))?;
    m_file.write_all(manifest_json.as_bytes())?;
    m_file
        .sync_all()
        .with_context(|| format!("fsync failed on manifest {}", manifest_path.display()))?;

    info!(
        "Successfully installed UKI: {} (sha256: {}...)",
        final_uki_path.display(),
        &artifact.sha256[..12.min(artifact.sha256.len())]
    );

    Ok(InstalledUki {
        kernel_version: artifact.kernel_version.clone(),
        path: final_uki_path,
        manifest_path,
        manifest: Some(manifest),
        has_backup_slot: has_backup,
        backup_slot_path: backup_path,
    })
}

/// Discovers all installed UKIs in the target output directory and reads their provenance.
pub fn list_installed_ukis(output_dir: &Path) -> Result<Vec<InstalledUki>> {
    let mut installed = Vec::new();
    if !output_dir.exists() {
        return Ok(installed);
    }

    for entry in fs::read_dir(output_dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        // Match primary UKIs: linux-<version>.efi (ignoring .prev.efi, .tmp, .staging)
        if !name.starts_with("linux-") || !name.ends_with(".efi") || name.ends_with(".prev.efi") {
            continue;
        }

        let version = name
            .strip_prefix("linux-")
            .and_then(|s| s.strip_suffix(".efi"))
            .unwrap_or_default()
            .to_string();

        let manifest_path = output_dir.join(format!("{name}.manifest.json"));
        let manifest = if manifest_path.exists() {
            fs::read_to_string(&manifest_path)
                .ok()
                .and_then(|text| serde_json::from_str::<UkiManifest>(&text).ok())
        } else {
            None
        };

        let backup_slot_path = output_dir.join(format!("linux-{version}.prev.efi"));
        let has_backup_slot = backup_slot_path.exists();

        installed.push(InstalledUki {
            kernel_version: version,
            path,
            manifest_path,
            manifest,
            has_backup_slot,
            backup_slot_path,
        });
    }

    installed.sort_by(|a, b| b.kernel_version.cmp(&a.kernel_version));
    Ok(installed)
}

/// Safely prunes stale RustyUKI-managed artifacts without touching foreign UKIs or running kernels.
pub fn prune_stale_ukis(
    output_dir: &Path,
    active_kernel_versions: &[String],
    running_kernel_version: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    let installed = list_installed_ukis(output_dir)?;

    for uki in installed {
        // Never prune the currently running kernel
        if let Some(running) = running_kernel_version {
            if uki.kernel_version == running {
                continue;
            }
        }

        // If kernel is still installed in OS, do not prune
        if active_kernel_versions
            .iter()
            .any(|v| v == &uki.kernel_version)
        {
            continue;
        }

        // OWNERSHIP SAFETY: Only delete if manifest confirms RustyUKI provenance!
        if uki.manifest.is_none() {
            warn!(
                "Skipping foreign or unmanaged UKI without manifest: {}",
                uki.path.display()
            );
            continue;
        }

        info!("Pruning stale RustyUKI image: {}", uki.path.display());
        if let Err(err) = fs::remove_file(&uki.path) {
            warn!("Failed removing {}: {err}", uki.path.display());
        } else {
            removed.push(uki.path.clone());
        }

        // Remove manifest
        if uki.manifest_path.exists() {
            let _ = fs::remove_file(&uki.manifest_path);
        }

        // Remove backup slot if present
        if uki.has_backup_slot && uki.backup_slot_path.exists() {
            let _ = fs::remove_file(&uki.backup_slot_path);
        }
    }

    Ok(removed)
}

/// Restores the previous backup slot (.prev.efi) for a kernel.
pub fn rollback_kernel_uki(output_dir: &Path, kernel_version: &str) -> Result<PathBuf> {
    let target_uki = output_dir.join(format!("linux-{kernel_version}.efi"));
    let backup_uki = output_dir.join(format!("linux-{kernel_version}.prev.efi"));

    if !backup_uki.exists() {
        bail!(
            "no rollback backup slot exists for kernel {kernel_version} at {}",
            backup_uki.display()
        );
    }

    info!(
        "Rolling back kernel {kernel_version} from {}",
        backup_uki.display()
    );

    let backup_metadata = fs::metadata(&backup_uki).with_context(|| {
        format!(
            "failed reading metadata of backup slot {}",
            backup_uki.display()
        )
    })?;
    let backup_len = backup_metadata.len();

    // Atomically swap backup into active slot
    let bytes_copied = fs::copy(&backup_uki, &target_uki).with_context(|| {
        format!(
            "failed restoring backup slot {} -> {}",
            backup_uki.display(),
            target_uki.display()
        )
    })?;

    if bytes_copied != backup_len {
        bail!("incomplete rollback copy: copied {bytes_copied} bytes, expected {backup_len} bytes");
    }

    let t_file = File::open(&target_uki).with_context(|| {
        format!(
            "failed opening restored target {} for sync",
            target_uki.display()
        )
    })?;
    t_file
        .sync_all()
        .with_context(|| format!("fsync failed on restored target {}", target_uki.display()))?;

    // Commit directory entry
    if let Ok(dir_file) = File::open(output_dir) {
        let _ = dir_file.sync_all();
    }

    Ok(target_uki)
}
