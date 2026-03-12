#!/bin/bash
# =============================================================================
# uki-setup.sh
# Builds a Unified Kernel Image (UKI) using a two-stage dracut + ukify
# pipeline on Fedora / Fedora-based
# systems and installs a kernel-install(8) plugin so the UKI is rebuilt
# automatically every time a kernel is installed or removed via dnf/rpm.
#
# Run once as root:
#   sudo bash uki-setup.sh
#
# After setup, future kernel updates are handled automatically.
# You can also rebuild manually at any time:
#   sudo /usr/local/sbin/uki-build.sh [kernel-version]
# =============================================================================

set -euo pipefail

# =============================================================================
# ──  USER CONFIGURATION  ─────────────────────────────────────────────────────
# =============================================================================

# Directory on the ESP where UKIs will be written.
# Most Fedora systems mount the ESP at /boot/efi.
EFI_DIR="/boot/efi/EFI/Linux"

# Kernel command-line embedded into the UKI.
# This acts as a manual fallback if auto-detection cannot find a usable value.
#
# Find your root UUID with:  lsblk -f   or   blkid
#
# Examples:
#   Plain ext4/xfs/btrfs:
#     CMDLINE="root=UUID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx rw quiet rhgb"
#
#   Btrfs with subvolume:
#     CMDLINE="root=UUID=xxxx rootflags=subvol=@ rw quiet rhgb"
#
#   LUKS full-disk encryption:
#     CMDLINE="rd.luks.uuid=xxxx rd.lvm.lv=fedora/root root=/dev/mapper/fedora-root rw quiet rhgb"
#
CMDLINE="root=UUID=REPLACE-ME rw quiet rhgb"

# Set to 1 to auto-detect cmdline automatically.
# Detection order:
#   1) /proc/cmdline (current boot)
#   2) /etc/kernel/cmdline
#   3) GRUB_CMDLINE_LINUX from /etc/default/grub or /etc/default/grub.d/*.cfg
# If all of the above fail, falls back to CMDLINE.
AUTO_DETECT_CMDLINE=1

# Optional Secure Boot settings passed to ukify via a temporary [UKI] config.
# Leave empty to build unsigned UKIs.
UKIFY_SB_KEY=""   # e.g. "/etc/pki/uki/db.key"
UKIFY_SB_CERT=""  # e.g. "/etc/pki/uki/db.crt"

# Optional initramfs validation configuration that is templated into uki-build.sh.
# Populate required/forbidden lists with one path per line (comments with '#').
INITRAMFS_REQUIRED_LIST="/etc/uki/initramfs-required.txt"
INITRAMFS_FORBIDDEN_LIST="/etc/uki/initramfs-forbidden.txt"
INITRAMFS_STATE_DIR="/var/lib/uki-build"
INITRAMFS_STRICT_DIFF=0

# =============================================================================
# ──  SCRIPT INTERNALS  ───────────────────────────────────────────────────────
# =============================================================================

SELF="$(realpath "$0")"
BUILD_SCRIPT="/usr/local/sbin/uki-build.sh"
INSTALL_PLUGIN="/usr/lib/kernel/install.d/90-uki-ukify.install"
BACKUP_ROOT="/var/backups/uki-setup"
PKG_MGR=""
PKG_INSTALL_CMD=()

# Colour helpers
_c() { printf '\e[%sm' "$1"; }
RED=$(_c 31); GRN=$(_c 32); YLW=$(_c 33); BLD=$(_c 1); RST=$(_c 0)
info()  { echo "${GRN}${BLD}[uki]${RST}  $*"; }
warn()  { echo "${YLW}${BLD}[warn]${RST} $*" >&2; }
die()   { echo "${RED}${BLD}[err]${RST}  $*" >&2; exit 1; }
hr()    { echo "──────────────────────────────────────────────────────────────"; }
require_cmd() { command -v "$1" &>/dev/null || die "Required command missing: $1"; }

sanitize_cmdline() {
    sed -E 's/(^| )BOOT_IMAGE=[^ ]*//g; s/(^| )initrd=[^ ]*//g; s/(^| )rd\.driver\.blacklist=[^ ]*//g; s/  +/ /g; s/^ //; s/ $//'
}

