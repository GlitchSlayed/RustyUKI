use anyhow::{Context, Result};
use clap::Parser;
use env_logger::Env;
use log::LevelFilter;
use rustyuki::app::{
    confirm_backend, generate, install, reconcile, resolve_generate_settings, rollback, status,
};
use rustyuki::backend::create_backend;
use rustyuki::cli::{Cli, Commands, GenerateArgs};
use rustyuki::cmd::{CommandRunner, RealCommandRunner};
use rustyuki::config::AppConfig;
use rustyuki::hook::install_kernel_update_hook;
use rustyuki::install;
use rustyuki::privilege::require_root_if_live;

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

    let mut cfg = AppConfig::load(&cli.config)?;

    // Apply global CLI overrides to config
    if let Some(sysroot) = cli.sysroot.clone() {
        cfg.uki.sysroot = Some(sysroot);
    }
    if let Some(backend) = cli.backend {
        cfg.bootloader.backend = backend;
    }

    let is_offline = cfg.uki.sysroot.is_some();
    let runner = RealCommandRunner::new(cli.dry_run);

    match &cli.command {
        Commands::Generate(args) => {
            require_root_if_live(is_offline)?;
            let uname = current_kernel(&runner).unwrap_or_default();
            let settings = resolve_generate_settings(&cfg, args, &uname);
            let (path, entry) = generate(&runner, &cfg, &settings, args.boot_once)?;
            println!("generated UKI: {} (entry {entry})", path.display());
        }
        Commands::Install(args) => {
            require_root_if_live(is_offline)?;
            let uname = current_kernel(&runner).unwrap_or_default();
            let settings = resolve_generate_settings(&cfg, args, &uname);
            let path = install(&runner, &cfg, &settings)?;
            println!("installed UKI: {}", path.display());
        }
        Commands::Activate(args) => {
            require_root_if_live(is_offline)?;
            let uname = current_kernel(&runner).unwrap_or_default();
            let target_version = args.kernel_version.as_deref().unwrap_or(&uname);
            let output_dir = cfg.effective_output_dir();
            let esp_path = cfg.effective_esp_path();

            let installed = install::list_installed_ukis(&output_dir)?;
            let target_uki = installed
                .into_iter()
                .find(|u| u.kernel_version == target_version)
                .with_context(|| {
                    format!("no installed UKI found for kernel version {target_version}")
                })?;

            let backend = create_backend(args.backend.unwrap_or(cfg.bootloader.backend));
            let res = backend.activate(&runner, &target_uki, &esp_path, args.trial, is_offline)?;
            println!("activated boot entry: {}", res.description);
        }
        Commands::Rollback(args) => {
            require_root_if_live(is_offline)?;
            let uname = current_kernel(&runner).unwrap_or_default();
            let target_version = args.kernel_version.as_deref().unwrap_or(&uname);
            let restored = rollback(&cfg, target_version)?;
            println!(
                "rolled back UKI for {target_version} -> {}",
                restored.display()
            );
        }
        Commands::Reconcile => {
            require_root_if_live(is_offline)?;
            let uname = current_kernel(&runner).unwrap_or_default();
            let settings = resolve_generate_settings(
                &cfg,
                &GenerateArgs {
                    kernel_version: Some(uname),
                    esp_path: None,
                    output_dir: None,
                    cmdline_file: None,
                    splash: None,
                    os_release: None,
                    boot_once: false,
                    sysroot: None,
                    backend: None,
                },
                "",
            );
            reconcile(&runner, &cfg, &settings)?;
            println!("reconcile complete");
        }
        Commands::InstallHook(args) => {
            require_root_if_live(is_offline)?;
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
        Commands::Confirm(args) => {
            require_root_if_live(false)?;
            let boot_num = confirm_backend(&runner, cfg.bootloader.backend, args.force)?;
            println!("confirmed {boot_num} as the permanent default boot entry");
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
