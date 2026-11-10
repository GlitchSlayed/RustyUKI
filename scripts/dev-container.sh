#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE_NAME="rustyuki-dev"

# Build container image if not already built
if ! podman image exists "${IMAGE_NAME}"; then
    echo "[*] Building development container image (${IMAGE_NAME})..."
    podman build -t "${IMAGE_NAME}" -f "${REPO_ROOT}/Containerfile.dev" "${REPO_ROOT}"
fi

# Run container with privileged capabilities, host network, and workspace mounted
if [ "$#" -eq 0 ]; then
    echo "[*] Starting interactive root shell in container..."
    exec podman run --rm -it \
        --privileged \
        --security-opt label=disable \
        -v "${REPO_ROOT}:/workspace:z" \
        -w /workspace \
        "${IMAGE_NAME}" \
        bash
else
    exec podman run --rm \
        --privileged \
        --security-opt label=disable \
        -v "${REPO_ROOT}:/workspace:z" \
        -w /workspace \
        "${IMAGE_NAME}" \
        "$@"
fi