read_grub_cmdline() {
    local file line value

    for file in /etc/default/grub /etc/default/grub.d/*.cfg; do
        [[ -f "$file" ]] || continue
        while IFS= read -r line; do
            [[ "$line" =~ ^[[:space:]]*# ]] && continue
            if [[ "$line" =~ GRUB_CMDLINE_LINUX[[:space:]]*= ]]; then
                value=$(printf '%s\n' "$line" | sed -n 's/^[[:space:]]*GRUB_CMDLINE_LINUX[[:space:]]*=[[:space:]]*"\\(.*\\)"[[:space:]]*$/\\1/p')
                [[ -z "$value" ]] && value=$(printf '%s\n' "$line" | sed -n "s/^[[:space:]]*GRUB_CMDLINE_LINUX[[:space:]]*=[[:space:]]*'\\(.*\\)'[[:space:]]*$/\\1/p")
                [[ -n "$value" ]] && { echo "$value"; return 0; }
            fi
        done < "$file"
    done

    return 1
}

get_effective_cmdline() {
    local proc_cmdline kernel_cmdline grub_cmdline

    if [[ "$AUTO_DETECT_CMDLINE" -eq 1 ]]; then
        if [[ -r /proc/cmdline ]]; then
            proc_cmdline=$(sanitize_cmdline < /proc/cmdline | xargs || true)
            if [[ -n "$proc_cmdline" && "$proc_cmdline" =~ (root=|rd.luks.uuid=|rootfstype=) ]]; then
                info "Using cmdline from /proc/cmdline"
                echo "$proc_cmdline"
                return 0
            fi
        fi

        if [[ -s /etc/kernel/cmdline ]]; then
            kernel_cmdline=$(sanitize_cmdline < /etc/kernel/cmdline | xargs || true)
            if [[ -n "$kernel_cmdline" && "$kernel_cmdline" =~ (root=|rd.luks.uuid=|rootfstype=) ]]; then
                info "Using cmdline from /etc/kernel/cmdline"
                echo "$kernel_cmdline"
                return 0
            fi
        fi

        grub_cmdline=$(read_grub_cmdline || true)
        if [[ -n "$grub_cmdline" ]]; then
            grub_cmdline=$(printf '%s\n' "$grub_cmdline" | sanitize_cmdline | xargs || true)
            if [[ -n "$grub_cmdline" && "$grub_cmdline" =~ (root=|rd.luks.uuid=|rootfstype=) ]]; then
                info "Using cmdline from GRUB configuration"
                echo "$grub_cmdline"
                return 0
            fi
        fi

        warn "Auto-detect enabled, but no bootable cmdline was detected. Falling back to configured CMDLINE."
    fi

    echo "$CMDLINE"
}

ESP_MOUNT_CANDIDATES=(
    /boot/efi
    /efi
    /boot
    /boot/EFI
    /esp
)

ESP_GUID="c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
BIOS_BOOT_GUID="21686148-6449-6e6f-744e-656564454649"
ESP_MIN_AVAIL_BYTES=$((150 * 1024 * 1024))

find_esp_device() {
    # Prefer lsblk PARTTYPE, but fall back to blkid TYPE/PART_ENTRY_TYPE
    local dev

    dev="$(lsblk -pnro PATH,PARTTYPE,FSTYPE 2>/dev/null \
        | awk -v esp_guid="$ESP_GUID" '$2==esp_guid && tolower($3) ~ /fat|vfat/ {print $1; exit}')"
    if [[ -n "$dev" ]]; then
        echo "$dev"
        return 0
    fi

    while read -r dev; do
        [[ -n "$dev" ]] || continue
        local part_entry_type fstype
        part_entry_type="$(blkid -s PART_ENTRY_TYPE -o value "$dev" 2>/dev/null | tr 'A-Z' 'a-z')"
        fstype="$(blkid -s TYPE -o value "$dev" 2>/dev/null | tr 'A-Z' 'a-z')"
        [[ "$part_entry_type" == "$ESP_GUID" ]] || continue
        [[ "$fstype" =~ ^(vfat|fat|fat32|msdos)$ ]] || continue
        echo "$dev"
        return 0
    done < <(lsblk -pnro PATH,TYPE 2>/dev/null | awk '$2=="part" {print $1}')

    return 1
}

find_bios_boot_device() {
    local dev

    dev="$(lsblk -pnro PATH,PARTTYPE,TYPE 2>/dev/null \
        | awk -v bios_guid="$BIOS_BOOT_GUID" '$3=="part" && tolower($2)==bios_guid {print $1; exit}')"
    if [[ -n "$dev" ]]; then
        echo "$dev"
        return 0
    fi

    while read -r dev; do
        [[ -n "$dev" ]] || continue
        if [[ "$(blkid -s PART_ENTRY_TYPE -o value "$dev" 2>/dev/null | tr 'A-Z' 'a-z')" == "$BIOS_BOOT_GUID" ]]; then
            echo "$dev"
            return 0
        fi
    done < <(lsblk -pnro PATH,TYPE 2>/dev/null | awk '$2=="part" {print $1}')

    return 1
}

is_valid_esp_partition() {
    local dev="$1"
    [[ -b "$dev" ]] || return 1

    local parttype fstype
    parttype="$(lsblk -pnro PARTTYPE "$dev" 2>/dev/null | head -1 | tr 'A-Z' 'a-z')"
    fstype="$(lsblk -pnro FSTYPE "$dev" 2>/dev/null | head -1 | tr 'A-Z' 'a-z')"

    if [[ "$parttype" != "$ESP_GUID" ]]; then
        parttype="$(blkid -s PART_ENTRY_TYPE -o value "$dev" 2>/dev/null | tr 'A-Z' 'a-z')"
    fi
    if [[ -z "$fstype" ]]; then
        fstype="$(blkid -s TYPE -o value "$dev" 2>/dev/null | tr 'A-Z' 'a-z')"
    fi

    [[ "$parttype" == "$ESP_GUID" ]] || return 1
    [[ "$fstype" =~ ^(vfat|fat|fat32|msdos)$ ]] || return 1
}

validate_esp_free_space() {
    local esp_mount="$1"
    local avail_bytes
    avail_bytes="$(df --output=avail -B1 "$esp_mount" 2>/dev/null | awk 'NR==2 {print $1}')"
    [[ "$avail_bytes" =~ ^[0-9]+$ ]] || die "ESP free-space check failed for ${esp_mount}. Checked via 'df --output=avail -B1'. Fix: verify ${esp_mount} is mounted and readable, then re-run."

    if (( avail_bytes < ESP_MIN_AVAIL_BYTES )); then
        die "ESP free-space check failed for ${esp_mount}. Available: ${avail_bytes} bytes; required: at least ${ESP_MIN_AVAIL_BYTES} bytes (~150MB). Fix: free space on the ESP or enlarge it, then re-run."
    fi
}

find_mounted_esp_target() {
    local candidate target fstype

    for candidate in "${ESP_MOUNT_CANDIDATES[@]}"; do
        target="$(findmnt -n -o TARGET "$candidate" 2>/dev/null || true)"
        [[ -n "$target" ]] || continue

        fstype="$(findmnt -n -o FSTYPE --target "$target" 2>/dev/null || true)"
        [[ "$fstype" =~ ^(vfat|fat|msdos)$ ]] || continue
        [[ -d "$target/EFI" ]] || continue

        echo "$target"
        return 0
    done

    while read -r target; do
        [[ -n "$target" ]] && { echo "$target"; return 0; }
    done < <(findmnt -rn -t vfat,fat -o TARGET 2>/dev/null | awk '$1 ~ /^\// && system("test -d " $1 "/EFI") == 0 {print $1}')

    return 1
}

ensure_esp_mounted() {
    local esp_mount="" esp_dev="" candidate

    esp_mount="$(find_mounted_esp_target || true)"
    if [[ -n "$esp_mount" ]]; then
        validate_esp_free_space "$esp_mount"
        info "ESP mounted at ${esp_mount}."
        return 0
    fi

    warn "ESP not currently mounted. Attempting automatic mount..."

    # First try fstab-based mount by mount point.
    for candidate in "${ESP_MOUNT_CANDIDATES[@]}"; do
        mkdir -p "$candidate"
        if mount "$candidate" &>/dev/null && findmnt "$candidate" &>/dev/null; then
            esp_mount="$(find_mounted_esp_target || true)"
            if [[ -n "$esp_mount" ]]; then
                info "Mounted ESP at ${esp_mount} using fstab entry."
                return 0
            fi
        fi
    done

    # Fallback: detect the ESP partition and mount directly.
    esp_dev="$(find_esp_device || true)"
    if [[ -n "$esp_dev" ]]; then
        if ! is_valid_esp_partition "$esp_dev"; then
            die "ESP detection found ${esp_dev}, but partition validation failed. Checked GUID (${ESP_GUID}) and FAT filesystem via lsblk/blkid. Fix: format the EFI System Partition as FAT32/vfat and ensure its GPT type is set to ESP."
        fi
        for candidate in "${ESP_MOUNT_CANDIDATES[@]}"; do
            mkdir -p "$candidate"
            if mount -t vfat "$esp_dev" "$candidate" &>/dev/null && findmnt "$candidate" &>/dev/null; then
                esp_mount="$(find_mounted_esp_target || true)"
                if [[ -n "$esp_mount" ]]; then
                    validate_esp_free_space "$esp_mount"
                    info "Mounted ESP device ${esp_dev} at ${esp_mount}."
                    return 0
                fi
            fi
        done
    fi

    return 1
}

backup_path() {
    local src="$1"
    [[ -e "$src" || -L "$src" ]] || return 0
    local ts backup_dir backup
    ts="$(date +%Y%m%d-%H%M%S)"
    backup_dir="${BACKUP_ROOT}/${ts}"
    backup="${backup_dir}${src}"
    mkdir -p "$(dirname "$backup")"
    cp -a "$src" "$backup"
    info "Backed up ${src} -> ${backup}"
}

detect_package_manager() {
    if command -v dnf &>/dev/null; then
        PKG_MGR="dnf"
        PKG_INSTALL_CMD=(dnf install -y)
    elif command -v apt-get &>/dev/null; then
        PKG_MGR="apt"
        PKG_INSTALL_CMD=(apt-get install -y)
    elif command -v zypper &>/dev/null; then
        PKG_MGR="zypper"
        PKG_INSTALL_CMD=(zypper --non-interactive install)
    elif command -v pacman &>/dev/null; then
        PKG_MGR="pacman"
        PKG_INSTALL_CMD=(pacman --noconfirm -S)
    else
        PKG_MGR=""
    fi
}

ensure_packages() {
    local missing=()
    local p
    for p in "$@"; do
        case "$PKG_MGR" in
            dnf|zypper) rpm -q "$p" &>/dev/null || missing+=("$p") ;;
            apt) dpkg -s "$p" &>/dev/null || missing+=("$p") ;;
            pacman) pacman -Q "$p" &>/dev/null || missing+=("$p") ;;
            *) missing+=("$p") ;;
        esac
    done

    if [[ ${#missing[@]} -eq 0 ]]; then
        info "All required packages already installed."
        return 0
    fi

    [[ -n "$PKG_MGR" ]] || die "No supported package manager found. Install dependencies manually: ${missing[*]}"
    info "Installing missing packages via ${PKG_MGR}: ${missing[*]}"
    "${PKG_INSTALL_CMD[@]}" "${missing[@]}" || die "Dependency installation failed via ${PKG_MGR}."
}

# =============================================================================
# PHASE 1 — Preflight checks
# =============================================================================

phase_preflight() {
    hr
    info "Phase 1: Preflight checks"

    [[ $EUID -eq 0 ]] || die "Must be run as root (sudo bash $SELF)"

    detect_package_manager
    require_cmd findmnt
    require_cmd lsblk
    require_cmd blkid
    require_cmd df
    require_cmd sed
    require_cmd awk
    require_cmd xargs

    # Verify we are on a Fedora-family system
    if [[ -f /etc/os-release ]]; then
        source /etc/os-release
        info "Detected OS: ${PRETTY_NAME:-unknown}"
    else
        warn "/etc/os-release not found — proceeding anyway."
    fi

    if [[ ! -d /sys/firmware/efi ]]; then
        local bios_boot_dev=""
        bios_boot_dev="$(find_bios_boot_device || true)"
        [[ -n "$bios_boot_dev" ]] && warn "Detected BIOS Boot partition (${bios_boot_dev}) via lsblk/blkid (GUID ${BIOS_BOOT_GUID})."
        if command -v efibootmgr &>/dev/null; then
            warn "Diagnostic: efibootmgr output (non-fatal diagnostic only):"
            efibootmgr 2>&1 | sed 's/^/[diag] /' >&2 || true
        else
            warn "Diagnostic: efibootmgr not installed yet; cannot gather firmware boot-entry diagnostics."
        fi
        warn "Diagnostic: mounted FAT targets with findmnt: $(findmnt -rn -t vfat,fat -o TARGET 2>/dev/null | tr '\n' ' ' || true)"
        die "UEFI gate failed: /sys/firmware/efi is missing. UKI setup requires booting this machine in UEFI firmware mode with a valid EFI System Partition (ESP). Fix: switch firmware/bootloader to UEFI mode, create/mark an ESP (GPT type ${ESP_GUID}, FAT32/vfat), mount it (e.g. /boot/efi), then re-run."
    fi

    info "UEFI environment confirmed via /sys/firmware/efi."

    local esp_dev=""
    esp_dev="$(find_esp_device || true)"
    [[ -n "$esp_dev" ]] || die "ESP detection failed before mount attempts. Checked GPT type ${ESP_GUID} via lsblk and blkid PART_ENTRY_TYPE fallback; no FAT32/vfat ESP partition found. Fix: create an EFI System Partition and format it as FAT32/vfat, then re-run."
    is_valid_esp_partition "$esp_dev" || die "ESP validation failed for ${esp_dev}. Checked GPT type (${ESP_GUID}) and filesystem (FAT32/vfat) via lsblk/blkid. Fix: correct partition type/filesystem, then re-run."

    ensure_esp_mounted || die "ESP is not mounted and automatic mount failed. Checked: ${ESP_MOUNT_CANDIDATES[*]}. Mount it manually, then re-run."

    local esp_mount=""
    esp_mount="$(find_mounted_esp_target || true)"
    [[ -n "$esp_mount" ]] || die "ESP mount verification failed after mount attempts. Checked candidates: ${ESP_MOUNT_CANDIDATES[*]}. Fix: mount your ESP manually (typically /boot/efi) and re-run."
    validate_esp_free_space "$esp_mount"
}

# =============================================================================
# PHASE 2 — Install dependencies
# =============================================================================

phase_deps() {
    hr
    info "Phase 2: Installing dependencies"

    local pkgs=(dracut efibootmgr binutils)

    case "$PKG_MGR" in
        dnf) pkgs+=(systemd-ukify) ;;
        apt) pkgs+=(systemd-ukify) ;;
        zypper) pkgs+=(systemd-ukify) ;;
        pacman) pkgs+=(systemd) ;;
        *) warn "Unknown package manager. Will verify commands without package installs." ;;
    esac

    ensure_packages "${pkgs[@]}"

    require_cmd dracut
    require_cmd ukify
    require_cmd lsinitrd
    require_cmd efibootmgr
}

# =============================================================================
# PHASE 3 — Write /usr/local/sbin/uki-build.sh
# =============================================================================

phase_write_build_script() {
    hr
    info "Phase 3: Writing build script → ${BUILD_SCRIPT}"

    mkdir -p "$(dirname "$BUILD_SCRIPT")"
    backup_path "$BUILD_SCRIPT"

    cat > "$BUILD_SCRIPT" <<'BUILDBODY'
#!/bin/bash
# uki-build.sh — Build (or rebuild) a UKI for the given kernel version.
# Called by the kernel-install plugin on dnf kernel install/remove,
# or manually: sudo uki-build.sh [kernel-version]

set -euo pipefail

# ── Config (mirrors values from uki-setup.sh — edit here after initial setup) ─
EFI_DIR="__EFI_DIR__"
CMDLINE="__CMDLINE__"
AUTO_DETECT_CMDLINE=__AUTO_DETECT_CMDLINE__
UKIFY_SB_KEY="__UKIFY_SB_KEY__"
UKIFY_SB_CERT="__UKIFY_SB_CERT__"
INITRAMFS_REQUIRED_LIST="__INITRAMFS_REQUIRED_LIST__"
INITRAMFS_FORBIDDEN_LIST="__INITRAMFS_FORBIDDEN_LIST__"
INITRAMFS_STATE_DIR="__INITRAMFS_STATE_DIR__"
INITRAMFS_STRICT_DIFF=__INITRAMFS_STRICT_DIFF__
# ─────────────────────────────────────────────────────────────────────────────

RED='\e[31;1m'; GRN='\e[32;1m'; YLW='\e[33;1m'; RST='\e[0m'
info()  { echo -e "${GRN}[uki-build]${RST} $*"; }
warn()  { echo -e "${YLW}[uki-build]${RST} $*" >&2; }
die()   { echo -e "${RED}[uki-build]${RST} $*" >&2; exit 1; }
require_cmd() { command -v "$1" &>/dev/null || die "Required command missing: $1"; }

ESP_MOUNT_CANDIDATES=(
    /boot/efi
    /efi
    /boot
    /boot/EFI
    /esp
)

ESP_GUID="c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
ESP_MIN_AVAIL_BYTES=$((150 * 1024 * 1024))

find_esp_device() {
    local dev

    dev=$(lsblk -pnro PATH,PARTTYPE,FSTYPE 2>/dev/null \
        | awk -v esp_guid="$ESP_GUID" '$2==esp_guid && tolower($3) ~ /fat|vfat/ {print $1; exit}')
    if [[ -n "$dev" ]]; then
        echo "$dev"
        return 0
    fi

    while read -r dev; do
        [[ -n "$dev" ]] || continue
        local part_entry_type fstype
        part_entry_type=$(blkid -s PART_ENTRY_TYPE -o value "$dev" 2>/dev/null | tr 'A-Z' 'a-z')
        fstype=$(blkid -s TYPE -o value "$dev" 2>/dev/null | tr 'A-Z' 'a-z')
        [[ "$part_entry_type" == "$ESP_GUID" ]] || continue
        [[ "$fstype" =~ ^(vfat|fat|fat32|msdos)$ ]] || continue
        echo "$dev"
        return 0
    done < <(lsblk -pnro PATH,TYPE 2>/dev/null | awk '$2=="part" {print $1}')

    return 1
}

is_valid_esp_partition() {
    local dev="$1"
    [[ -b "$dev" ]] || return 1

    local parttype fstype
    parttype=$(lsblk -pnro PARTTYPE "$dev" 2>/dev/null | head -1 | tr 'A-Z' 'a-z')
    fstype=$(lsblk -pnro FSTYPE "$dev" 2>/dev/null | head -1 | tr 'A-Z' 'a-z')

    if [[ "$parttype" != "$ESP_GUID" ]]; then
        parttype=$(blkid -s PART_ENTRY_TYPE -o value "$dev" 2>/dev/null | tr 'A-Z' 'a-z')
    fi
    if [[ -z "$fstype" ]]; then
        fstype=$(blkid -s TYPE -o value "$dev" 2>/dev/null | tr 'A-Z' 'a-z')
    fi

    [[ "$parttype" == "$ESP_GUID" ]] || return 1
    [[ "$fstype" =~ ^(vfat|fat|fat32|msdos)$ ]] || return 1
}

validate_esp_free_space() {
    local esp_mount="$1"
    local avail_bytes
    avail_bytes=$(df --output=avail -B1 "$esp_mount" 2>/dev/null | awk 'NR==2 {print $1}')
    [[ "$avail_bytes" =~ ^[0-9]+$ ]] || die "ESP free-space check failed for ${esp_mount}. Checked via 'df --output=avail -B1'. Fix: verify ${esp_mount} is mounted and readable, then re-run."

    if (( avail_bytes < ESP_MIN_AVAIL_BYTES )); then
        die "ESP free-space check failed for ${esp_mount}. Available: ${avail_bytes} bytes; required: at least ${ESP_MIN_AVAIL_BYTES} bytes (~150MB). Fix: free space on the ESP or enlarge it, then re-run."
    fi
}

find_mounted_esp_target() {
    local candidate target fstype

    for candidate in "${ESP_MOUNT_CANDIDATES[@]}"; do
        target=$(findmnt -n -o TARGET "$candidate" 2>/dev/null || true)
        [[ -n "$target" ]] || continue

        fstype=$(findmnt -n -o FSTYPE --target "$target" 2>/dev/null || true)
        [[ "$fstype" =~ ^(vfat|fat|msdos)$ ]] || continue
        [[ -d "$target/EFI" ]] || continue

        echo "$target"
        return 0
    done

    while read -r target; do
        [[ -n "$target" ]] && { echo "$target"; return 0; }
    done < <(findmnt -rn -t vfat,fat -o TARGET 2>/dev/null | awk '$1 ~ /^\// && system("test -d " $1 "/EFI") == 0 {print $1}')

    return 1
}

ensure_esp_mounted() {
    local esp_mount="" esp_dev="" candidate

    esp_mount=$(find_mounted_esp_target || true)
    if [[ -n "$esp_mount" ]]; then
        validate_esp_free_space "$esp_mount"
        info "ESP mounted at ${esp_mount}"
        return 0
    fi

    warn "ESP not mounted. Attempting automatic mount..."
    for candidate in "${ESP_MOUNT_CANDIDATES[@]}"; do
        mkdir -p "$candidate"
        if mount "$candidate" &>/dev/null && findmnt "$candidate" &>/dev/null; then
            esp_mount=$(find_mounted_esp_target || true)
            if [[ -n "$esp_mount" ]]; then
                info "Mounted ESP at ${esp_mount} using fstab entry"
                return 0
            fi
        fi
    done

    esp_dev=$(find_esp_device || true)
    if [[ -n "$esp_dev" ]]; then
        is_valid_esp_partition "$esp_dev" || die "ESP detection found ${esp_dev}, but partition validation failed. Checked GUID (${ESP_GUID}) and FAT filesystem via lsblk/blkid. Fix: format the EFI System Partition as FAT32/vfat and ensure its GPT type is set to ESP."
        for candidate in "${ESP_MOUNT_CANDIDATES[@]}"; do
            mkdir -p "$candidate"
            if mount -t vfat "$esp_dev" "$candidate" &>/dev/null && findmnt "$candidate" &>/dev/null; then
                esp_mount=$(find_mounted_esp_target || true)
                if [[ -n "$esp_mount" ]]; then
                    validate_esp_free_space "$esp_mount"
                    info "Mounted ESP device ${esp_dev} at ${esp_mount}"
                    return 0
                fi
            fi
        done
    fi

    return 1
}

sanitize_cmdline() {
    sed -E 's/(^| )BOOT_IMAGE=[^ ]*//g; s/(^| )initrd=[^ ]*//g; s/(^| )rd\.driver\.blacklist=[^ ]*//g; s/  +/ /g; s/^ //; s/ $//'
}

