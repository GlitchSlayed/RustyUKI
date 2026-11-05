use anyhow::{bail, Result};
use nix::unistd::getuid;

/// Ensures program is executed with root privileges when required.
pub fn require_root() -> Result<()> {
    if getuid().is_root() {
        Ok(())
    } else {
        bail!("this operation requires root privileges; rerun with sudo")
    }
}

/// Permissive check: requires root only when running on live host without sysroot.
pub fn require_root_if_live(is_offline: bool) -> Result<()> {
    if is_offline || getuid().is_root() {
        Ok(())
    } else {
        bail!("modifying live system boot configuration requires root privileges; rerun with sudo")
    }
}
