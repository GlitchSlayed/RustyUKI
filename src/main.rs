#![deny(unused_must_use)]

mod app;
mod cli;
mod cmd;
mod config;
mod dracut;
mod efi;
mod error;
mod privilege;
mod ukify;

use anyhow::{Context, Result};
use app::{generate, install, resolve_generate_settings, status};
use clap::Parser;
use cli::{Cli, Commands};
use cmd::{CommandRunner, RealCommandRunner};
use config::AppConfig;
use env_logger::Env;
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
            let _ = generate(&runner, &cfg, &settings)?;
        }
        Commands::Install(args) => {
            let uname = current_kernel(&runner)?;
            let settings = resolve_generate_settings(&cfg, args, &uname);
            let _ = install(&runner, &cfg, &settings)?;
        }
        Commands::Status => {
            let text = status(&runner, &cfg)?;
            println!("{text}");
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
