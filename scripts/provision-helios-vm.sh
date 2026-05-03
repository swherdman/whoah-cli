#!/bin/bash
#
# Provision a Helios VM on the helios-host WSL instance via libvirt/KVM.
#
# Usage: ./provision-helios-vm.sh [--rebuild]
#   --rebuild: destroy existing VM and volumes, recreate from seed image
#
# Run from any WSL instance (shares the eth0 network namespace with helios-host).
# All VM operations execute on helios-host via SSH.
#
set -euo pipefail

# ─── VM configuration ───────────────────────────────────────────────────────
VM_NAME="helios"
VM_MEMORY_MB=20480        # 20GB — minimum viable for Omicron
VM_VCPUS=4
VM_DISK_GB=128
POOL_PATH="/var/lib/libvirt/images"
LIBVIRT_POOL="default"
LIBVIRT_NET="default"

# ─── helios-host connection ──────────────────────────────────────────────────
SSH_PORT=2222
HOST_USER="$(whoami)"
HOST_UID="$(id -u)"
ETH0_IP=$(ip -4 addr show eth0 | grep -oP '(?<=inet )\d+\.\d+\.\d+\.\d+')

# ─── Seed image ──────────────────────────────────────────────────────────────
SEED_GZ="helios-qemu-ttya-full-20231220.raw.gz"
SEED_URL="https://pkg.oxide.computer/seed/helios-qemu-ttya-full-20231220.raw.gz"
SEED_SHA256="51fc4ead25c1b3ba5d22e79a1cd069ad46b310a084246f612e7ff287085a190f"
WORK_DIR="/home/${HOST_USER}/helios-engvm/tmp"

REBUILD=""
for arg in "$@"; do
    case "$arg" in
        --rebuild) REBUILD=1 ;;
        *) echo "Usage: $0 [--rebuild]"; exit 1 ;;
    esac
done

# ─── SSH public key ──────────────────────────────────────────────────────────
HOST_PUBKEY=$(cat ~/.ssh/id_rsa.pub 2>/dev/null || cat ~/.ssh/id_ed25519.pub 2>/dev/null) || {
    echo "ERROR: No SSH public key found in ~/.ssh/"
    exit 1
}

# ─── Helpers ─────────────────────────────────────────────────────────────────
ssh_host() {
    ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
        -p "$SSH_PORT" "${HOST_USER}@${ETH0_IP}" "$@"
}

# ─── Step 1: Ensure helios-host is running ───────────────────────────────────
echo "=== Step 1: helios-host ==="

WSL_KEEPALIVE_PID=""
cleanup() { [ -n "${WSL_KEEPALIVE_PID:-}" ] && kill "$WSL_KEEPALIVE_PID" 2>/dev/null || true; }
trap cleanup EXIT

if ssh -o ConnectTimeout=2 -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
       -p "$SSH_PORT" "${HOST_USER}@${ETH0_IP}" true 2>/dev/null; then
    echo "  helios-host already running."
else
    ssh-keygen -f "$HOME/.ssh/known_hosts" -R "[${ETH0_IP}]:${SSH_PORT}" 2>/dev/null || true
    wsl.exe -d helios-host -- bash -c "sleep infinity" &
    WSL_KEEPALIVE_PID=$!
    echo "  Starting helios-host (keepalive PID: $WSL_KEEPALIVE_PID)..."

    SSH_READY=false
    for i in $(seq 1 30); do
        if ssh -o ConnectTimeout=2 -o StrictHostKeyChecking=accept-new \
               -o BatchMode=yes -p "$SSH_PORT" "${HOST_USER}@${ETH0_IP}" true 2>/dev/null; then
            SSH_READY=true
            break
        fi
        sleep 2
        printf "  attempt %d/30...\r" "$i"
    done
    echo ""
    [ "$SSH_READY" = false ] && { echo "ERROR: helios-host SSH did not come up."; exit 1; }
    echo "  Connected."
fi

# ─── Step 1b: Verify KVM is available ───────────────────────────────────────
echo ""
echo "=== Step 1b: KVM check ==="

KVM_OK=$(ssh_host 'test -c /dev/kvm && echo yes || echo no')
if [ "$KVM_OK" != "yes" ]; then
    echo "  ERROR: /dev/kvm not present in helios-host."
    echo ""
    echo "  This is a known WSL bug (microsoft/WSL#7509) — /dev/kvm disappears"
    echo "  after wsl --shutdown or Windows reboot despite nested virt being the"
    echo "  default. Fix: add nestedVirtualization=true to .wslconfig and restart."
    echo ""
    echo "  1. The .wslconfig has already been written to C:\\Users\\${HOST_USER}\\.wslconfig"
    echo "     (or create it if missing):"
    echo "       [wsl2]"
    echo "       nestedVirtualization=true"
    echo ""
    echo "  2. From PowerShell: wsl --shutdown"
    echo "  3. Reopen your WSL terminal and re-run this script."
    exit 1
