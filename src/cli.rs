use clap::{ArgAction, Args, Parser, Subcommand};
use std::path::PathBuf;

/// RustyUKI command-line interface.
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

    #[command(subcommand)]
    pub command: Commands,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Generate UKI image for a kernel.
    Generate(GenerateArgs),
    /// Generate UKI and run bootloader update/install step.
    Install(GenerateArgs),
    /// Reconcile UKIs for all installed kernels and prune stale artifacts.
    Reconcile,
    /// Install kernel-install hook so reconcile runs on kernel updates.
    InstallHook(InstallHookArgs),
    /// Show current operational status and resolved settings.
    Status,
    /// Make the currently booted trial UKI the permanent default boot entry.
    Confirm,
}

/// Shared override arguments for generation actions.
#[derive(Debug, Clone, Args)]
pub struct GenerateArgs {
    /// Kernel version override.
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
    /// Set the new EFI entry as the one-time next boot target instead of immediately changing permanent boot order.
    #[arg(long)]
    pub boot_once: bool,
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
