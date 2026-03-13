use crate::cmd::CommandRunner;
use anyhow::{Context, Result};
use log::info;
use std::path::{Path, PathBuf};

/// Builds an initramfs-only artifact via dracut.
pub fn build_initramfs(
    runner: &dyn CommandRunner,
    kernel_version: &str,
    out_path: &Path,
    extra_args: &[String],
) -> Result<PathBuf> {
    info!(
        "Stage 1/2: building initramfs for kernel {} -> {}",
        kernel_version,
        out_path.display()
    );

    let out = out_path
        .to_str()
        .with_context(|| format!("non-UTF8 initramfs path {}", out_path.display()))?;

    let mut args = vec![
        "-f".to_string(),
        out.to_string(),
        kernel_version.to_string(),
    ];
    args.extend(extra_args.iter().cloned());
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();

    runner
        .run("dracut", &refs)
        .context("dracut invocation failed")?;

    Ok(out_path.to_path_buf())
}
