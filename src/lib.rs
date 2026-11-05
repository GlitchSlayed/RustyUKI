pub mod app;
pub mod assessment;
pub mod backend;
pub mod build;
pub mod cli;
pub mod cmd;
pub mod cmdline;
pub mod config;
pub mod error;
pub mod esp;
pub mod hook;
pub mod install;
pub mod kernel;
pub mod model;
pub mod privilege;

// Backwards-compatibility re-exports for existing tests and modules
pub mod dracut {
    pub use crate::build::dracut::build_initramfs as build_initramfs_legacy;
    use crate::cmd::CommandRunner;
    use anyhow::Result;
    use std::path::{Path, PathBuf};

    pub fn build_initramfs(
        runner: &dyn CommandRunner,
        kernel_version: &str,
        out_path: &Path,
        extra_args: &[String],
    ) -> Result<PathBuf> {
        crate::build::dracut::build_initramfs(runner, kernel_version, out_path, extra_args, None)
    }
}

pub mod ukify {
    pub use crate::build::ukify::{assemble_uki as build_uki, UkifyParams};
}

pub mod efi {
    pub use crate::backend::direct::{
        detect_new_entry_number, parse_boot_state, promote_current_boot_entry, query_boot_state,
        schedule_one_time_boot, BootEntry, BootState,
    };
    pub use crate::esp::{make_efi_loader_path, parse_proc_mounts, unescape_mount_field};

    use crate::cmd::CommandRunner;
    use anyhow::Result;
    use std::path::Path;

    pub fn validate_esp_mount(esp_path: &Path) -> Result<()> {
        if !esp_path.exists() || !esp_path.is_dir() {
            anyhow::bail!(
                "ESP path {} does not exist. Is your ESP mounted?",
                esp_path.display()
            );
        }
        Ok(())
    }

    pub fn validate_esp_preflight(esp_path: &Path, output_dir: &Path) -> Result<()> {
        crate::esp::validate_esp_preflight(esp_path, output_dir, false)
    }

    pub fn register_boot_entry(
        runner: &dyn CommandRunner,
        esp_path: &Path,
        uki_path: &Path,
        label: &str,
    ) -> Result<String> {
        let uki = crate::model::InstalledUki {
            kernel_version: label.replace("Linux UKI ", ""),
            path: uki_path.to_path_buf(),
            manifest_path: uki_path.with_extension("manifest.json"),
            manifest: None,
            has_backup_slot: false,
            backup_slot_path: uki_path.with_extension("prev.efi"),
        };
        let backend = crate::backend::direct::DirectUefiBackend;
        use crate::backend::BootBackend;
        let res = backend.activate(runner, &uki, esp_path, false, false)?;
        Ok(res.entry_id.unwrap_or_else(|| "0000".to_string()))
    }
}
