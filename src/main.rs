#![deny(unused_must_use)]

mod app;
mod cli;
mod cmd;
mod config;
mod dracut;
mod efi;
mod error;
mod hook;
mod kernel;
mod privilege;
mod ukify;

use anyhow::{Context, Result};
use app::{confirm, generate, install, reconcile, resolve_generate_settings, status};
use clap::Parser;
use cli::{Cli, Commands};
use cmd::{CommandRunner, RealCommandRunner};
use config::AppConfig;
use env_logger::Env;
use hook::install_kernel_update_hook;
use log::LevelFilter;
use privilege::require_root;

fn init_logging(verbose: u8) {
    let level = match verbose {
        0 => LevelFilter::Info,
        1 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };
    let env = Env::default().default_filter_or(level.as_str());
    env_logger::Builder::from_env(env).init();
}

fn current_kernel(runner: &dyn CommandRunner) -> Result<String> {
    let uname = runner
        .run("uname", &["-r"])
        .context("failed getting running kernel with uname -r")?
        .stdout
        .trim()
        .to_string();
    Ok(uname)
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    require_root()?;

    let cfg = AppConfig::load(&cli.config)?;
    let runner = RealCommandRunner::new(cli.dry_run);

    match &cli.command {
        Commands::Generate(args) => {
            let uname = current_kernel(&runner)?;
            let settings = resolve_generate_settings(&cfg, args, &uname);
            let _ = generate(&runner, &cfg, &settings, args.boot_once)?;
        }
        Commands::Install(args) => {
            let uname = current_kernel(&runner)?;
            let settings = resolve_generate_settings(&cfg, args, &uname);
            let _ = install(&runner, &cfg, &settings)?;
        }
        Commands::Reconcile => {
            let uname = current_kernel(&runner)?;
            let settings = resolve_generate_settings(
                &cfg,
                &cli::GenerateArgs {
                    kernel_version: Some(uname),
                    esp_path: None,
                    output_dir: None,
                    cmdline_file: None,
                    splash: None,
                    os_release: None,
                    boot_once: false,
                },
                "",
            );
            reconcile(&runner, &cfg, &settings)?;
        }
        Commands::InstallHook(args) => {
            let binary = args.binary_path.clone().unwrap_or_else(|| {
                std::env::current_exe()
                    .unwrap_or_else(|_| std::path::PathBuf::from("/usr/local/bin/rustyuki"))
            });
            install_kernel_update_hook(&binary, &cli.config, &args.plugin_path)?;
            println!(
                "installed kernel-install hook at {}",
                args.plugin_path.display()
            );
        }
        Commands::Status => {
            let text = status(&runner, &cfg)?;
            println!("{text}");
        }
        Commands::Confirm => {
            let boot_num = confirm(&runner)?;
            println!("confirmed Boot{boot_num} as the permanent default boot entry");
        }
    }

    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