read_grub_cmdline() {
    local file line value

    for file in /etc/default/grub /etc/default/grub.d/*.cfg; do
        [[ -f "$file" ]] || continue
        while IFS= read -r line; do
            [[ "$line" =~ ^[[:space:]]*# ]] && continue
            if [[ "$line" =~ GRUB_CMDLINE_LINUX[[:space:]]*= ]]; then
                value=$(printf '%s\n' "$line" | sed -n 's/^[[:space:]]*GRUB_CMDLINE_LINUX[[:space:]]*=[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p')
                [[ -z "$value" ]] && value=$(printf '%s\n' "$line" | sed -n "s/^[[:space:]]*GRUB_CMDLINE_LINUX[[:space:]]*=[[:space:]]*'\(.*\)'[[:space:]]*$/\1/p")
                [[ -n "$value" ]] && { echo "$value"; return 0; }
            fi
        done < "$file"
    done

    return 1
}

get_effective_cmdline() {
    local proc_cmdline kernel_cmdline grub_cmdline

    if [[ "$AUTO_DETECT_CMDLINE" -eq 1 ]]; then
        if [[ -r /proc/cmdline ]]; then
            proc_cmdline=$(sanitize_cmdline < /proc/cmdline | xargs || true)
            if [[ -n "$proc_cmdline" && "$proc_cmdline" =~ (root=|rd.luks.uuid=|rootfstype=) ]]; then
                info "Using cmdline from /proc/cmdline: ${proc_cmdline}"
                echo "$proc_cmdline"
                return 0
            fi
        fi

        if [[ -s /etc/kernel/cmdline ]]; then
            kernel_cmdline=$(sanitize_cmdline < /etc/kernel/cmdline | xargs || true)
            if [[ -n "$kernel_cmdline" && "$kernel_cmdline" =~ (root=|rd.luks.uuid=|rootfstype=) ]]; then
                info "Using cmdline from /etc/kernel/cmdline: ${kernel_cmdline}"
                echo "$kernel_cmdline"
                return 0
            fi
        fi

        grub_cmdline=$(read_grub_cmdline || true)
        if [[ -n "$grub_cmdline" ]]; then
            grub_cmdline=$(printf '%s\n' "$grub_cmdline" | sanitize_cmdline | xargs || true)
            if [[ -n "$grub_cmdline" && "$grub_cmdline" =~ (root=|rd.luks.uuid=|rootfstype=) ]]; then
                info "Using cmdline from GRUB configuration: ${grub_cmdline}"
                echo "$grub_cmdline"
                return 0
            fi
        fi

        warn "Auto-detect enabled, but no bootable cmdline was detected. Falling back to configured CMDLINE: ${CMDLINE}"
    fi

    info "Using configured cmdline: ${CMDLINE}"
    echo "$CMDLINE"
}

check_paths_against_list() {
    local list_file="$1" all_paths_file="$2" mode="$3"
    [[ -f "$list_file" ]] || return 0

    local entry normalized hit=0
    while IFS= read -r entry || [[ -n "$entry" ]]; do
        entry="${entry%%#*}"
        entry="${entry## }"
        entry="${entry%% }"
        [[ -n "$entry" ]] || continue
        normalized="${entry#/}"
        if grep -Fxq "$normalized" "$all_paths_file"; then
            if [[ "$mode" == "forbidden" ]]; then
                die "Initramfs validation failed: forbidden path present: ${entry}"
            fi
        else
            if [[ "$mode" == "required" ]]; then
                die "Initramfs validation failed: required path missing: ${entry}"
            fi
        fi
        hit=1
    done < "$list_file"

    if [[ "$hit" -eq 1 ]]; then
        info "Validated ${mode} path list: ${list_file}"
    fi
}

validate_initramfs_artifact() {
    local unpack_dir list_file current_manifest previous_manifest
    unpack_dir=$(mktemp -d)
    list_file=$(mktemp)

    cleanup_items+=("$unpack_dir" "$list_file")

    info "Validating initramfs artifact before UKI assembly"
    lsinitrd "$INITRD_OUT" >/dev/null
    lsinitrd -f /init "$INITRD_OUT" >/dev/null || die "Initramfs validation failed: /init missing"

    (
        cd "$unpack_dir"
        lsinitrd --unpack "$INITRD_OUT" >/dev/null
    )

    (
        cd "$unpack_dir"
        find . -mindepth 1 -printf '%P\n' | sort -u
    ) > "$list_file"

    check_paths_against_list "$INITRAMFS_REQUIRED_LIST" "$list_file" "required"
    check_paths_against_list "$INITRAMFS_FORBIDDEN_LIST" "$list_file" "forbidden"

    mkdir -p "$INITRAMFS_STATE_DIR"
    current_manifest="$INITRAMFS_STATE_DIR/initramfs-${KERNEL_VER}.manifest"
    previous_manifest="$INITRAMFS_STATE_DIR/initramfs-${KERNEL_VER}.manifest.prev"

    if [[ -f "$current_manifest" ]]; then
        cp -f "$current_manifest" "$previous_manifest"
    fi
    cp -f "$list_file" "$current_manifest"

    if [[ -f "$previous_manifest" ]] && ! diff -u "$previous_manifest" "$current_manifest" >/dev/null; then
        if [[ "$INITRAMFS_STRICT_DIFF" -eq 1 ]]; then
            diff -u "$previous_manifest" "$current_manifest" || true
            die "Initramfs regression detected for ${KERNEL_VER}."
        fi
        warn "Initramfs contents changed for ${KERNEL_VER}; review diff if unexpected."
    fi
}

KERNEL_VER="${1:-$(uname -r)}"
KERNEL_IMG="/lib/modules/${KERNEL_VER}/vmlinuz"
INITRD_OUT="/tmp/initramfs-${KERNEL_VER}.img"
UKI_OUT="${EFI_DIR}/linux-${KERNEL_VER}.efi"

[[ $EUID -eq 0 ]] || die "Must run as root."
require_cmd dracut
require_cmd ukify
require_cmd lsinitrd
require_cmd findmnt
require_cmd lsblk
require_cmd blkid
require_cmd df
require_cmd efibootmgr
[[ -f "$KERNEL_IMG" ]] || die "Kernel image not found: ${KERNEL_IMG}"
mkdir -p "$EFI_DIR"
ensure_esp_mounted || die "ESP is not mounted and automatic mount failed. Checked: ${ESP_MOUNT_CANDIDATES[*]}"
ESP_MOUNT_CHECK=$(find_mounted_esp_target || true)
[[ -n "$ESP_MOUNT_CHECK" ]] || die "ESP mount verification failed after mount attempts. Checked candidates: ${ESP_MOUNT_CANDIDATES[*]}. Fix: mount your ESP manually (typically /boot/efi) and re-run."
validate_esp_free_space "$ESP_MOUNT_CHECK"

EFFECTIVE_CMDLINE=$(get_effective_cmdline)

cleanup_items=("$INITRD_OUT")

info "Stage 1/2: Building standalone initramfs via dracut: ${INITRD_OUT}"
dracut --force --kver "$KERNEL_VER" "$INITRD_OUT"

validate_initramfs_artifact

UKIFY_ARGS=(
    build
    --linux "$KERNEL_IMG"
    --initrd "$INITRD_OUT"
    --cmdline "$EFFECTIVE_CMDLINE"
    --os-release /etc/os-release
    --uname "$KERNEL_VER"
    --output "$UKI_OUT"
)

UKIFY_CONF=""
if [[ -n "$UKIFY_SB_KEY" || -n "$UKIFY_SB_CERT" ]]; then
    [[ -n "$UKIFY_SB_KEY" && -n "$UKIFY_SB_CERT" ]] || die "Set both UKIFY_SB_KEY and UKIFY_SB_CERT for Secure Boot signing."
    [[ -r "$UKIFY_SB_KEY" ]] || die "Cannot read UKIFY_SB_KEY: ${UKIFY_SB_KEY}"
    [[ -r "$UKIFY_SB_CERT" ]] || die "Cannot read UKIFY_SB_CERT: ${UKIFY_SB_CERT}"
    UKIFY_CONF=$(mktemp)
    cat > "$UKIFY_CONF" <<EOF
[UKI]
SecureBootPrivateKey=${UKIFY_SB_KEY}
SecureBootCertificate=${UKIFY_SB_CERT}
EOF
    UKIFY_ARGS+=(--config "$UKIFY_CONF")
fi

cleanup() {
    local item
    for item in "${cleanup_items[@]}"; do
        if [[ -d "$item" ]]; then
            rm -rf "$item"
        else
            rm -f "$item"
        fi
    done
    [[ -n "$UKIFY_CONF" ]] && rm -f "$UKIFY_CONF"
}
trap cleanup EXIT

info "Stage 2/2: Assembling UKI via ukify: ${UKI_OUT}"
ukify "${UKIFY_ARGS[@]}"

info "UKI built successfully: ${UKI_OUT} ($(du -sh "$UKI_OUT" | cut -f1))"

# Register / refresh UEFI boot entry
LABEL="Linux UKI ${KERNEL_VER}"

# Determine ESP mount point, disk, and partition number
ESP_MOUNT=$(find_mounted_esp_target)     || { warn "Cannot detect ESP mount — skipping efibootmgr."; exit 0; }
ESP_DEV=$(findmnt -n -o SOURCE "$ESP_MOUNT")     || { warn "Cannot detect ESP device — skipping efibootmgr."; exit 0; }
ESP_DEV_NAME="${ESP_DEV##*/}"   # e.g. sda1 or nvme0n1p1
ESP_DISK_NAME=$(lsblk -no PKNAME "$ESP_DEV" 2>/dev/null | head -1)     || { warn "Cannot detect disk for ${ESP_DEV} — skipping efibootmgr."; exit 0; }
ESP_PART_NUM=$(cat "/sys/class/block/${ESP_DEV_NAME}/partition" 2>/dev/null)     || { warn "Cannot read partition number — skipping efibootmgr."; exit 0; }

