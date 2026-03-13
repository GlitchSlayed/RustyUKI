use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

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

pub fn render_kernel_install_plugin(binary: &Path, config: &Path) -> Result<String> {
    let bin = binary
        .to_str()
        .with_context(|| format!("binary path is non-UTF8: {}", binary.display()))?;
    let cfg = config
        .to_str()
        .with_context(|| format!("config path is non-UTF8: {}", config.display()))?;

    Ok(format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

COMMAND="${{1:-}}"
KERNEL_VER="${{2:-unknown}}"
RUSTYUKI_BIN="{bin}"
RUSTYUKI_CONFIG="{cfg}"

log() {{
  echo "[rustyuki-hook] $*" >&2
}}

if [[ ! -x "$RUSTYUKI_BIN" ]]; then
  log "RustyUKI binary not executable: $RUSTYUKI_BIN"
  exit 1
fi

case "$COMMAND" in
  add)
    log "kernel add: $KERNEL_VER; reconciling all installed kernels"
    ;;
  remove)
    log "kernel remove: $KERNEL_VER; reconciling all installed kernels"
    ;;
  *)
    log "kernel command '$COMMAND'; running reconcile"
    ;;
esac

exec "$RUSTYUKI_BIN" --config "$RUSTYUKI_CONFIG" reconcile
"#
    ))
}

#[cfg(test)]
mod tests {
    use super::render_kernel_install_plugin;
    use std::path::Path;

    #[test]
    fn plugin_contains_reconcile_and_config() {
        let script = render_kernel_install_plugin(
            Path::new("/usr/local/bin/rustyuki"),
            Path::new("/etc/uki/uki.conf"),
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(script.contains("reconcile"));
        assert!(script.contains("--config \"$RUSTYUKI_CONFIG\""));
        assert!(script.contains("KERNEL_VER"));
    }
}
