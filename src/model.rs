use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Boot management backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BootBackendType {
    /// Firmware directly executes the UKI via UEFI boot entries.
    #[default]
    Direct,
    /// systemd-boot manages and launches UKIs under /EFI/Linux.
    SystemdBoot,
}

impl std::str::FromStr for BootBackendType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "direct" | "uefi" | "direct-uefi" => Ok(Self::Direct),
            "systemd-boot" | "sd-boot" | "systemd" => Ok(Self::SystemdBoot),
            _ => Err(format!(
                "unknown boot backend '{s}'; expected 'direct' or 'systemd-boot'"
            )),
        }
    }
}

impl std::fmt::Display for BootBackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Direct => write!(f, "direct"),
            Self::SystemdBoot => write!(f, "systemd-boot"),
        }
    }
}

/// Distribution kernel flavor categorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KernelFlavor {
    /// Standard Fedora kernel (kernel, kernel-core).
    Fedora,
    /// CachyOS high-performance kernel (kernel-cachyos, kernel-cachyos-lto, etc.).
    CachyOs,
    /// Custom, upstream vanilla, or generic Linux kernel.
    Generic,
}

impl std::fmt::Display for KernelFlavor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fedora => write!(f, "Fedora"),
            Self::CachyOs => write!(f, "CachyOS"),
            Self::Generic => write!(f, "Generic"),
        }
    }
}

/// Metadata describing an installed Linux kernel candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelInfo {
    /// Kernel release version (e.g. "6.17.4-100.fc44.x86_64" or "6.17.4-1.cachyos.fc44.x86_64").
    pub version: String,
    /// Detected kernel flavor.
    pub flavor: KernelFlavor,
    /// Absolute path to the kernel binary (vmlinuz).
    pub vmlinuz_path: PathBuf,
    /// Directory containing modules (/lib/modules/<version>).
    pub modules_dir: PathBuf,
    /// RPM package name if discovered through package management.
    pub package_name: Option<String>,
}

/// Provenance metadata recorded alongside installed UKIs.
///
/// Written to `<uki-path>.manifest.json` on the ESP to ensure RustyUKI
/// tracks ownership, creation parameters, and avoids pruning foreign UKIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UkiManifest {
    /// Schema version for future evolution.
    pub schema_version: u32,
    /// RustyUKI generator identifier.
    pub generator: String,
    /// Version of RustyUKI that built the image.
    pub generator_version: String,
    /// Target kernel version string.
    pub kernel_version: String,
    /// Creation timestamp (ISO 8601 or RFC 3339).
    pub created_at: String,
    /// SHA-256 hash of the final .efi binary.
    pub uki_sha256: String,
    /// Hash of the cmdline embedded into the image.
    pub cmdline_hash: String,
    /// Boot backend targeted during installation.
    pub backend: BootBackendType,
    /// Kernel flavor.
    pub flavor: KernelFlavor,
}

impl UkiManifest {
    pub const CURRENT_SCHEMA: u32 = 1;

    pub fn new(
        kernel_version: &str,
        uki_sha256: &str,
        cmdline_hash: &str,
        backend: BootBackendType,
        flavor: KernelFlavor,
    ) -> Self {
        Self {
            schema_version: Self::CURRENT_SCHEMA,
            generator: "rustyuki".to_string(),
            generator_version: env!("CARGO_PKG_VERSION").to_string(),
            kernel_version: kernel_version.to_string(),
            created_at: format!("{:?}", std::time::SystemTime::now()),
            uki_sha256: uki_sha256.to_string(),
            cmdline_hash: cmdline_hash.to_string(),
            backend,
            flavor,
        }
    }
}

/// Generated UKI artifact in staging before deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UkiArtifact {
    pub kernel_version: String,
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    pub cmdline_hash: String,
    pub flavor: KernelFlavor,
}

/// Information about an existing UKI installed on the boot partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledUki {
    pub kernel_version: String,
    pub path: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: Option<UkiManifest>,
    pub has_backup_slot: bool,
    pub backup_slot_path: PathBuf,
}

/// Result of boot activation operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationResult {
    pub backend: BootBackendType,
    pub entry_id: Option<String>,
    pub description: String,
    pub is_trial: bool,
}

/// System boot health assessment status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootHealthStatus {
    /// Fully healthy: system reached targets and services are running cleanly.
    Healthy,
    /// Degraded: some non-essential services failed, but system is usable.
    Degraded,
    /// Emergency/Rescue mode: critical failure occurred during startup.
    Emergency,
    /// State unknown or not running under systemd init.
    Unknown,
}