# Build the loader path relative to the ESP (efibootmgr needs backslashes)
REL_PATH="${UKI_OUT#${ESP_MOUNT}}"          # strip ESP mount prefix
REL_PATH_BS="${REL_PATH//\//\\}"            # forward→backslash

# Remove any existing entry with the same label
OLD_NUM=$(efibootmgr 2>/dev/null \
    | grep -F "* ${LABEL}" \
    | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\).*/\1/p' \
    || true)
if [[ -n "$OLD_NUM" ]]; then
    info "Removing stale EFI entry Boot${OLD_NUM}…"
    efibootmgr --quiet --bootnum "$OLD_NUM" --delete-bootnum
fi

info "Registering UEFI entry: '${LABEL}'"
efibootmgr --quiet \
    --create \
    --disk    "/dev/${ESP_DISK_NAME}" \
    --part    "$ESP_PART_NUM" \
    --label   "$LABEL" \
    --loader  "$REL_PATH_BS"

info "Done. Run 'efibootmgr -v' to verify."
BUILDBODY

    # Substitute configuration placeholders
    sed -i \
        -e "s|__EFI_DIR__|${EFI_DIR}|g" \
        -e "s|__CMDLINE__|${CMDLINE}|g" \
        -e "s|__AUTO_DETECT_CMDLINE__|${AUTO_DETECT_CMDLINE}|g" \
        -e "s|__UKIFY_SB_KEY__|${UKIFY_SB_KEY}|g" \
        -e "s|__UKIFY_SB_CERT__|${UKIFY_SB_CERT}|g" \
        -e "s|__INITRAMFS_REQUIRED_LIST__|${INITRAMFS_REQUIRED_LIST}|g" \
        -e "s|__INITRAMFS_FORBIDDEN_LIST__|${INITRAMFS_FORBIDDEN_LIST}|g" \
        -e "s|__INITRAMFS_STATE_DIR__|${INITRAMFS_STATE_DIR}|g" \
        -e "s|__INITRAMFS_STRICT_DIFF__|${INITRAMFS_STRICT_DIFF}|g" \
        "$BUILD_SCRIPT"

    chmod 0755 "$BUILD_SCRIPT"
    info "Build script written."
}

