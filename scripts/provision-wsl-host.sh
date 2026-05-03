#!/bin/bash
#
# Build a pre-configured WSL instance for hosting Helios VMs via libvirt/KVM.
#
# Uses Docker to build a fully configured rootfs, exports it as a tar,
# and imports it as a WSL instance. Post-import setup is done entirely
# via SSH — no wsl.exe calls after the instance is started.
#
# IMPORTANT: systemd is NOT used. Services (sshd, libvirtd) are started
# via [boot] command= in wsl.conf. This prevents the binfmt_misc
# WSLInterop breakage caused by systemd-binfmt.service.
#
# The script is idempotent: re-run skips completed steps.
#
# Usage: ./provision-wsl-host.sh [--rebuild]
#   --rebuild: destroy existing helios-host instance and recreate
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

INSTANCE_NAME="helios-host"
DOCKER_IMAGE="helios-host-rootfs"
ROOTFS_TAR="$SCRIPT_DIR/helios-host-rootfs.tar"
WSL_INSTALL_PATH='C:\Users\'"$(whoami)"'\AppData\Local\WSL\helios-host'

# --- Gather host user details ---
HOST_USER="$(whoami)"
HOST_UID="$(id -u)"
HOST_PUBKEY="$(cat ~/.ssh/id_rsa.pub 2>/dev/null || cat ~/.ssh/id_ed25519.pub 2>/dev/null)" || {
    echo "ERROR: No SSH public key found in ~/.ssh/"
    exit 1
}

SSH_PORT=2222

# --- Helpers ---
instance_exists() {
    wsl.exe --list --quiet 2>/dev/null | tr -d '\0\r' | grep -q "^${INSTANCE_NAME}$"
}

echo "=== Building helios-host WSL instance ==="
echo "  User: $HOST_USER (UID $HOST_UID)"
echo "  SSH port: $SSH_PORT"
echo "  Instance: $INSTANCE_NAME"
echo ""

# =====================================================================
# STEP 1: Docker image (no wsl.exe calls)
# =====================================================================
echo "=== Step 1: Docker image ==="

if docker images "$DOCKER_IMAGE" --format '{{.ID}}' 2>/dev/null | grep -q .; then
    echo "  Image '$DOCKER_IMAGE' already exists, skipping build."
    echo "  (Use 'docker rmi $DOCKER_IMAGE' to force rebuild.)"
else
    echo "  Building Docker image..."

    DOCKER_CONTEXT=$(mktemp -d)
    trap 'rm -rf "$DOCKER_CONTEXT"' EXIT

    cat > "$DOCKER_CONTEXT/Dockerfile" <<'DOCKERFILE'
FROM ubuntu:24.04

ARG HOST_USER=swherdman
ARG HOST_UID=1000
ARG SSH_PORT=2222

ENV DEBIAN_FRONTEND=noninteractive

# Install all required packages in one layer
RUN apt-get update -q && \
    apt-get install -y -q --no-install-recommends \
        qemu-kvm \
        libvirt-daemon-system \
        libvirt-clients \
        openssh-server \
        virtinst \
        qemu-utils \
        cpio \
        kmod \
        sudo \
        iproute2 \
        curl \
        ca-certificates \
        dnsmasq-base \
        iptables \
        dbus \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# Create user account matching host
# Ubuntu 24.04 docker image has a 'ubuntu' user at UID 1000 — remove it first
RUN userdel -r ubuntu 2>/dev/null || true && \
    groupadd -f libvirt && \
    useradd -m -u ${HOST_UID} -s /bin/bash -G sudo,libvirt ${HOST_USER} && \
    echo "${HOST_USER} ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/${HOST_USER} && \
    chmod 440 /etc/sudoers.d/${HOST_USER}

# Configure SSH keys directory
RUN mkdir -p /home/${HOST_USER}/.ssh && \
    chmod 700 /home/${HOST_USER}/.ssh

# Configure sshd on custom port
RUN sed -i "s/^#Port 22/Port ${SSH_PORT}/" /etc/ssh/sshd_config && \
    mkdir -p /run/sshd

