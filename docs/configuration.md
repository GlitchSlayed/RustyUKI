# RustyUKI Configuration Guide

RustyUKI reads its configuration from `/etc/rustyuki/config.toml` by default (falling back to `/etc/uki/uki.conf` for backward compatibility). You can override this location using the `--config <PATH>` global CLI option.

If the configuration file does not exist, RustyUKI operates with built-in safe defaults.

---

## Configuration File Format

The configuration file is formatted in standard TOML. Below is an annotated reference configuration:

```toml
[uki]
# Output directory on the ESP where .efi UKI binaries will be written.
output_dir = "/efi/EFI/Linux"

# Mountpoint of the EFI System Partition (ESP).
esp_path = "/efi"

# Override kernel release version (empty = active kernel / passed version).
kernel_version = ""

# Automatically detect kernel cmdline from system sources (/proc, /etc/kernel/cmdline).
auto_detect_cmdline = true

# Fallback kernel command line used if auto-detection is disabled.
configured_cmdline = "root=UUID=auto rw quiet"

# Path to the persistent kernel command line file.
cmdline_file = "/etc/kernel/cmdline"

# Directory for caching validated cmdline hashes and failure logs.
cmdline_state_dir = "/var/lib/uki-build"

# Warn if cmdline has fewer tokens than this threshold after sanitization.
cmdline_min_tokens = 3

# Optional: Path to a splash/logo image (BMP) embedded in the UKI.
splash = ""

# Path to the os-release file embedded in the UKI PE .osrel section.
os_release = "/usr/lib/os-release"

# Target sysroot for offline image building (empty = live host "/").
sysroot = ""

[dracut]
# Additional command-line flags passed directly to dracut during stage 1 initramfs creation.
extra_args = []

[ukify]
# Additional command-line flags passed directly to ukify during stage 2 UKI assembly.
extra_args = []

[bootloader]
# Boot management backend: "systemd-boot" or "direct"
# - "systemd-boot": Standardized Type #2 UKI discovery under /EFI/Linux, kernel selection, and boot counting.
# - "direct": Direct UEFI execution via efibootmgr entries (no intermediate bootloader).
backend = "systemd-boot"

# Automatically arm one-shot trial boot for newly generated UKIs.
trial_boot = true

# Keep previous working UKI as .prev.efi fallback slot during updates.
keep_backup = true

# Automatically prune stale UKIs during reconciliation (provenance-checked).
prune_stale_images = true

[signing]
# Enable Secure Boot PE signing.
enabled = false

# Signing tool: "sbsign" or "pesign".
tool = "sbsign"

# Path to X.509 certificate for signing (PEM or DER).
cert_path = "/etc/pki/sb/db.crt"

# Path to private key for signing (PEM).
key_path = "/etc/pki/sb/db.key"

# Optional NSS database nickname or PKCS#11 token for pesign.
token = ""
```

---

## Configuration Reference

### `[uki]` Section

| Key | Type | Default | Description |
|---|---|---|---|
| `output_dir` | Path | `"/efi/EFI/Linux"` | Destination directory for `.efi` UKI binaries on the ESP. |
| `esp_path` | Path | `"/efi"` | Root mountpoint of the EFI System Partition. Checked for >=50MB free space before build. |
| `kernel_version` | String | `""` | Target kernel version override. When empty, resolves via CLI argument or installed kernels. |
| `auto_detect_cmdline` | Boolean | `true` | When `true`, discovers parameters from `/proc/cmdline` and `/etc/kernel/cmdline`, preserving essential driver blacklists (`rd.driver.blacklist=*`). |
| `configured_cmdline` | String | `"root=UUID=auto rw quiet"` | Fallback command line if auto-detection is disabled. |
| `cmdline_file` | Path | `"/etc/kernel/cmdline"` | Persistent command-line file. |
| `cmdline_state_dir` | Path | `"/var/lib/uki-build"` | Directory for SHA-256 cmdline hash cache and failure tracking. |
| `cmdline_min_tokens` | Integer | `3` | Emits a warning if sanitized cmdline contains fewer tokens than threshold. |
| `splash` | Path | `""` | Optional path to boot splash bitmap. |
| `os_release` | Path | `"/usr/lib/os-release"` | OS release metadata embedded into `.osrel` PE section. |
| `sysroot` | Path | `""` | Offline target root directory. Completely bypasses host NVRAM operations. |

### `[bootloader]` Section

| Key | Type | Default | Description |
|---|---|---|---|
| `backend` | String | `"systemd-boot"` | Boot backend: `"systemd-boot"` (default) for standardized Type #2 UKIs under `/EFI/Linux/` or `"direct"` for Direct UEFI firmware execution. |
| `trial_boot` | Boolean | `true` | Arms a one-time trial boot (`BootNext` or `bootctl set-oneshot`) during installation. |
| `keep_backup` | Boolean | `true` | Preserves the previous UKI as `linux-<version>.prev.efi` for instant rollback. |
| `prune_stale_images` | Boolean | `true` | Cleans up UKIs for uninstalled kernels during reconciliation, verifying provenance sidecars (`.manifest.json`). |

### `[signing]` Section

| Key | Type | Default | Description |
|---|---|---|---|
| `enabled` | Boolean | `false` | Enable Secure Boot signing for generated UKIs. |
| `tool` | String | `"sbsign"` | Signing utility: `"sbsign"` or `"pesign"`. |
| `cert_path` | Path | `"/etc/pki/sb/db.crt"` | Path to public X.509 certificate for `sbsign`. |
| `key_path` | Path | `"/etc/pki/sb/db.key"` | Path to private key for `sbsign`. |
| `token` | String | `""` | Optional NSS database certificate nickname or PKCS#11 token for `pesign`. |