# =============================================================================
# PHASE 4 — Write kernel-install(8) plugin
# =============================================================================

phase_write_plugin() {
    hr
    info "Phase 4: Writing kernel-install plugin → ${INSTALL_PLUGIN}"

    mkdir -p "$(dirname "$INSTALL_PLUGIN")"
    backup_path "$INSTALL_PLUGIN"

    cat > "$INSTALL_PLUGIN" <<PLUGINBODY
#!/bin/bash
# /usr/lib/kernel/install.d/90-uki-ukify.install
# kernel-install plugin — rebuild UKI on kernel add/remove.
# Managed by uki-setup.sh — do not edit directly.

COMMAND="\${1:?}"
KERNEL_VER="\${2:?}"
BUILD_SCRIPT="${BUILD_SCRIPT}"

log()  { logger -t uki-install "\$*"; echo "[uki-install] \$*"; }
warn() { logger -p user.warning -t uki-install "\$*"; echo "[uki-install] WARN: \$*" >&2; }

case "\$COMMAND" in
    add)
        log "Kernel add: \${KERNEL_VER} — rebuilding UKI…"
        if [[ ! -x "\$BUILD_SCRIPT" ]]; then
            warn "\${BUILD_SCRIPT} not found/executable — skipping."
            exit 0
        fi
        "\$BUILD_SCRIPT" "\$KERNEL_VER"
        ;;
    remove)
        log "Kernel remove: \${KERNEL_VER} — cleaning UKI…"
        UKI="${EFI_DIR}/linux-\${KERNEL_VER}.efi"
        if [[ -f "\$UKI" ]]; then
            rm -f "\$UKI"
            log "Removed \${UKI}"
        fi
        LABEL="Linux UKI \${KERNEL_VER}"
        BOOT_NUM=\$(efibootmgr 2>/dev/null \
            | grep -F "* \${LABEL}" \
            | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\).*/\1/p' || true)
        if [[ -n "\$BOOT_NUM" ]]; then
            efibootmgr --quiet --bootnum "\$BOOT_NUM" --delete-bootnum \
                && log "Removed EFI entry Boot\${BOOT_NUM}"
        fi
        ;;
    *)
        exit 0
        ;;