fi
echo "  KVM available."

# ─── Step 2: Teardown if --rebuild ───────────────────────────────────────────
if [ -n "$REBUILD" ]; then
    echo ""
    echo "=== Step 2: Teardown (--rebuild) ==="
    ssh_host bash <<REMOTE
set -euo pipefail
if sudo virsh domstate "${VM_NAME}" 2>/dev/null | grep -q running; then
    echo "  Destroying running VM..."
    sudo virsh destroy "${VM_NAME}"
fi
if sudo virsh dominfo "${VM_NAME}" >/dev/null 2>&1; then
    echo "  Undefining VM..."
    sudo virsh undefine "${VM_NAME}"
fi
for vol in "${VM_NAME}.qcow2" "${VM_NAME}-metadata.cpio"; do
    if sudo virsh vol-info "\$vol" --pool "${LIBVIRT_POOL}" >/dev/null 2>&1; then
        echo "  Deleting volume \$vol..."
        sudo virsh vol-delete "\$vol" --pool "${LIBVIRT_POOL}"
    fi
done
echo "  Teardown complete."
REMOTE
fi

# ─── Step 3: Check if VM already exists ──────────────────────────────────────
echo ""
echo "=== Step 3: VM status ==="

VM_STATE=$(ssh_host "sudo virsh domstate '${VM_NAME}' 2>/dev/null || echo 'undefined'")

if [ "$VM_STATE" = "running" ]; then
    echo "  VM is already running. Skipping to IP discovery."

elif [ "$VM_STATE" = "shut off" ]; then
    echo "  VM exists (shut off). Starting..."
    ssh_host "sudo virsh start '${VM_NAME}'"

else
    # ─── Fresh creation ───────────────────────────────────────────────────────
    echo "  No VM found. Creating from seed image..."
    echo ""
    echo "=== Step 3a: Seed image ==="

    # Check if seed image is already on helios-host
    SEED_ON_HOST=$(ssh_host "test -f '${WORK_DIR}/${SEED_GZ}' && echo yes || echo no")

    if [ "$SEED_ON_HOST" != "yes" ]; then
        # Search locally for the seed image before failing
        LOCAL_SEED=""
        WIN_DOWNLOADS="/mnt/c/Users/${HOST_USER}/Downloads/${SEED_GZ}"
        LOCAL_CACHE="${HOME}/helios-engvm/input/${SEED_GZ}"

        if [ -f "$LOCAL_CACHE" ]; then
            LOCAL_SEED="$LOCAL_CACHE"
            echo "  Found seed image in local cache: $LOCAL_CACHE"
        elif [ -f "$WIN_DOWNLOADS" ]; then
            LOCAL_SEED="$WIN_DOWNLOADS"
            echo "  Found seed image in Windows Downloads: $WIN_DOWNLOADS"
        fi

        if [ -n "$LOCAL_SEED" ]; then
            echo "  Copying seed image to helios-host (this may take a few minutes)..."
            ssh_host "mkdir -p '${WORK_DIR}'"
            scp -P "$SSH_PORT" -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
                "$LOCAL_SEED" "${HOST_USER}@${ETH0_IP}:${WORK_DIR}/${SEED_GZ}"
            echo "  Copied."
        else
            echo "ERROR: Seed image not found."
            echo "  Searched:"
            echo "    helios-host: ${WORK_DIR}/${SEED_GZ}"
            echo "    local cache: ${LOCAL_CACHE}"
            echo "    Windows:     ${WIN_DOWNLOADS}"
            echo ""
            echo "  Download from: ${SEED_URL}"
            echo "  Then place at: ${LOCAL_CACHE}"
            exit 1
        fi
    fi

    ssh_host bash <<REMOTE
set -euo pipefail
cd "${WORK_DIR}"
SHA=\$(sha256sum "${SEED_GZ}" | cut -d' ' -f1)
if [ "\$SHA" != "${SEED_SHA256}" ]; then
    echo "  WARNING: SHA256 mismatch on seed image (got \$SHA)"
    echo "  Expected: ${SEED_SHA256}"
    echo "  Continuing anyway — may be a different image version."
fi
echo "  Seed image present: ${SEED_GZ} (\$(du -h "${SEED_GZ}" | cut -f1))"
REMOTE

    echo ""
    echo "=== Step 3b: Prepare volumes ==="

    ssh_host bash <<REMOTE
