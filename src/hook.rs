use crate::assessment::render_boot_confirm_service;
use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Installs the kernel-install plugin into the system.
pub fn install_kernel_update_hook(binary: &Path, config: &Path, plugin_path: &Path) -> Result<()> {
    let parent = plugin_path.parent().with_context(|| {
        format!(
            "kernel-install plugin path has no parent: {}",
            plugin_path.display()
        )
    })?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed creating plugin directory {}", parent.display()))?;

    let script = render_kernel_install_plugin(binary, config)?;
    fs::write(plugin_path, script)
        .with_context(|| format!("failed writing plugin {}", plugin_path.display()))?;

    let mut perms = fs::metadata(plugin_path)
        .with_context(|| format!("failed stat {}", plugin_path.display()))?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(plugin_path, perms)
        .with_context(|| format!("failed chmod +x {}", plugin_path.display()))?;

    Ok(())
}

/// Renders the bash plugin script for kernel-install.
pub fn render_kernel_install_plugin(binary: &Path, config: &Path) -> Result<String> {
    let bin = binary
        .to_str()
        .with_context(|| format!("binary path is non-UTF8: {}", binary.display()))?;
    let cfg = config
        .to_str()
        .with_context(|| format!("config path is non-UTF8: {}", config.display()))?;

    Ok(format!(
        r#"#!/usr/bin/env bash
set -uo pipefail

COMMAND="${{1:-}}"
KERNEL_VER="${{2:-unknown}}"
ENTRY_DIR_ABS="${{3:-}}"
KERNEL_IMAGE="${{4:-}}"

RUSTYUKI_BIN="{bin}"
RUSTYUKI_CONFIG="{cfg}"
STATE_DIR="/var/lib/uki-build/pending-failures"

log() {{
  echo "[rustyuki-hook] $*" >&2
}}

if [[ ! -x "$RUSTYUKI_BIN" ]]; then
  log "RustyUKI binary not executable: $RUSTYUKI_BIN"
  exit 0
fi

case "$COMMAND" in
  add)
    log "kernel add: $KERNEL_VER — generating and deploying UKI"
    if ! "$RUSTYUKI_BIN" --config "$RUSTYUKI_CONFIG" install --kernel-version "$KERNEL_VER"; then
      log "warning: UKI build failed for kernel $KERNEL_VER; logging to failure registry"
      mkdir -p "$STATE_DIR"
      echo "Failed at $(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$STATE_DIR/$KERNEL_VER.failed"
    else
      rm -f "$STATE_DIR/$KERNEL_VER.failed" 2>/dev/null || true
    fi
    ;;
  remove)
    log "kernel remove: $KERNEL_VER — reconciling remaining UKIs"
    if ! "$RUSTYUKI_BIN" --config "$RUSTYUKI_CONFIG" reconcile; then
      log "warning: reconcile failed after removing kernel $KERNEL_VER; logging to failure registry"
      mkdir -p "$STATE_DIR"
      echo "Reconcile failure after removing $KERNEL_VER at $(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$STATE_DIR/reconcile.failed"
    fi
    rm -f "$STATE_DIR/$KERNEL_VER.failed" 2>/dev/null || true
    ;;
  *)
    log "unknown command '$COMMAND' — skipping"
    ;;
esac

# Always exit 0 to protect DNF/RPM package transaction integrity
exit 0
"#
    ))
}

/// Installs the systemd boot confirmation service unit.
pub fn install_boot_confirm_service(unit_path: &Path, binary: &Path) -> Result<()> {
    let parent = unit_path.parent().with_context(|| {
        format!(
            "systemd unit path has no parent directory: {}",
            unit_path.display()
        )
    })?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed creating unit directory {}", parent.display()))?;

    let unit = render_boot_confirm_service(binary)?;
    fs::write(unit_path, unit)
        .with_context(|| format!("failed writing unit {}", unit_path.display()))?;

    let mut perms = fs::metadata(unit_path)
        .with_context(|| format!("failed stat {}", unit_path.display()))?
        .permissions();
    perms.set_mode(0o644);
    fs::set_permissions(unit_path, perms)
        .with_context(|| format!("failed chmod 0644 {}", unit_path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{render_boot_confirm_service, render_kernel_install_plugin};
    use std::path::Path;

    #[test]
    fn plugin_contains_failure_registry_and_config() {
        let script = render_kernel_install_plugin(
            Path::new("/usr/local/bin/rustyuki"),
            Path::new("/etc/uki/uki.conf"),
        )
        .unwrap();

        assert!(script.contains("pending-failures"));
        assert!(script.contains("--config \"$RUSTYUKI_CONFIG\""));
        assert!(script.contains("KERNEL_VER"));
        assert!(script.contains("install --kernel-version"));
    }

    #[test]
    fn boot_confirm_service_has_safe_ordering() {
        let service = render_boot_confirm_service(Path::new("/usr/local/bin/rustyuki")).unwrap();
        assert!(service.contains("After=multi-user.target"));
        assert!(!service.contains("Before=network.target"));
    }
}
