use crate::model::BootBackendType;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Root application configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
    /// Bootloader backend settings.
    pub bootloader: BootConfig,
    /// UKI build settings.
    pub uki: UkiConfig,
    /// Dracut settings.
    pub dracut: DracutConfig,
    /// Ukify settings.
    pub ukify: UkifyConfig,
    /// Optional Secure Boot signing settings.
    pub signing: SigningConfig,
}

/// Bootloader backend selection and activation preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BootConfig {
    /// Chosen backend: direct UEFI firmware entry or systemd-boot.
    pub backend: BootBackendType,
    /// Whether new installations use trial boot (BootNext for direct, set-oneshot for systemd-boot).
    pub trial_boot: bool,
    /// Whether to keep previous working UKI as .prev.efi fallback.
    pub keep_backup: bool,
    /// Enable systemd-boot boot counting where supported (+3-0.efi).
    pub boot_counting: bool,
}

impl Default for BootConfig {
    fn default() -> Self {
        Self {
            backend: BootBackendType::Direct,
            trial_boot: true,
            keep_backup: true,
            boot_counting: true,
        }
    }
}

/// UKI generation and layout configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UkiConfig {
    /// Target kernel version. Empty string means detect from running kernel or available modules.
    pub kernel_version: String,
    /// Optional target sysroot directory for offline installation.
    pub sysroot: Option<PathBuf>,
    /// ESP mount path relative to root/sysroot.
    pub esp_path: PathBuf,
    /// Output directory for UKI artifacts relative to root/sysroot.
    pub output_dir: PathBuf,
    /// Path to kernel command line file.
    pub cmdline_file: PathBuf,
    /// Fallback kernel cmdline if auto detection fails.
    pub configured_cmdline: String,
    /// Whether to auto-detect cmdline from system sources.
    pub auto_detect_cmdline: bool,
    /// Directory used for cmdline state metadata.
    pub cmdline_state_dir: PathBuf,
    /// Warning threshold for minimum cmdline tokens.
    pub cmdline_min_tokens: usize,
    /// Optional splash image.
    pub splash: String,
    /// Path to `os-release` file.
    pub os_release: PathBuf,
}

impl Default for UkiConfig {
    fn default() -> Self {
        Self {
            kernel_version: String::new(),
            sysroot: None,
            esp_path: PathBuf::from("/boot/efi"),
            output_dir: PathBuf::from("/boot/efi/EFI/Linux"),
            cmdline_file: PathBuf::from("/etc/kernel/cmdline"),
            configured_cmdline: "root=UUID=REPLACE-ME rw quiet rhgb".to_string(),
            auto_detect_cmdline: true,
            cmdline_state_dir: PathBuf::from("/var/lib/uki-build"),
            cmdline_min_tokens: 2,
            splash: String::new(),
            os_release: PathBuf::from("/etc/os-release"),
        }
    }
}

/// Dracut settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DracutConfig {
    /// Additional dracut arguments.
    pub extra_args: Vec<String>,
}

/// Ukify settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UkifyConfig {
    /// Additional ukify arguments.
    pub extra_args: Vec<String>,
}

/// Secure Boot signing settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SigningConfig {
    /// Enable UKI signing.
    pub enabled: bool,
    /// Signing tool: "pesign", "sbsign", or "ukify".
    pub tool: String,
    /// Path to signing certificate / public key.
    pub cert_path: Option<PathBuf>,
    /// Path to signing private key.
    pub key_path: Option<PathBuf>,
    /// Optional PKCS#11 or NSS database token/certificate nickname for pesign.
    pub token: Option<String>,
}

