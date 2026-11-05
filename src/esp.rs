use anyhow::{bail, Context, Result};
use log::warn;
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfo {
    pub target: PathBuf,
    pub options: Vec<String>,
}

/// Validates that ESP mountpoint exists and is ready for UKI operations.
pub fn validate_esp_preflight(esp_path: &Path, output_dir: &Path, is_offline: bool) -> Result<()> {
    if !esp_path.exists() || !esp_path.is_dir() {
        bail!(
            "ESP path {} does not exist or is not a directory. Is your ESP mounted?",
            esp_path.display()
        );
    }

    if !is_offline {
        let mounts = parse_proc_mounts(
            &fs::read_to_string("/proc/mounts")
                .context("failed reading /proc/mounts while validating ESP mount state")?,
        )?;

        let mount = mounts
            .iter()
            .find(|m| m.target == esp_path)
            .with_context(|| {
                format!(
                    "ESP path {} is not an active mount point. Run: mount {}",
                    esp_path.display(),
                    esp_path.display()
                )
            })?;

        if mount.options.iter().any(|opt| opt == "ro") {
            bail!(
                "ESP is mounted read-only. Run: mount -o remount,rw {}",
                esp_path.display()
            );
        }
    }

    let free_bytes = statvfs_free_bytes(esp_path)?;
    let free_mb = free_bytes / (1024 * 1024);
    if free_mb < 50 {
        bail!("ESP has only {free_mb}MB free. At least 50MB is required to write a UKI safely.");
    }
    if free_mb < 150 {
        warn!("ESP has only {free_mb}MB free. Consider freeing space before generating a new UKI.");
    }

    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "failed creating UKI output directory {}",
            output_dir.display()
        )
    })?;

    ensure_path_writable(output_dir).with_context(|| {
        format!(
            "Output directory {} is not writable. Verify filesystem permissions.",
            output_dir.display()
        )
    })?;

    Ok(())
}

pub fn statvfs_free_bytes(path: &Path) -> Result<u64> {
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

pub fn ensure_path_writable(path: &Path) -> Result<()> {
    let path_cstr = CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("path contains interior null byte: {}", path.display()))?;
    let rc = unsafe { nix::libc::access(path_cstr.as_ptr(), nix::libc::W_OK) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()).context("write access check failed")
    }
}

pub fn parse_proc_mounts(text: &str) -> Result<Vec<MountInfo>> {
    text.lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let _source = fields.next().context("missing source")?;
            let target = fields.next().context("missing target")?;
            let _fstype = fields.next().context("missing fs type")?;
            let options = fields.next().context("missing options")?;

            Ok(MountInfo {
                target: PathBuf::from(unescape_mount_field(target)),
                options: options.split(',').map(ToOwned::to_owned).collect(),
            })
        })
        .collect()
}

pub fn unescape_mount_field(field: &str) -> String {
    field
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

/// Converts absolute UKI path on the ESP into a UEFI backslash loader path.
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

/// Returns the Discoverable Partitions Specification (DPS) root GUID for an architecture.
pub fn dps_root_guid(arch: &str) -> Option<&'static str> {
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

#[cfg(test)]
mod tests {
    use super::{make_efi_loader_path, parse_proc_mounts, unescape_mount_field};
    use std::path::Path;

    #[test]
    fn loader_path_backslashes() {
        let loader = make_efi_loader_path(
            Path::new("/boot/efi"),
            Path::new("/boot/efi/EFI/Linux/linux-6.17.4.efi"),
        )
        .unwrap();
        assert_eq!(loader, "\\EFI\\Linux\\linux-6.17.4.efi");
    }

    #[test]
    fn parse_proc_mounts_test() {
        let input = "/dev/nvme0n1p1 /boot/efi vfat rw,relatime 0 0\n";
        let parsed = parse_proc_mounts(input).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].target, Path::new("/boot/efi"));
        assert!(parsed[0].options.contains(&"rw".to_string()));
    }

    #[test]
    fn unescape_spaces() {
        assert_eq!(unescape_mount_field("/boot/My\\040ESP"), "/boot/My ESP");
    }
}
