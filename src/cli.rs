use crate::model::BootBackendType;
use clap::{ArgAction, Args, Parser, Subcommand};
use std::path::PathBuf;

/// RustyUKI: Modern Unified Kernel Image lifecycle manager.
#[derive(Debug, Parser)]
#[command(name = "rustyuki", version, about)]
pub struct Cli {
    /// Increase logging verbosity (-v debug, -vv trace).
    #[arg(short = 'v', action = ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Dry run: print actions without performing command execution.
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Path to config file.
    #[arg(long, global = true, default_value = "/etc/uki/uki.conf")]
    pub config: PathBuf,

    /// Target root directory for offline installation or image-building.
    #[arg(long, global = true)]
    pub sysroot: Option<PathBuf>,

    /// Override the bootloader backend (direct or systemd-boot).
    #[arg(long, global = true)]
    pub backend: Option<BootBackendType>,

    #[command(subcommand)]
    pub command: Commands,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Generate UKI image in staging without activating firmware entries.
    Generate(GenerateArgs),
    /// Generate and install UKI to ESP with atomic durability and backup slot.
    Install(GenerateArgs),
    /// Activate a bootloader backend entry for an installed UKI.
    Activate(ActivateArgs),
    /// Roll back to the previous working UKI fallback slot.
    Rollback(RollbackArgs),
    /// Reconcile UKIs for all installed kernels and prune stale artifacts.
    Reconcile,
    /// Install kernel-install hook so reconcile runs on kernel updates.
    InstallHook(InstallHookArgs),
    /// Show current operational status, boot backend, and resolved settings.
    Status,
    /// Assess boot health and make the trial UKI the permanent default boot entry.
    Confirm,
}

/// Shared override arguments for generation and installation actions.
#[derive(Debug, Clone, Args, Default)]
pub struct GenerateArgs {
    /// Target kernel version override.
    #[arg(long)]
    pub kernel_version: Option<String>,
    /// ESP mount path override.
    #[arg(long)]
    pub esp_path: Option<PathBuf>,
    /// UKI output directory override.
    #[arg(long)]
    pub output_dir: Option<PathBuf>,
    /// Kernel cmdline file override.
    #[arg(long)]
    pub cmdline_file: Option<PathBuf>,
    /// Optional splash image path override.
    #[arg(long)]
    pub splash: Option<PathBuf>,
    /// Optional os-release path override.
    #[arg(long)]
    pub os_release: Option<PathBuf>,
    /// Set the new EFI entry as the one-time next boot target instead of changing default.
    #[arg(long)]
    pub boot_once: bool,
    /// Explicit target sysroot directory.
    #[arg(long)]
    pub sysroot: Option<PathBuf>,
    /// Bootloader backend override.
    #[arg(long)]
    pub backend: Option<BootBackendType>,
}

/// Arguments for activating an installed UKI.
#[derive(Debug, Clone, Args)]
pub struct ActivateArgs {
    /// Target kernel version to activate. Defaults to latest installed.
    #[arg(long)]
    pub kernel_version: Option<String>,
    /// One-time trial boot flag.
    #[arg(long)]
    pub trial: bool,
    /// Bootloader backend override.
    #[arg(long)]
    pub backend: Option<BootBackendType>,
}

/// Arguments for rolling back a kernel UKI.
#[derive(Debug, Clone, Args)]
pub struct RollbackArgs {
    /// Kernel version to roll back. Defaults to running or latest installed kernel.
    #[arg(long)]
    pub kernel_version: Option<String>,
}

/// Options for installing the kernel-install hook.
#[derive(Debug, Clone, Parser)]
pub struct InstallHookArgs {
    /// Destination path for the kernel-install plugin script.
    #[arg(long, default_value = "/usr/lib/kernel/install.d/90-rustyuki.install")]
    pub plugin_path: PathBuf,

    /// Optional explicit path to the rustyuki binary to invoke from the hook.
    #[arg(long)]
    pub binary_path: Option<PathBuf>,
}
