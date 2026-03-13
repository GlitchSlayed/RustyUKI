use anyhow::{bail, Result};
use nix::unistd::getuid;

/// Ensures program is executed as root.
pub fn require_root() -> Result<()> {
    if getuid().is_root() {
        Ok(())
    } else {
        bail!("this operation requires root privileges; rerun with sudo")
    }
}