set -euo pipefail
cd "${WORK_DIR}"

# Convert seed to qcow2 if not already done
if [ ! -f "${VM_NAME}.qcow2" ]; then
    echo "  Extracting raw image..."
    gunzip -c "${SEED_GZ}" > "${VM_NAME}.raw"
    echo "  Converting to qcow2..."
    qemu-img convert -f raw -O qcow2 "${VM_NAME}.raw" "${VM_NAME}.qcow2"
    rm -f "${VM_NAME}.raw"
    echo "  Resizing to ${VM_DISK_GB}G..."
    qemu-img resize "${VM_NAME}.qcow2" "${VM_DISK_GB}G"
else
    echo "  qcow2 already exists (\$(du -h "${VM_NAME}.qcow2" | cut -f1)), skipping conversion."
fi

# Generate metadata CPIO
echo "  Generating metadata CPIO..."
CPIO_DIR=\$(mktemp -d)
trap 'rm -rf "\$CPIO_DIR"' EXIT

# authorized_keys — placed at /root/.ssh/authorized_keys by helios init,
# then copied to user's .ssh by firstboot.sh
cat > "\$CPIO_DIR/authorized_keys" <<'PUBKEY'
${HOST_PUBKEY}
PUBKEY
chmod 600 "\$CPIO_DIR/authorized_keys"

# nodename — sets VM hostname
echo "${VM_NAME}" > "\$CPIO_DIR/nodename"

# firstboot.sh — runs once on first Helios boot, creates user account
cat > "\$CPIO_DIR/firstboot.sh" <<'FIRSTBOOT'
#!/bin/bash
set -o errexit
set -o pipefail
set -o xtrace
echo "Just a moment..." >/dev/msglog
/sbin/zfs create "rpool/home/${HOST_USER}"
/usr/sbin/useradd -u "${HOST_UID}" -g staff -c "${HOST_USER}" -d "/home/${HOST_USER}" \
    -P "Primary Administrator" -s /bin/bash "${HOST_USER}"
/bin/passwd -N "${HOST_USER}"
/bin/mkdir "/home/${HOST_USER}/.ssh"
/bin/cp /root/.ssh/authorized_keys "/home/${HOST_USER}/.ssh/authorized_keys"
/bin/chown -R "${HOST_USER}:staff" "/home/${HOST_USER}"
/bin/chmod 0700 "/home/${HOST_USER}"
/bin/ntpdig -S 0.pool.ntp.org || true
echo "ok go" >/dev/msglog
FIRSTBOOT
chmod +x "\$CPIO_DIR/firstboot.sh"

# Build CPIO (newc format, relative paths)
( cd "\$CPIO_DIR" && find . -type f | cpio --quiet -o ) > "${VM_NAME}-metadata.cpio"
echo "  Metadata CPIO: \$(du -h "${VM_NAME}-metadata.cpio" | cut -f1)"
REMOTE

    echo ""
    echo "=== Step 3c: Upload volumes to libvirt pool ==="

    ssh_host bash <<REMOTE
set -euo pipefail
cd "${WORK_DIR}"

# Root disk
if sudo virsh vol-info "${VM_NAME}.qcow2" --pool "${LIBVIRT_POOL}" >/dev/null 2>&1; then
    echo "  Volume ${VM_NAME}.qcow2 already in pool, skipping upload."
else
    echo "  Creating volume ${VM_NAME}.qcow2..."
    QCOW2_SIZE=\$(du -b "${VM_NAME}.qcow2" | cut -f1)
    sudo virsh vol-create-as --pool "${LIBVIRT_POOL}" --name "${VM_NAME}.qcow2" \
        --capacity "${VM_DISK_GB}G" --format qcow2
    echo "  Uploading (\$(du -h "${VM_NAME}.qcow2" | cut -f1))..."
    sudo virsh vol-upload --pool "${LIBVIRT_POOL}" --vol "${VM_NAME}.qcow2" \
        --file "${VM_NAME}.qcow2"
    echo "  Uploaded."
fi

# Metadata disk
if sudo virsh vol-info "${VM_NAME}-metadata.cpio" --pool "${LIBVIRT_POOL}" >/dev/null 2>&1; then
    echo "  Volume ${VM_NAME}-metadata.cpio already in pool, skipping upload."
else
    echo "  Uploading metadata CPIO..."
    sudo virsh vol-create-as --pool "${LIBVIRT_POOL}" --name "${VM_NAME}-metadata.cpio" \
        --capacity 1M --format raw
    sudo virsh vol-upload --pool "${LIBVIRT_POOL}" --vol "${VM_NAME}-metadata.cpio" \
        --file "${VM_NAME}-metadata.cpio"
    echo "  Uploaded."