# Boot script — starts services without systemd.
# This is the key design decision: no systemd means no systemd-binfmt,
# which means no binfmt_misc WSLInterop breakage.
RUN printf '#!/bin/bash\nexport PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\n# Load KVM module — detect Intel vs AMD\nif grep -q "vmx" /proc/cpuinfo 2>/dev/null; then\n  modprobe kvm_intel nested=1 2>/dev/null || true\nelif grep -q "svm" /proc/cpuinfo 2>/dev/null; then\n  modprobe kvm_amd nested=1 2>/dev/null || true\nfi\n# Fix /dev/kvm permissions — WSL does not run udev so device appears as root:root\nif [ -c /dev/kvm ]; then chown root:kvm /dev/kvm && chmod 660 /dev/kvm; fi\nmkdir -p /run/dbus /run/sshd\ndbus-daemon --system --fork 2>/dev/null\n/usr/sbin/virtlogd -d 2>/dev/null\n/usr/sbin/libvirtd -d 2>/dev/null\nip link delete virbr0 2>/dev/null || true\nsleep 2 && virsh net-start default 2>/dev/null || true\n/usr/sbin/sshd -p %s\n' "${SSH_PORT}" > /usr/local/bin/wsl-services.sh && \
    chmod +x /usr/local/bin/wsl-services.sh

# Write wsl.conf — NO systemd, services started via boot command
RUN printf '[user]\ndefault=%s\n\n[boot]\ncommand=/usr/local/bin/wsl-services.sh\n\n[interop]\nenabled=true\n' "${HOST_USER}" > /etc/wsl.conf

# Create libvirt default pool directory
RUN mkdir -p /var/lib/libvirt/images

# Prepare helios-engvm working directory
RUN mkdir -p /home/${HOST_USER}/helios-engvm/input \
             /home/${HOST_USER}/helios-engvm/tmp && \
    chown -R ${HOST_USER}:${HOST_USER} /home/${HOST_USER}/helios-engvm

# Fix ownership on home directory
RUN chown -R ${HOST_USER}:${HOST_USER} /home/${HOST_USER}
DOCKERFILE

    echo "$HOST_PUBKEY" > "$DOCKER_CONTEXT/authorized_keys"

    cat >> "$DOCKER_CONTEXT/Dockerfile" <<DOCKERFILE_KEYS

# Copy SSH public key
COPY authorized_keys /home/${HOST_USER}/.ssh/authorized_keys
RUN chown ${HOST_USER}:${HOST_USER} /home/${HOST_USER}/.ssh/authorized_keys && \\
    chmod 600 /home/${HOST_USER}/.ssh/authorized_keys
DOCKERFILE_KEYS

    # Use legacy builder — BuildKit has connection issues with Docker Desktop on WSL
    DOCKER_BUILDKIT=0 docker build \
        --build-arg HOST_USER="$HOST_USER" \
        --build-arg HOST_UID="$HOST_UID" \
        --build-arg SSH_PORT="$SSH_PORT" \
        -t "$DOCKER_IMAGE" \
        "$DOCKER_CONTEXT"

    rm -rf "$DOCKER_CONTEXT"
    trap - EXIT
fi

# =====================================================================
# STEP 2: Export rootfs (no wsl.exe calls)
# =====================================================================
echo ""
echo "=== Step 2: Export rootfs ==="

if [ -f "$ROOTFS_TAR" ]; then
    echo "  Rootfs tar already exists ($(du -h "$ROOTFS_TAR" | cut -f1)), skipping export."
else
    CONTAINER_ID=$(docker create "$DOCKER_IMAGE")
    docker export "$CONTAINER_ID" > "$ROOTFS_TAR"
    docker rm "$CONTAINER_ID" > /dev/null
    echo "  Exported: $(du -h "$ROOTFS_TAR" | cut -f1)"
fi

# =====================================================================
# STEP 3: WSL import (needs wsl.exe)
# =====================================================================
echo ""
echo "=== Step 3: WSL import ==="

if instance_exists; then
    if [[ "${1:-}" == "--rebuild" ]]; then
        echo "  Destroying existing instance..."
        wsl.exe --terminate "$INSTANCE_NAME" 2>/dev/null || true
        wsl.exe --unregister "$INSTANCE_NAME" 2>/dev/null || true
        echo "  Destroyed."
    else
        echo "  Instance already exists, skipping import."
        echo "  (Use --rebuild to recreate.)"
    fi
fi

if ! instance_exists; then
    echo "  Creating install directory..."
    powershell.exe -NoProfile -Command \
        "New-Item -ItemType Directory -Force -Path '$WSL_INSTALL_PATH'" > /dev/null 2>&1 || true

    echo "  Importing rootfs..."
    wsl.exe --import "$INSTANCE_NAME" "$WSL_INSTALL_PATH" "$ROOTFS_TAR" 2>&1 | tr -d '\0\r'
    echo "  Imported."
fi

