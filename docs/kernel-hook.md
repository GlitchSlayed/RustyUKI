# Automated Kernel Updates & Hooks

RustyUKI integrates with Fedora's standard `kernel-install` framework to automate UKI lifecycle management. When enabled, kernel package transactions (`dnf update kernel`, `dnf remove kernel`) build, update, or prune UKIs automatically.

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
  --binary-path /usr/bin/rustyuki \
  --config /etc/rustyuki/config.toml
```

> [!NOTE]
> Scripts in `/etc/kernel/install.d/` take precedence over scripts with the same filename in `/usr/lib/kernel/install.d/`.

---

## Failure Visibility and Recovery Architecture

In standard package manager workflows (`dnf`, `rpm`), failing a transaction with a non-zero exit code during post-install scriptlets can leave the RPM database corrupted or leave the kernel half-installed. Conversely, silently ignoring failures causes the system to boot into an outdated or missing kernel without the administrator knowing.

RustyUKI resolves this dilemma with **explicit persistent failure tracking**:

1. **Failure Registry Directory**:
   When UKI generation fails inside `kernel-install`, the hook records the failure into:
   ```
   /var/lib/uki-build/pending-failures/<kernel-version>.failed
   ```
   This file records the failure timestamp, kernel release, and diagnostic cause.

2. **Transaction Stability**:
   The scriptlet exits cleanly (`0`) so that RPM finishes package installation safely without aborting other scriptlets.

3. **Retryable Reconciliation**:
   Administrators and monitoring services can query `rustyuki status`, which flags pending build failures in red. Running `sudo rustyuki reconcile` automatically attempts to re-build any pending failed UKIs once underlying issues (e.g. disk space or missing packages) are resolved.

4. **Failure Clearance**:
   Upon a successful build or when a failed kernel is removed, the `.failed` marker is automatically removed.

---

## Plugin Script Implementation

The generated plugin script is structured as follows:

```bash
#!/usr/bin/env bash
set -uo pipefail

COMMAND="${1:-}"
KERNEL_VER="${2:-unknown}"
ENTRY_DIR_ABS="${3:-}"
KERNEL_IMAGE="${4:-}"

RUSTYUKI_BIN="/usr/bin/rustyuki"
CONFIG_FILE="/etc/rustyuki/config.toml"
FAILURE_LOG_DIR="/var/lib/uki-build/pending-failures"

if [[ ! -x "${RUSTYUKI_BIN}" ]]; then
    echo "rustyuki: binary not found at ${RUSTYUKI_BIN}, skipping"
    exit 0
fi

mkdir -p "${FAILURE_LOG_DIR}"

case "${COMMAND}" in
    add)
        echo "rustyuki: generating unified kernel image for ${KERNEL_VER}..."
        if ! "${RUSTYUKI_BIN}" --config "${CONFIG_FILE}" generate --kernel "${KERNEL_VER}"; then
            echo "ERROR: rustyuki failed generating UKI for ${KERNEL_VER}" >&2
            echo "$(date -Iseconds): generate failed for ${KERNEL_VER}" > "${FAILURE_LOG_DIR}/${KERNEL_VER}.failed"
            exit 0
        fi
        rm -f "${FAILURE_LOG_DIR}/${KERNEL_VER}.failed"
        ;;
    remove)
        echo "rustyuki: reconciling UKI images following removal of ${KERNEL_VER}..."
        "${RUSTYUKI_BIN}" --config "${CONFIG_FILE}" reconcile || true
        rm -f "${FAILURE_LOG_DIR}/${KERNEL_VER}.failed"
        ;;
    *)
        exit 0
        ;;
esac
```

---

## Lifecycle Triggers

### 1. Kernel Installation (`add <kernel-version>`)

When a new kernel package is installed:
1. `kernel-install` invokes `90-rustyuki.install add <version> ...`.
2. The hook invokes `rustyuki generate --kernel <version>`.
3. RustyUKI builds the isolated initramfs and UKI for this specific kernel in staging, verifies the PE executable, durable-installs to `/EFI/Linux/linux-<version>.efi`, preserves `.prev.efi`, writes `.manifest.json`, and registers the boot entry.

### 2. Kernel Removal (`remove <kernel-version>`)

When an old kernel package is removed during system updates:
1. `kernel-install` invokes `90-rustyuki.install remove <version> ...`.
2. The hook executes `rustyuki reconcile`.
3. `reconcile` queries installed kernels across all flavors (Fedora, CachyOS, custom RPMs), verifies active kernels, removes orphaned `linux-<version>.efi` files that carry RustyUKI manifests, and keeps foreign UKIs intact.