fi
REMOTE

    echo ""
    echo "=== Step 3d: Define and start VM ==="

    ssh_host bash <<REMOTE
set -euo pipefail

cat > /tmp/${VM_NAME}.xml <<'VMXML'
<domain type='kvm'>
  <name>${VM_NAME}</name>
  <memory unit='MiB'>${VM_MEMORY_MB}</memory>
  <vcpu>${VM_VCPUS}</vcpu>
  <os>
    <type arch='x86_64'>hvm</type>
    <boot dev='hd'/>
  </os>
  <features><acpi/><apic/></features>
  <cpu mode='host-model' check='partial'/>
  <clock offset='utc'>
    <timer name='rtc' tickpolicy='catchup'/>
    <timer name='pit' tickpolicy='delay'/>
    <timer name='hpet' present='yes'/>
  </clock>
  <on_poweroff>destroy</on_poweroff>
  <on_reboot>restart</on_reboot>
  <on_crash>destroy</on_crash>
  <devices>
    <emulator>/usr/bin/qemu-system-x86_64</emulator>
    <disk type='file' device='disk'>
      <driver name='qemu' type='qcow2'/>
      <source file='${POOL_PATH}/${VM_NAME}.qcow2'/>
      <target dev='vda' bus='virtio'/>
    </disk>
    <disk type='file' device='disk'>
      <driver name='qemu' type='raw'/>
      <source file='${POOL_PATH}/${VM_NAME}-metadata.cpio'/>
      <target dev='vdb' bus='virtio'/>
    </disk>
    <interface type='network'>
      <source network='${LIBVIRT_NET}'/>
      <model type='virtio'/>
    </interface>
    <serial type='pty'><target port='0'/></serial>
    <console type='pty'><target type='serial' port='0'/></console>
  </devices>
</domain>
VMXML

sudo virsh define /tmp/${VM_NAME}.xml
rm -f /tmp/${VM_NAME}.xml
echo "  VM defined."
sudo virsh start "${VM_NAME}"
echo "  VM started."
REMOTE
fi

# ─── Step 4: Wait for VM IP ───────────────────────────────────────────────────
echo ""
echo "=== Step 4: Waiting for VM IP ==="

VM_IP=""
for i in $(seq 1 60); do
    VM_IP=$(ssh_host "sudo virsh domifaddr '${VM_NAME}' 2>/dev/null" \
        | grep -oP '\d+\.\d+\.\d+\.\d+(?=/\d+)' || true)
    if [ -n "$VM_IP" ]; then
        echo "  VM IP: $VM_IP"
        break
    fi
    sleep 5
    printf "  attempt %d/60 (waiting for DHCP)...\r" "$i"
done
echo ""

if [ -z "$VM_IP" ]; then
    echo "ERROR: VM did not get a DHCP lease within 5 minutes."
    echo "  Check serial console: ssh -p $SSH_PORT ${HOST_USER}@${ETH0_IP} 'sudo virsh console ${VM_NAME}'"
    exit 1
fi

# ─── Step 5: Wait for SSH on VM ───────────────────────────────────────────────
echo "=== Step 5: Waiting for SSH on VM ==="

SSH_VM_READY=false
for i in $(seq 1 60); do
    if ssh -o ConnectTimeout=3 -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
           "${HOST_USER}@${VM_IP}" true 2>/dev/null; then
        SSH_VM_READY=true
        break
    fi
    sleep 5
    printf "  attempt %d/60...\r" "$i"
done
echo ""

if [ "$SSH_VM_READY" = false ]; then
    echo "ERROR: SSH to Helios VM did not come up within 5 minutes."
    echo "  VM IP: $VM_IP"
    echo "  Try manually: ssh ${HOST_USER}@${VM_IP}"
    echo "  Serial console: ssh -p $SSH_PORT ${HOST_USER}@${ETH0_IP} 'sudo virsh console ${VM_NAME}'"
    exit 1
fi

# ─── Done ─────────────────────────────────────────────────────────────────────
echo ""
echo "=== Helios VM is ready ==="
echo ""
echo "  VM SSH:          ssh ${HOST_USER}@${VM_IP}"
echo "  Hypervisor SSH:  ssh -p ${SSH_PORT} ${HOST_USER}@${ETH0_IP}"
echo "  Serial console:  ssh -p ${SSH_PORT} ${HOST_USER}@${ETH0_IP} 'sudo virsh console ${VM_NAME}'"
echo ""
echo "  VM:    $(ssh -o BatchMode=yes "${HOST_USER}@${VM_IP}" 'uname -a' 2>/dev/null || echo '(uname failed)')"
echo ""
