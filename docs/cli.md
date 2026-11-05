# RustyUKI CLI Reference

RustyUKI is a command-line utility for managing Unified Kernel Images. This reference covers all global options, subcommands, arguments, exit codes, and operational examples.

---

## Command Syntax

```bash
rustyuki [GLOBAL OPTIONS] <SUBCOMMAND> [SUBCOMMAND OPTIONS]
```

> [!NOTE]
> RustyUKI operations typically require root privileges because they interact with the EFI System Partition, `/proc`, `/lib/modules`, and UEFI NVRAM. Run commands with `sudo`. Offline operations with `--sysroot` pointing to an unprivileged directory or image root do not require root.

---

## Global Options

The following flags apply to all subcommands:

| Option | Type | Default | Description |
|---|---|---|---|
| `--config <PATH>` | File path | `/etc/rustyuki/config.toml` (fallback `/etc/uki/uki.conf`) | Path to the TOML configuration file. If the file does not exist, internal defaults are used. |
| `--dry-run` | Flag | `false` | Log all external commands (`dracut`, `ukify`, `efibootmgr`, `bootctl`, `systemctl`) without executing them or modifying the filesystem. |
| `--sysroot <PATH>` | Directory path | None (live host `"/"`) | Target sysroot directory for offline installation and image-building. Completely disables host NVRAM access. |
| `--backend <TYPE>` | Enum (`direct`, `systemd-boot`) | Config default | Override bootloader management backend. |
| `-v` | Flag | `INFO` | Enable `DEBUG` level logging. |
| `-vv` | Flag | `INFO` | Enable `TRACE` level logging for granular troubleshooting. |
| `-h`, `--help` | Flag | — | Print command-line help information. |
| `-V`, `--version` | Flag | — | Print version information. |

---

## Subcommands

### 1. `status`

Inspects and displays the resolved runtime configuration, active boot backend, discovered kernels, and installed UKIs on the ESP.

```bash
sudo rustyuki status
```

**Output Fields:**
- `kernel`: Running kernel version reported by `uname -r`.
- `backend`: Selected boot backend (`direct` or `systemd-boot`).
- `esp_path`: Active EFI System Partition mountpoint.
- `output_dir`: Directory where UKI binaries are placed (`/EFI/Linux`).
- `cmdline_file`: Path to the active kernel command line file.
- **GPT autodiscovery advisory**: Advises if your root partition carries a Discoverable Partitions Specification (DPS) GPT type GUID and whether `root=` is redundant or missing in your cmdline.
- `os_release`: Path to the `os-release` file used for UKI metadata.
- **Bootloader status**: Current NVRAM boot entries or `bootctl status`.
- **Installed UKIs**: Lists installed UKIs, their provenance status (RustyUKI managed or foreign/unmanaged), and whether a `.prev.efi` fallback slot exists.

---

### 2. `generate`

Executes Stage 1 UKI generation in isolated staging storage without modifying the ESP or firmware NVRAM state:
1. Validates ESP preflight capacity and permissions.
2. Resolves target kernel (supporting Fedora and CachyOS variants).
3. Generates the initramfs with `dracut` in staging.
4. Resolves, sanitizes, and verifies the kernel cmdline.
5. Assembles the UKI PE binary with `ukify`.
6. Performs Secure Boot signing (if configured with `pesign` or `sbsign`).
7. Inspects and validates PE header magic (`MZ`) and cryptographic SHA-256 hash.

```bash
sudo rustyuki generate [OPTIONS]
```

**Options:**

| Flag | Argument | Default | Description |
|---|---|---|---|
| `--kernel-version` | String | `uname -r` or config | Target kernel version (must have `/lib/modules/<ver>/vmlinuz`). |
| `--esp-path` | Directory path | Config `/efi` | Mountpoint of the EFI System Partition. |
| `--output-dir` | Directory path | Config `/efi/EFI/Linux` | Target directory for the final `.efi` binary. |
| `--cmdline-file` | File path | Config `/etc/kernel/cmdline` | File to read kernel parameters from. |
| `--splash` | File path | Config or none | Path to a BMP splash logo embedded in the UKI. |
| `--os-release` | File path | Config `/usr/lib/os-release` | OS metadata file embedded into the `.osrel` PE section. |
| `--boot-once` | Flag | `false` | Arms a one-time trial boot after generation. |
| `--sysroot` | Directory path | None | Target root directory for offline generation. |
| `--backend` | Enum | Config | Backend override (`direct` or `systemd-boot`). |

---

### 3. `install`

Executes the complete installation pipeline:
1. Generates and validates the UKI binary in staging.
2. Performs durable installation onto the ESP (syncing file data, preserving existing UKI in `.prev.efi` fallback slot, atomic FAT rename, syncing parent directory).
3. Writes sidecar provenance metadata (`.manifest.json`).
4. Installs and enables `rustyuki-boot-confirm.service`.
5. Arms trial boot (`BootNext` for direct UEFI, or `bootctl set-oneshot` for systemd-boot).

```bash
sudo rustyuki install [OPTIONS]
```

---

### 4. `activate`

Manually activates a bootloader backend entry for an already-installed UKI:

```bash
sudo rustyuki activate --kernel-version 6.17.4-100.fc44.x86_64 [--boot-once]
```

**Options:**
- `--kernel-version <VERSION>`: Kernel version to activate.
- `--boot-once`: Sets as a one-time trial instead of changing permanent default.
- `--sysroot <PATH>`: Target sysroot for offline activation.
- `--backend <direct|systemd-boot>`: Backend override.

---

### 5. `rollback`

Restores the previous known-working UKI from the `.prev.efi` fallback slot:

```bash
sudo rustyuki rollback --kernel-version 6.17.4-100.fc44.x86_64
```

Restores `linux-<version>.prev.efi` back to `linux-<version>.efi` atomically without recompiling.

---

### 6. `reconcile`

Reconciles installed kernels against UKIs present on the ESP:
1. Queries installed kernels across Fedora stock (`kernel-core`), CachyOS (`kernel-cachyos-lto`), and custom RPM variants.
2. Generates missing UKIs for installed kernels.
3. Prunes stale UKIs whose kernel packages have been uninstalled.
4. **Safety Check**: Checks `.manifest.json` before deleting any file. Never deletes foreign/unmanaged UKIs or the currently running kernel.

```bash
sudo rustyuki reconcile
```

---

### 7. `install-hook`

Installs the `kernel-install` plugin script into `/usr/lib/kernel/install.d/90-rustyuki.install`:

```bash
sudo rustyuki install-hook
```

---

### 8. `confirm`

Evaluates system health via `systemctl is-system-running` and promotes the trial boot to the permanent default:
- If system is `running` or `degraded`, updates `BootOrder` (direct UEFI) or runs `systemd-bless-boot good` (systemd-boot).
- If system is in `emergency`, `rescue`, or `maintenance` mode, **refuses confirmation** and leaves the previous boot order intact.

```bash
sudo rustyuki confirm
```
