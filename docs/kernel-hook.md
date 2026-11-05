# Automated Kernel Updates & Hooks

RustyUKI integrates with Fedora's standard `kernel-install` framework to automate UKI lifecycle management. When enabled, kernel package transactions (`dnf update kernel`, `dnf remove kernel`) rebuild or prune UKIs automatically.

---

## Installation

To activate automatic UKI updates, run:

```bash
sudo rustyuki install-hook
```

This creates an executable plugin script at:
```
/usr/lib/kernel/install.d/90-rustyuki.install
```

### Custom Hook Paths

You can override the target location or the binary path called by the hook:

```bash
sudo rustyuki install-hook \
  --plugin-path /etc/kernel/install.d/90-rustyuki.install \
  --binary-path /usr/local/bin/rustyuki
```

> [!NOTE]
> Scripts in `/etc/kernel/install.d/` take precedence over scripts with the same filename in `/usr/lib/kernel/install.d/`.

---

## Plugin Script Implementation

The generated plugin script is rendered as follows:

```bash
#!/usr/bin/env bash
set -uo pipefail

COMMAND="${1:-}"
KERNEL_VER="${2:-unknown}"
RUSTYUKI_BIN="/usr/local/bin/rustyuki"
RUSTYUKI_CONFIG="/etc/uki/uki.conf"

log() {
  echo "[rustyuki-hook] $*" >&2
}

if [[ ! -x "$RUSTYUKI_BIN" ]]; then
  log "RustyUKI binary not executable: $RUSTYUKI_BIN"
  exit 0
fi

case "$COMMAND" in
  add)
    log "kernel add: $KERNEL_VER — building UKI for this kernel only"
    if ! "$RUSTYUKI_BIN" --config "$RUSTYUKI_CONFIG" generate --kernel-version "$KERNEL_VER"; then
      log "warning: generate failed for kernel $KERNEL_VER; leaving transaction successful"
    fi
    ;;
  remove)
    log "kernel remove: $KERNEL_VER — reconciling all installed kernels"
    if ! "$RUSTYUKI_BIN" --config "$RUSTYUKI_CONFIG" reconcile; then
      log "warning: reconcile failed after removing kernel $KERNEL_VER; leaving transaction successful"
    fi
    ;;
  *)
    log "unknown command '$COMMAND' — skipping"
    ;;
esac

exit 0
```

---

## Lifecycle Triggers

### 1. Kernel Installation (`add <kernel-version> <kernel-image-dir>`)

When a new kernel RPM is installed:
1. `kernel-install` invokes `90-rustyuki.install add <version> ...`.
2. The hook executes:
   ```bash
   rustyuki --config /etc/uki/uki.conf generate --kernel-version <version>
   ```
3. RustyUKI builds the initramfs and UKI specifically for this new kernel version, registers an EFI entry, and leaves existing UKIs intact.

### 2. Kernel Removal (`remove <kernel-version> <kernel-image-dir>`)

When an old kernel RPM is removed during system updates:
1. `kernel-install` invokes `90-rustyuki.install remove <version> ...`.
2. The hook executes:
   ```bash
   rustyuki --config /etc/uki/uki.conf reconcile
   ```
3. `reconcile` queries installed kernels via `rpm -q kernel`, verifies `/lib/modules/<version>/vmlinuz`, removes orphaned `linux-<version>.efi` files from the ESP output directory, and runs `bootctl update`.

---

## Package Manager Transaction Safety

Package managers like `dnf` execute hooks during transaction scriptlets. If a third-party hook exits with a non-zero exit code, `dnf` may abort or mark the package transaction as failed, potentially leaving the RPM database or package dependencies in a broken state.

To guarantee package manager stability:
- The hook logs warnings and failures to standard error (`echo "[rustyuki-hook] warning..." >&2`).
- The script always exits with code `0`.
- If the RustyUKI binary is missing or non-executable, the script exits silently with code `0`.
