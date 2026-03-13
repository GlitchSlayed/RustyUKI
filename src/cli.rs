use clap::{ArgAction, Parser, Subcommand};
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
    /// Show current operational status and resolved settings.
    Status,
}

/// Shared override arguments for generation actions.
#[derive(Debug, Clone, Parser)]
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
}