esac
exit 0
PLUGINBODY

    chmod 0755 "$INSTALL_PLUGIN"
    info "Plugin written."
}

# =============================================================================
# PHASE 5 — Disable conflicting default kernel-install plugins
#           (grub BLS entry generator etc.)
# =============================================================================

phase_disable_bls_plugins() {
    hr
    info "Phase 5: Disabling default GRUB/BLS kernel-install plugins"
    info "(They would create /boot/loader/entries/ stubs that fight with UKI booting)"

    mkdir -p /etc/kernel/install.d

    # These are the Fedora default plugins we want to silence.
    # We shadow them by creating null symlinks in /etc/kernel/install.d/
    # which takes precedence over /usr/lib/kernel/install.d/.
    local plugins=(
        20-grub.install
        50-depmod.install
        90-loaderentry.install
        92-crashkernel.install
        95-kernel-install.install   # might not exist on all versions, harmless
    )

    for p in "${plugins[@]}"; do
        local target="/etc/kernel/install.d/${p}"
        backup_path "$target"
        if [[ ! -e "$target" ]]; then
            ln -s /dev/null "$target"
            info "  Disabled: ${p}"
        else
            info "  Already overridden: ${p} — skipping."
        fi
    done
}

# =============================================================================
# PHASE 6 — Build UKI for the currently running kernel
# =============================================================================

