use crate::cmd::CommandRunner;
pub use crate::config::SigningConfig;
use anyhow::{bail, Context, Result};
use log::info;
use std::fs;
use std::path::{Path, PathBuf};

/// Signs a generated UKI PE binary if Secure Boot signing is enabled.
pub fn sign_uki_if_configured(
    runner: &dyn CommandRunner,
    uki_path: &Path,
    config: &SigningConfig,
) -> Result<PathBuf> {
    if !config.enabled {
        return Ok(uki_path.to_path_buf());
    }

    info!(
        "Stage 3: signing UKI {} with {}",
        uki_path.display(),
        config.tool
    );

    let signed_path = uki_path.with_extension("signed.efi");

    match config.tool.to_ascii_lowercase().as_str() {
        "pesign" => {
            let cert_token = config.token.as_deref().unwrap_or("auto");

            let in_str = uki_path.to_str().context("invalid UTF-8 path")?;
            let out_str = signed_path.to_str().context("invalid UTF-8 path")?;

            runner
                .run(
                    "pesign",
                    &["-s", "-i", in_str, "-o", out_str, "-c", cert_token, "-f"],
                )
                .context("pesign invocation failed")?;
        }
        "sbsign" => {
            let key = config
                .key_path
                .as_ref()
                .context("sbsign requires signing.key_path to be set")?;
            let cert = config
                .cert_path
                .as_ref()
                .context("sbsign requires signing.cert_path to be set")?;

            let in_str = uki_path.to_str().context("invalid UTF-8 path")?;
            let out_str = signed_path.to_str().context("invalid UTF-8 path")?;
            let key_str = key.to_str().context("invalid UTF-8 path")?;
            let cert_str = cert.to_str().context("invalid UTF-8 path")?;

            runner
                .run(
                    "sbsign",
                    &[
                        "--key", key_str, "--cert", cert_str, "--output", out_str, in_str,
                    ],
                )
                .context("sbsign invocation failed")?;
        }
        "none" | "" => {
            return Ok(uki_path.to_path_buf());
        }
        other => {
            bail!(
                "unsupported signing tool '{other}'; supported options are 'pesign' and 'sbsign'"
            );
        }
    }

    if signed_path.exists() {
        fs::rename(&signed_path, uki_path).with_context(|| {
            format!(
                "failed replacing unsigned UKI with signed binary at {}",
                uki_path.display()
            )
        })?;
    }

    Ok(uki_path.to_path_buf())
}
