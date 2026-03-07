#!/bin/bash
# =============================================================================
# uki-setup.sh
# Builds a Unified Kernel Image (UKI) using dracut on Fedora / Fedora-based
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
# !! EDIT THIS before running setup !!
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
CMDLINE="rw quiet rhgb"

# Set to 1 to auto-detect the cmdline from the currently running system
# (reads /proc/cmdline, strips loader-specific tokens like BOOT_IMAGE=).
# Useful when you are unsure of the exact parameters needed.
AUTO_DETECT_CMDLINE=0

# Path to the systemd-boot EFI stub used by dracut --uefi.
# Fedora ships this in systemd-boot-unsigned or systemd.
# dracut will find it automatically on Fedora; set explicitly if needed.
EFI_STUB=""   # e.g. "/usr/lib/systemd/boot/efi/linuxx64.efi.stub"

# =============================================================================
# ──  SCRIPT INTERNALS  ───────────────────────────────────────────────────────
# =============================================================================

SELF="$(realpath "$0")"
BUILD_SCRIPT="/usr/local/sbin/uki-build.sh"
INSTALL_PLUGIN="/usr/lib/kernel/install.d/90-uki-dracut.install"
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

    # Check for UEFI — try several indicators since /sys/firmware/efi
    # can be absent even on real UEFI systems (efivarfs not mounted, etc.)
    local uefi_detected=0
    [[ -d /sys/firmware/efi ]]          && uefi_detected=1
    [[ -d /sys/firmware/efi/efivars ]]  && uefi_detected=1
    findmnt /boot/efi &>/dev/null       && uefi_detected=1
    findmnt /efi      &>/dev/null       && uefi_detected=1
    [[ -d /boot/efi/EFI ]]              && uefi_detected=1
    [[ -d /efi/EFI    ]]                && uefi_detected=1
    if command -v efibootmgr &>/dev/null && efibootmgr &>/dev/null 2>&1; then
        uefi_detected=1
    fi

    if [[ $uefi_detected -eq 0 ]]; then
        warn "Could not confirm UEFI environment via any detection method."
        warn "(/sys/firmware/efi absent, no ESP mounted, efibootmgr unresponsive)"
        read -r -p "Continue anyway? [y/N] " ans
        [[ "${ans,,}" == "y" ]] || die "Aborted. Verify your system is UEFI and the ESP is mounted."
    else
        info "UEFI environment confirmed."
    fi

    # Verify ESP is mounted
    if ! findmnt /boot/efi &>/dev/null && ! findmnt /efi &>/dev/null; then
        die "ESP not mounted at /boot/efi or /efi. Mount it first."
    fi
}

# =============================================================================
# PHASE 2 — Install dependencies
# =============================================================================

phase_deps() {
    hr
    info "Phase 2: Installing dependencies"

    local pkgs=(dracut efibootmgr binutils)

    case "$PKG_MGR" in
        dnf) pkgs+=(systemd-boot-unsigned) ;;
        apt) pkgs+=(systemd-boot-efi) ;;
        zypper) pkgs+=(systemd-boot) ;;
        pacman) pkgs+=(systemd) ;;
        *) warn "Unknown package manager. Will verify commands without package installs." ;;
    esac

    ensure_packages "${pkgs[@]}"

    require_cmd dracut
    require_cmd efibootmgr
    dracut --help | grep -q -- '--uefi' || die "Installed dracut does not support --uefi"

    if ! command -v sbsign &>/dev/null; then
        warn "sbsigntools not installed — UKIs will not be Secure Boot signed."
        warn "Install later using your package manager (package often named 'sbsigntools')."
    fi
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
EFI_STUB="__EFI_STUB__"
# ─────────────────────────────────────────────────────────────────────────────

RED='\e[31;1m'; GRN='\e[32;1m'; YLW='\e[33;1m'; RST='\e[0m'
info()  { echo -e "${GRN}[uki-build]${RST} $*"; }
warn()  { echo -e "${YLW}[uki-build]${RST} $*" >&2; }
die()   { echo -e "${RED}[uki-build]${RST} $*" >&2; exit 1; }
require_cmd() { command -v "$1" &>/dev/null || die "Required command missing: $1"; }

KERNEL_VER="${1:-$(uname -r)}"
KERNEL_IMG="/lib/modules/${KERNEL_VER}/vmlinuz"
UKI_OUT="${EFI_DIR}/linux-${KERNEL_VER}.efi"

