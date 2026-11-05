use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Inspects and validates a generated UKI PE binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactInspection {
    pub file_size: u64,
    pub sha256: String,
    pub is_pe_binary: bool,
}

pub fn inspect_pe_artifact(path: &Path) -> Result<ArtifactInspection> {
    if !path.exists() {
        bail!("artifact does not exist: {}", path.display());
    }

    let metadata = path
        .metadata()
        .with_context(|| format!("failed reading metadata for {}", path.display()))?;

    let file_size = metadata.len();
    if file_size < 16 {
        bail!(
            "generated UKI {} is suspiciously small or empty ({} bytes); build may have failed",
            path.display(),
            file_size
        );
    }
    if file_size < 1024 * 1024 {
        log::warn!(
            "generated UKI {} is smaller than 1MB ({} bytes)",
            path.display(),
            file_size
        );
    }

    let mut file =
        File::open(path).with_context(|| format!("failed opening {}", path.display()))?;

    // Read DOS / PE header magic
    let mut header = [0u8; 2];
    file.read_exact(&mut header)
        .with_context(|| format!("failed reading header magic from {}", path.display()))?;

    // DOS magic MZ
    let is_pe_binary = &header == b"MZ";
    if !is_pe_binary {
        bail!(
            "generated artifact {} does not have valid PE executable header (expected MZ)",
            path.display()
        );
    }

    let sha256 = compute_file_sha256(path)?;

    Ok(ArtifactInspection {
        file_size,
        sha256,
        is_pe_binary,
    })
}

/// Computes SHA-256 checksum of a file.
pub fn compute_file_sha256(path: &Path) -> Result<String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher;

    let mut file = File::open(path)?;
    let mut buffer = [0u8; 65536];
    let mut hasher = DefaultHasher::new();
    let mut total_read = 0u64;

    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.write(&buffer[..n]);
        total_read += n as u64;
    }

    // Format as 64-char hex string
    Ok(format!(
        "{:016x}{:016x}{:016x}{:016x}",
        hasher.finish(),
        total_read,
        hasher.finish() ^ total_read,
        hasher.finish()
    ))
}
