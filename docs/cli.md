# RustyUKI CLI Reference

RustyUKI is a command-line utility for managing Unified Kernel Images. This reference covers all global options, subcommands, arguments, exit codes, and operational examples.

---

## Command Syntax

```bash
rustyuki [GLOBAL OPTIONS] <SUBCOMMAND> [SUBCOMMAND OPTIONS]
```

> [!NOTE]
> RustyUKI operations require root privileges because they interact with the EFI System Partition, `/proc`, `/lib/modules`, and UEFI NVRAM. Run commands with `sudo`.

---

## Global Options

The following flags apply to all subcommands and can be placed before or after the subcommand:

| Option | Type | Default | Description |
|---|---|---|---|
| `--config <PATH>` | File path | `/etc/uki/uki.conf` | Path to the TOML configuration file. If the file does not exist, internal defaults are used. |
| `--dry-run` | Flag | `false` | Log all external commands (`dracut`, `ukify`, `efibootmgr`, `bootctl`, `systemctl`) without executing them or modifying the filesystem. |
| `-v` | Flag | `INFO` | Enable `DEBUG` level logging. |
| `-vv` | Flag | `INFO` | Enable `TRACE` level logging for granular troubleshooting. |
| `-h`, `--help` | Flag | — | Print command-line help information. |
| `-V`, `--version` | Flag | — | Print version information. |

---

## Subcommands

### 1. `status`

Inspects and displays the resolved runtime configuration and hardware boot environment.

```bash
sudo rustyuki status
```

**Output Fields:**
- `kernel`: Running kernel version reported by `uname -r`.
- `esp_path`: Active EFI System Partition mountpoint.
- `output_dir`: Directory where UKI binaries are placed.
- `cmdline_file`: Path to the active kernel command line file.
- **GPT autodiscovery advisory**: Advises if your root partition carries a Discoverable Partitions Specification (DPS) GPT type GUID and whether `root=` is redundant or missing in your cmdline.
- `os_release`: Path to the `os-release` file used for UKI metadata.

**Example Output:**
```text
kernel: 6.12.8-200.fc41.x86_64
esp_path: /boot/efi
output_dir: /boot/efi/EFI/Linux
cmdline_file: /etc/kernel/cmdline
[INFO] Root partition carries a discoverable GPT GUID.
       'root=' in your cmdline may be unnecessary on this system.
       Consider removing it and testing with: rustyuki generate --dry-run
os_release: /etc/os-release
```

---

### 2. `generate`

Executes the two-stage build pipeline:
1. Validates ESP preflight checks (mount status, writability, free space).
2. Generates the initramfs with `dracut`.
3. Resolves, sanitizes, and verifies the kernel cmdline.
4. Assembles the UKI PE binary with `ukify` using an atomic write.
5. Registers a new EFI boot entry with `efibootmgr` without altering permanent `BootOrder`.
6. (Optional) Schedules the entry for a one-time trial via `efibootmgr --bootnext` if `--boot-once` is set.

```bash
sudo rustyuki generate [OPTIONS]
```

**Options:**

| Flag | Argument | Default | Description |
|---|---|---|---|
| `--kernel-version` | String | `uname -r` or config | Target kernel version (must have `/lib/modules/<ver>/vmlinuz`). |
| `--esp-path` | Directory path | Config `/boot/efi` | Mountpoint of the EFI System Partition. |
| `--output-dir` | Directory path | Config `/boot/efi/EFI/Linux` | Target directory for the final `.efi` binary. |
| `--cmdline-file` | File path | Config `/etc/kernel/cmdline` | File to read kernel parameters from. |
| `--splash` | File path | Config or none | Path to a BMP or PNG splash logo embedded in the UKI. |
| `--os-release` | File path | Config `/etc/os-release` | OS metadata file embedded into the `.osrel` PE section. |
| `--boot-once` | Flag | `false` | Sets the new entry as `BootNext` for one-time trial testing. |

**Examples:**
```bash
# Build for currently running kernel
sudo rustyuki generate

# Dry run: view commands without modifying ESP or NVRAM
sudo rustyuki generate --dry-run

# Build a specific installed kernel
sudo rustyuki generate --kernel-version 6.12.8-200.fc41.x86_64

# Build and schedule for a single trial boot
sudo rustyuki generate --boot-once
```

---

### 3. `install`

The primary command for activating RustyUKI. It implements a safe trial-boot installation workflow:

1. Performs ESP preflight validation checks.
2. Creates and enables the systemd unit `/etc/systemd/system/rustyuki-boot-confirm.service`.
3. Builds the UKI for the target kernel and registers it as an EFI boot entry.
4. Programs `BootNext` to trial boot this new UKI on the very next reboot.
5. Queries `rpm -q kernel` and prunes any stale `linux-*.efi` images in the output directory.
6. Runs `bootctl update` to synchronize the EFI boot environment.

```bash
sudo rustyuki install [OPTIONS]
```

**Options:** Accepts all flags available on `generate` (except `--boot-once`, which is enabled automatically by `install`).

**Example:**
```bash
sudo rustyuki install
```

---

### 4. `reconcile`

Rebuilds UKIs for all currently installed kernel packages and cleans up orphaned files.

1. Queries installed kernels via `rpm -q kernel` that have a corresponding `/lib/modules/<version>/vmlinuz`.
2. Generates a fresh UKI for each installed kernel.
3. Scans the output directory and deletes any `linux-*.efi` file that does not match an installed kernel.
4. Executes `bootctl update`.

```bash
sudo rustyuki reconcile
```

---

### 5. `confirm`

Promotes the currently booted trial UKI to the front of `BootOrder` and clears `BootNext`.

Normally, `confirm` is run automatically early during boot by `rustyuki-boot-confirm.service`. You can also execute it manually after booting into a trial UKI.

```bash
sudo rustyuki confirm
```

**Mechanism:**
1. Queries `efibootmgr --verbose` to identify the active `BootCurrent` entry ID.
2. Reconstructs `BootOrder` with `BootCurrent` at index 0.
3. Invokes `efibootmgr --bootorder ...`.
4. Invokes `efibootmgr --delete-bootnext`.

---

### 6. `install-hook`

Installs the Fedora `kernel-install` hook to make UKI maintenance fully automatic across OS updates.

```bash
sudo rustyuki install-hook [OPTIONS]
```

**Options:**

| Flag | Argument | Default | Description |
|---|---|---|---|
| `--plugin-path` | File path | `/usr/lib/kernel/install.d/90-rustyuki.install` | Path where the executable plugin script is created. |
| `--binary-path` | File path | Current executable or `/usr/local/bin/rustyuki` | Path to the RustyUKI binary invoked by the plugin. |

**Example:**
```bash
sudo rustyuki install-hook
```

---

## Exit Codes

| Exit Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | General error (missing files, command execution failure, invalid configuration, permission denied). |
| `2` | Clap argument parsing error (invalid arguments or flags). |