[[ $EUID -eq 0 ]] || die "Must run as root."
require_cmd dracut
require_cmd findmnt
require_cmd lsblk
require_cmd efibootmgr
[[ -f "$KERNEL_IMG" ]] || die "Kernel image not found: ${KERNEL_IMG}"
mkdir -p "$EFI_DIR"

# Build effective cmdline
if [[ "$AUTO_DETECT_CMDLINE" -eq 1 ]]; then
    EFFECTIVE_CMDLINE=$(sed 's/BOOT_IMAGE=[^ ]*//g; s/initrd=[^ ]*//g; s/  */ /g' /proc/cmdline | xargs)
    info "Auto-detected cmdline: ${EFFECTIVE_CMDLINE}"
else
    EFFECTIVE_CMDLINE="$CMDLINE"
    info "Using configured cmdline: ${EFFECTIVE_CMDLINE}"
fi

# Locate EFI stub
if [[ -z "$EFI_STUB" ]]; then
    for candidate in \
        /usr/lib/systemd/boot/efi/linuxx64.efi.stub \
        /lib/systemd/boot/efi/linuxx64.efi.stub \
        /usr/lib/gummiboot/linuxx64.efi.stub; do
        if [[ -f "$candidate" ]]; then
            EFI_STUB="$candidate"
            break
        fi
    done
fi
[[ -f "${EFI_STUB:-}" ]] || die "EFI stub not found. Install your distro package for systemd-boot EFI stub."
info "EFI stub: ${EFI_STUB}"

# Build dracut arguments
DRACUT_ARGS=(
    --force
    --no-hostonly-cmdline       # we embed our own cmdline below
    --kernel-image  "$KERNEL_IMG"
    --kver          "$KERNEL_VER"
    --uefi
    --uefi-stub     "$EFI_STUB"
    --kernel-cmdline "$EFFECTIVE_CMDLINE"
)

info "Building UKI: ${UKI_OUT}"
info "Running dracut…"
dracut "${DRACUT_ARGS[@]}" "$UKI_OUT"

info "UKI built successfully: ${UKI_OUT} ($(du -sh "$UKI_OUT" | cut -f1))"

# Register / refresh UEFI boot entry
LABEL="Linux UKI ${KERNEL_VER}"

# Determine ESP mount point, disk, and partition number
ESP_MOUNT=$(findmnt -n -o TARGET /boot/efi 2>/dev/null || findmnt -n -o TARGET /efi 2>/dev/null) \
    || { warn "Cannot detect ESP mount — skipping efibootmgr."; exit 0; }
ESP_DEV=$(findmnt -n -o SOURCE "$ESP_MOUNT") \
    || { warn "Cannot detect ESP device — skipping efibootmgr."; exit 0; }
ESP_DEV_NAME="${ESP_DEV##*/}"   # e.g. sda1 or nvme0n1p1
ESP_DISK_NAME=$(lsblk -no PKNAME "$ESP_DEV" 2>/dev/null | head -1) \
    || { warn "Cannot detect disk for ${ESP_DEV} — skipping efibootmgr."; exit 0; }
ESP_PART_NUM=$(cat "/sys/class/block/${ESP_DEV_NAME}/partition" 2>/dev/null) \
    || { warn "Cannot read partition number — skipping efibootmgr."; exit 0; }

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
        -e "s|__EFI_STUB__|${EFI_STUB}|g" \
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
# /usr/lib/kernel/install.d/90-uki-dracut.install
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

    if [[ "$AUTO_DETECT_CMDLINE" -eq 0 && "$CMDLINE" == "rw quiet rhgb" ]]; then
        warn "────────────────────────────────────────────────────────────"
        warn "CMDLINE is still the placeholder default."
        warn "The UKI may not boot correctly without a proper root= parameter."
        warn "Edit CMDLINE at the top of this script, then re-run, OR"
        warn "set AUTO_DETECT_CMDLINE=1 to pull parameters from the live system."
        warn "────────────────────────────────────────────────────────────"
        read -r -p "Continue with default CMDLINE anyway? [y/N] " ans
        [[ "${ans,,}" == "y" ]] || { info "Aborted. Edit CMDLINE and re-run."; exit 0; }
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