# =====================================================================
# STEP 4: Start instance and wait for SSH
# =====================================================================
echo ""
echo "=== Step 4: Start instance ==="

# All WSL instances share eth0 — get our own IP
ETH0_IP=$(ip -4 addr show eth0 | grep -oP '(?<=inet )\d+\.\d+\.\d+\.\d+')
WSL_KEEPALIVE_PID=""

# Check if sshd is already reachable (instance may already be running)
if ssh -o ConnectTimeout=2 -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
       -p "$SSH_PORT" "$HOST_USER@$ETH0_IP" true 2>/dev/null; then
    echo "  Instance already running (SSH reachable)."
else
    # Clear stale host keys (rebuild generates new SSH host keys)
    ssh-keygen -f "$HOME/.ssh/known_hosts" -R "[$ETH0_IP]:$SSH_PORT" 2>/dev/null || true

    # Start the instance with a keepalive process.
    # The [boot] command= in wsl.conf starts sshd and libvirtd.
    # sleep infinity keeps the instance alive until SSH connects.
    wsl.exe -d "$INSTANCE_NAME" -- bash -c "sleep infinity" &
    WSL_KEEPALIVE_PID=$!
    echo "  Instance started (keepalive PID: $WSL_KEEPALIVE_PID)."

    echo "  Shared eth0 IP: $ETH0_IP"
    echo "  Waiting for sshd on port $SSH_PORT..."

    SSH_READY=false
    for i in $(seq 1 30); do
        if ssh -o ConnectTimeout=2 -o StrictHostKeyChecking=accept-new \
               -o BatchMode=yes -p "$SSH_PORT" "$HOST_USER@$ETH0_IP" true 2>/dev/null; then
            SSH_READY=true
            break
        fi
        sleep 2
        printf "  attempt %d/30...\r" "$i"
    done
    echo ""

    if [ "$SSH_READY" = false ]; then
        echo "ERROR: SSH did not become available within 60 seconds."
        echo "  Try manually: ssh -p $SSH_PORT $HOST_USER@$ETH0_IP"
        [ -n "$WSL_KEEPALIVE_PID" ] && kill "$WSL_KEEPALIVE_PID" 2>/dev/null || true
        exit 1
    fi

    echo "  SSH connected."
fi

# =====================================================================
# STEP 5: Post-import setup via SSH (no wsl.exe)
# =====================================================================
echo ""
echo "=== Step 5: Configuring libvirt via SSH ==="

ssh -p "$SSH_PORT" "$HOST_USER@$ETH0_IP" bash <<'SETUP'
# Clean stale bridge if present (survives WSL restarts, blocks net-start)
sudo ip link delete virbr0 2>/dev/null || true

# Start and configure libvirt default network
sudo virsh net-start default 2>/dev/null || true
sudo virsh net-autostart default 2>/dev/null || true

# Create and start default storage pool (if not already defined)
if ! sudo virsh pool-info default >/dev/null 2>&1; then
    sudo virsh pool-define-as default dir --target /var/lib/libvirt/images
fi
sudo virsh pool-autostart default 2>/dev/null || true
sudo virsh pool-start default 2>/dev/null || true

echo ""
echo "  Network: $(sudo virsh net-list --name 2>/dev/null | head -1)"
echo "  Pool:    $(sudo virsh pool-list --name 2>/dev/null | head -1)"
echo "  KVM:     $(test -c /dev/kvm && echo 'available' || echo 'not available')"
SETUP

# =====================================================================
# STEP 6: Verify interop survived
# =====================================================================
echo ""
echo "=== Step 6: Verify ==="

if [ -e /proc/sys/fs/binfmt_misc/WSLInterop ]; then
    echo "  WSL interop: OK (binfmt_misc intact)"
else
    echo "  WARNING: WSL interop was destroyed. This should not happen without systemd."
    echo "  Investigate before proceeding."
fi

# =====================================================================
# STEP 7: Summary
# =====================================================================
echo ""
echo "=== helios-host is ready ==="
echo ""
echo "  Hypervisor SSH:  ssh -p $SSH_PORT $HOST_USER@$ETH0_IP"
echo ""
echo "  Next steps:"
echo "    1. Copy Helios seed image into the instance"
echo "    2. Create Helios VM via virsh"
echo "    3. SSH directly to Helios VM at 192.168.122.x"
echo ""

# Clean up
rm -f "$ROOTFS_TAR"
[ -n "${WSL_KEEPALIVE_PID:-}" ] && kill "$WSL_KEEPALIVE_PID" 2>/dev/null || true

echo "=== Done ==="