impl AppConfig {
    /// Loads TOML configuration from a file. If the file is missing, searches default locations or returns defaults.
    pub fn load(path: &Path) -> Result<Self> {
        let actual_path = if path.exists() {
            path.to_path_buf()
        } else if Path::new("/etc/rustyuki/config.toml").exists() {
            PathBuf::from("/etc/rustyuki/config.toml")
        } else if Path::new("/etc/uki/uki.conf").exists() {
            PathBuf::from("/etc/uki/uki.conf")
        } else {
            return Ok(Self::default());
        };

        let text = fs::read_to_string(&actual_path)
            .with_context(|| format!("failed reading config file {}", actual_path.display()))?;
        let mut parsed: Self = toml::from_str(&text)
            .with_context(|| format!("failed parsing TOML config {}", actual_path.display()))?;

        // Normalize empty sysroot ("" or "/") to None so it is not mistaken for an offline sysroot
        if let Some(sysroot) = &parsed.uki.sysroot {
            if sysroot.as_os_str().is_empty() || sysroot == Path::new("/") {
                parsed.uki.sysroot = None;
            }
        }
        Ok(parsed)
    }

    /// Resolves an absolute path considering the configured sysroot.
    pub fn resolve_path(&self, subpath: &Path) -> PathBuf {
        if let Some(sysroot) = &self.uki.sysroot {
            if subpath.starts_with(sysroot) {
                return subpath.to_path_buf();
            }
            if subpath.is_absolute() {
                let stripped = subpath.strip_prefix("/").unwrap_or(subpath);
                sysroot.join(stripped)
            } else {
                sysroot.join(subpath)
            }
        } else {
            subpath.to_path_buf()
        }
    }

    /// Resolves the actual effective ESP directory on disk.
    pub fn effective_esp_path(&self) -> PathBuf {
        self.resolve_path(&self.uki.esp_path)
    }

    /// Resolves the actual effective output directory on disk.
    pub fn effective_output_dir(&self) -> PathBuf {
        self.resolve_path(&self.uki.output_dir)
    }

    /// Resolves the effective cmdline file path on disk.
    pub fn effective_cmdline_file(&self) -> PathBuf {
        self.resolve_path(&self.uki.cmdline_file)
    }

    /// Resolves the effective os-release path on disk.
    pub fn effective_os_release(&self) -> PathBuf {
        self.resolve_path(&self.uki.os_release)
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;
    use crate::model::BootBackendType;
    use std::path::PathBuf;

    #[test]
    fn parse_toml_config() {
        let content = r#"
[bootloader]
backend = "systemd-boot"
trial_boot = false
keep_backup = true

[uki]
kernel_version = "6.17.0"
esp_path = "/boot/efi"
output_dir = "/boot/efi/EFI/Linux"
cmdline_file = "/etc/kernel/cmdline"
configured_cmdline = "root=UUID=1234 rw quiet"
auto_detect_cmdline = true
cmdline_state_dir = "/var/lib/uki-build"
cmdline_min_tokens = 2
splash = ""
os_release = "/etc/os-release"

[dracut]
extra_args = ["--omit", "plymouth"]

[ukify]
extra_args = ["--measure"]

[signing]
enabled = false
tool = "pesign"
"#;

        let cfg: AppConfig = toml::from_str(content).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(cfg.bootloader.backend, BootBackendType::SystemdBoot);
        assert!(!cfg.bootloader.trial_boot);
        assert_eq!(cfg.uki.kernel_version, "6.17.0");
        assert!(cfg.uki.auto_detect_cmdline);
        assert_eq!(cfg.dracut.extra_args.len(), 2);
        assert_eq!(cfg.ukify.extra_args.len(), 1);
        assert!(!cfg.signing.enabled);
    }

    #[test]
    fn resolve_path_with_sysroot() {
        let mut cfg = AppConfig::default();
        cfg.uki.sysroot = Some(PathBuf::from("/mnt/sysroot"));
        cfg.uki.esp_path = PathBuf::from("/boot/efi");

        assert_eq!(
            cfg.effective_esp_path(),
            PathBuf::from("/mnt/sysroot/boot/efi")
        );
        assert_eq!(
            cfg.effective_output_dir(),
            PathBuf::from("/mnt/sysroot/boot/efi/EFI/Linux")
        );
    }
}