phase_initial_build() {
    hr
    info "Phase 6: Building UKI for current kernel: $(uname -r)"

    if [[ "$AUTO_DETECT_CMDLINE" -eq 0 && "$CMDLINE" == "root=UUID=REPLACE-ME rw quiet rhgb" ]]; then
        warn "────────────────────────────────────────────────────────────"
        warn "AUTO_DETECT_CMDLINE is disabled and CMDLINE is still placeholder text."
        warn "Set CMDLINE to a real root=... value, or enable AUTO_DETECT_CMDLINE=1."
        warn "────────────────────────────────────────────────────────────"
        read -r -p "Continue with placeholder CMDLINE anyway? [y/N] " ans
        [[ "${ans,,}" == "y" ]] || { info "Aborted. Set CMDLINE and re-run."; exit 0; }
    fi

    "$BUILD_SCRIPT" "$(uname -r)"
}

# =============================================================================
# PHASE 7 — Post-install summary
# =============================================================================

phase_summary() {
    hr
    info "Setup complete!"
    echo ""
    echo "  Files installed:"
    echo "    ${BUILD_SCRIPT}   ← rebuild script (edit CMDLINE here)"
    echo "    ${INSTALL_PLUGIN} ← auto-trigger on kernel installs"
    echo ""
    echo "  UKIs are stored in: ${EFI_DIR}/"
    find "${EFI_DIR}" -maxdepth 1 -type f -name "*.efi" -exec ls -lh {} + 2>/dev/null | sed 's/^/    /' || true
    echo ""
    echo "  Current UEFI boot entries:"
    efibootmgr -v 2>/dev/null | grep -E 'BootOrder|Boot[0-9A-Fa-f]{4}' | sed 's/^/    /' || true
    echo ""
    echo "  To set the UKI as the primary boot target:"
    echo "    sudo efibootmgr --bootorder NNNN,MMMM,..."
    echo "    (replace NNNN with the BootXXXX number of your UKI entry)"
    echo ""
    echo "  To rebuild manually:"
    echo "    sudo ${BUILD_SCRIPT} \$(uname -r)"
    echo ""
    echo "  Backups created under: ${BACKUP_ROOT}/"
    echo ""
    warn "Reboot only after confirming the cmdline in ${BUILD_SCRIPT} is correct!"
    hr
}

# =============================================================================
# Main
# =============================================================================

if [[ "${UKI_SETUP_SKIP_MAIN:-0}" -ne 1 ]]; then
    phase_preflight
    phase_deps
    phase_write_build_script
    phase_write_plugin
    phase_disable_bls_plugins
    phase_initial_build
    phase_summary
fi
