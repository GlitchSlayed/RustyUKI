pub mod direct;
pub mod systemd_boot;

pub use direct::DirectUefiBackend;
pub use systemd_boot::SystemdBootBackend;

use crate::cmd::CommandRunner;
use crate::model::{ActivationResult, BootBackendType, InstalledUki};
use anyhow::Result;
use std::path::Path;

/// Common abstraction for bootloader management backends.
pub trait BootBackend {
    /// Human-readable identifier for the backend.
    fn name(&self) -> &'static str;

    /// Activates an installed UKI (registers EFI entry or configures bootctl).
    fn activate(
        &self,
        runner: &dyn CommandRunner,
        uki: &InstalledUki,
        esp_path: &Path,
        trial: bool,
        is_offline: bool,
    ) -> Result<ActivationResult>;

    /// Promotes a successfully verified trial boot to permanent default.
    fn confirm(&self, runner: &dyn CommandRunner) -> Result<String>;

    /// Queries operational status and boot manager variables.
    fn status(&self, runner: &dyn CommandRunner) -> Result<String>;
}

/// Instantiates the appropriate backend based on configuration.
pub fn create_backend(backend_type: BootBackendType) -> Box<dyn BootBackend> {
    match backend_type {
        BootBackendType::Direct => Box::new(DirectUefiBackend),
        BootBackendType::SystemdBoot => Box::new(SystemdBootBackend),
    }
}
