use crate::cmd::CommandRunner;
use anyhow::{Context, Result};
use log::info;
use std::fs;
use std::path::{Path, PathBuf};

/// Parameters for UKI assembly.
pub struct UkifyParams<'a> {
    /// Kernel image path.
    pub kernel_image: &'a Path,
    /// Initramfs image path.
    pub initramfs_image: &'a Path,
    /// Command line string.
    pub cmdline: &'a str,
    /// `os-release` path.
    pub os_release: &'a Path,
    /// Optional splash image path.
    pub splash: Option<&'a Path>,
    /// Final output path.
    pub output: &'a Path,
    /// Extra ukify arguments.
    pub extra_args: &'a [String],
}

/// Builds a UKI through `ukify build`, using temp output and atomic rename.
pub fn build_uki(runner: &dyn CommandRunner, params: &UkifyParams<'_>) -> Result<PathBuf> {
    let parent = params
        .output
        .parent()
        .with_context(|| format!("output has no parent: {}", params.output.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed creating output directory {}", parent.display()))?;

    let file_name = params
        .output
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("invalid output filename {}", params.output.display()))?;

    let temp_output = parent.join(format!(".{file_name}.tmp"));
    if temp_output.exists() {
        let _ = fs::remove_file(&temp_output);
    }

    info!(
        "Stage 2/2: assembling UKI {} -> {}",
        params.kernel_image.display(),
        params.output.display()
    );

    let kernel = params
        .kernel_image
        .to_str()
        .with_context(|| format!("kernel path is non-UTF8: {}", params.kernel_image.display()))?;
    let initrd = params.initramfs_image.to_str().with_context(|| {
        format!(
            "initramfs path is non-UTF8: {}",
            params.initramfs_image.display()
        )
    })?;
    let os_release = params.os_release.to_str().with_context(|| {
        format!(
            "os-release path is non-UTF8: {}",
            params.os_release.display()
        )
    })?;
    let temp_out = temp_output
        .to_str()
        .with_context(|| format!("temp output path is non-UTF8: {}", temp_output.display()))?;

    let mut args = vec![
        "build".to_string(),
        "--linux".to_string(),
        kernel.to_string(),
        "--initrd".to_string(),
        initrd.to_string(),
        "--cmdline".to_string(),
        params.cmdline.to_string(),
        "--os-release".to_string(),
        os_release.to_string(),
        "--output".to_string(),
        temp_out.to_string(),
    ];

    if let Some(splash_path) = params.splash {
        let splash = splash_path
            .to_str()
            .with_context(|| format!("splash path is non-UTF8: {}", splash_path.display()))?;
        args.push("--splash".to_string());
        args.push(splash.to_string());
    }

    args.extend(params.extra_args.iter().cloned());
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();

    let run_result = runner.run("ukify", &refs);
    if let Err(err) = run_result {
        let _ = fs::remove_file(&temp_output);
        return Err(err).context("ukify invocation failed");
    }

    fs::rename(&temp_output, params.output).with_context(|| {
        format!(
            "failed to atomically move {} -> {}",
            temp_output.display(),
            params.output.display()
        )
    })?;

    Ok(params.output.to_path_buf())
}
