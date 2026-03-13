use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Root application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// UKI section.
    pub uki: UkiConfig,
    /// Dracut section.
    pub dracut: DracutConfig,
    /// Ukify section.
    pub ukify: UkifyConfig,
}

/// UKI-related configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UkiConfig {
    /// Kernel version. Empty means use `uname -r`.
    pub kernel_version: String,
    /// ESP mount path.
    pub esp_path: PathBuf,
    /// Output directory for UKI artifacts.
    pub output_dir: PathBuf,
    /// Path to kernel command line file.
    pub cmdline_file: PathBuf,
    /// Optional splash image.
    pub splash: String,
    /// Path to `os-release` file.
    pub os_release: PathBuf,
}

/// Dracut settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DracutConfig {
    /// Additional dracut arguments.
    pub extra_args: Vec<String>,
}

/// Ukify settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UkifyConfig {
    /// Additional ukify arguments.
    pub extra_args: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            uki: UkiConfig {
                kernel_version: String::new(),
                esp_path: PathBuf::from("/boot/efi"),
                output_dir: PathBuf::from("/boot/efi/EFI/Linux"),
                cmdline_file: PathBuf::from("/etc/kernel/cmdline"),
                splash: String::new(),
                os_release: PathBuf::from("/etc/os-release"),
            },
            dracut: DracutConfig {
                extra_args: Vec::new(),
            },
            ukify: UkifyConfig {
                extra_args: Vec::new(),
            },
        }
    }
}

impl AppConfig {
    /// Loads TOML configuration from a file. If the file is missing, defaults are returned.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let text = fs::read_to_string(path)
            .with_context(|| format!("failed reading config file {}", path.display()))?;
        let parsed: Self = toml::from_str(&text)
            .with_context(|| format!("failed parsing TOML config {}", path.display()))?;
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;

    #[test]
    fn parse_toml_config() {
        let content = r#"
[uki]
kernel_version = "6.10.0"
esp_path = "/boot/efi"
output_dir = "/boot/efi/EFI/Linux"
cmdline_file = "/etc/kernel/cmdline"
splash = ""
os_release = "/etc/os-release"

[dracut]
extra_args = ["--omit", "plymouth"]

[ukify]
extra_args = ["--measure"]
"#;

        let cfg: AppConfig = toml::from_str(content).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(cfg.uki.kernel_version, "6.10.0");
        assert_eq!(cfg.dracut.extra_args.len(), 2);
        assert_eq!(cfg.ukify.extra_args.len(), 1);
    }
}
