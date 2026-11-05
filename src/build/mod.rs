pub mod dracut;
pub mod inspect;
pub mod signing;
pub mod ukify;

use crate::cmd::CommandRunner;
use crate::config::AppConfig;
use crate::model::{KernelFlavor, UkiArtifact};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Input parameters for generating a UKI.
pub struct GeneratePipelineParams<'a> {
    pub kernel_version: &'a str,
    pub kernel_image: &'a Path,
    pub cmdline: &'a str,
    pub os_release: &'a Path,
    pub splash: Option<&'a Path>,
    pub staging_dir: &'a Path,
    pub flavor: KernelFlavor,
    pub sysroot: Option<&'a Path>,
}

/// Executes the isolated UKI generation pipeline:
/// 1. Compiles initramfs with dracut into temporary storage.
/// 2. Assembles UKI PE binary with ukify into staging directory.
/// 3. Signs PE binary if Secure Boot signing is enabled.
/// 4. Inspects and validates generated binary (verifies MZ header, size sanity, computes SHA-256).
pub fn generate_uki(
    runner: &dyn CommandRunner,
    cfg: &AppConfig,
    params: &GeneratePipelineParams<'_>,
) -> Result<UkiArtifact> {
    fs::create_dir_all(params.staging_dir).with_context(|| {
        format!(
            "failed creating staging directory {}",
            params.staging_dir.display()
        )
    })?;

    let initramfs = params
        .staging_dir
        .join(format!("initramfs-{}.img", params.kernel_version));

    let _built_initramfs = dracut::build_initramfs(
        runner,
        params.kernel_version,
        &initramfs,
        &cfg.dracut.extra_args,
        params.sysroot,
    )?;

    let staged_uki = params
        .staging_dir
        .join(format!("linux-{}.efi", params.kernel_version));

    let ukify_params = ukify::UkifyParams {
        kernel_image: params.kernel_image,
        initramfs_image: &initramfs,
        cmdline: params.cmdline,
        os_release: params.os_release,
        splash: params.splash,
        output: &staged_uki,
        extra_args: &cfg.ukify.extra_args,
    };

    let assembled = ukify::assemble_uki(runner, &ukify_params)?;

    // Optional Secure Boot signing
    let final_staged = signing::sign_uki_if_configured(runner, &assembled, &cfg.signing)?;

    // Inspect artifact to ensure valid PE binary and compute cryptographic hash
    let inspection = inspect::inspect_pe_artifact(&final_staged)?;

    // Clean up temporary initramfs from staging
    if initramfs.exists() {
        let _ = fs::remove_file(&initramfs);
    }

    // Compute cmdline hash for provenance tracking
    let cmdline_hash = compute_string_hash(params.cmdline);

    Ok(UkiArtifact {
        kernel_version: params.kernel_version.to_string(),
        path: final_staged,
        sha256: inspection.sha256,
        size_bytes: inspection.file_size,
        cmdline_hash,
        flavor: params.flavor.clone(),
    })
}

fn compute_string_hash(s: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
