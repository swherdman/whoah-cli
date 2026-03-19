# whoah-cli — We Have Oxide At Home CLI

> [!WARNING]
> **Pre-alpha software.** This project is under active development and not yet ready for general use. If you're interested in running Oxide at home and want to get involved, [reach out](https://swherdman.com/portfolio/whoah/).

A TUI/CLI tool for deploying [Oxide](https://oxide.computer) on non-Oxide hardware. Automates the full lifecycle from VM provisioning through Omicron build and deployment, so you can run a real Oxide rack at home.

![WHOAH TUI](https://swherdman.com/images/notion/3286a62cb72d8065920ddc628ed60168.webp)

## What it does

WHOAH automates the multi-step process of deploying Oxide's control plane on commodity hardware:

1. **Provision** — Creates a VM on your hypervisor, installs Helios (Oxide's illumos distro), and configures networking
2. **Configure** — Sets up SSH access, user accounts, and package caching for faster builds
3. **Build** — Installs prerequisites, clones Omicron, applies configuration overrides, and compiles the full Oxide stack
4. **Patch** — Applies necessary patches for non-native hardware (e.g., propolis string I/O emulation for nested virtualization)

The TUI provides real-time progress tracking with streaming build output, a debug screen for SSH session monitoring, and a dashboard for monitoring your deployment.

## Supported platforms

| Hypervisor | Status |
|---|---|
| Proxmox VE | Supported |
| Others | Planned |

## Quick start

### Prerequisites

- A Proxmox VE host with enough resources (4 cores, 48GB RAM, 256GB disk recommended)
- The [Helios install ISO](https://docs.oxide.computer) available on your Proxmox storage
- Rust toolchain on your local machine

### Install

```bash
git clone https://github.com/swherdman/whoah-cli.git
cd whoah-cli
cargo build --release
```

### Initialize a deployment

```bash
./target/release/whoah init
```

This creates a deployment configuration at `~/.whoah/deployments/<name>/` with:
- `deployment.toml` — network, hypervisor, and VM settings
- `build.toml` — Omicron build configuration and overrides

### Deploy

```bash
./target/release/whoah
```

Launch the TUI, navigate to the Build tab (`1`), and press `b` to start the pipeline. The tool handles everything from VM creation through a running Oxide control plane.

## Configuration

### deployment.toml

```toml
[network]
gateway = "192.168.1.1"
external_dns_ips = ["192.168.1.40", "192.168.1.41"]
infra_ip = "192.168.1.50"
internal_services_range = { first = "192.168.1.40", last = "192.168.1.49" }
instance_pool_range = { first = "192.168.1.51", last = "192.168.1.60" }

[proxmox]
host = "192.168.1.5"
ssh_user = "root"
node = "pve"

[proxmox.vm]
vmid = 302
name = "helios"
cores = 2
sockets = 2
memory_mb = 49152
disk_gb = 256
```

### build.toml

```toml
[omicron]
repo_path = "~/omicron"

[omicron.overrides]
cockroachdb_redundancy = 3
control_plane_storage_buffer_gib = 5
vdev_count = 3
vdev_size_bytes = 42949672960
```

## TUI Screens

| Key | Screen | Description |
|---|---|---|
| `1` | Build | Pipeline progress with streaming output |
| `2` | Config | Configuration browser |
| `3` | Monitor | Dashboard with zone, disk, and service status |
| `d` | Debug | Live SSH sessions, mux masters, Docker containers |

## Architecture

- **Build pipeline** — Phased execution model (Provision → Configure → Build → Patch) with per-step progress tracking
- **SSH** — `DirectSsh` for build commands (system `ssh` binary with OS-level ControlMaster), `SshHost` (openssh crate) for monitoring
- **Serial console** — Automated Helios installation via socat over SSH to Proxmox
- **Package caching** — Local nginx reverse proxy + squid SSL-bump forward proxy for build acceleration
- **Patched propolis** — Automated build pipeline via GitHub Actions on a self-hosted illumos runner ([swherdman/propolis](https://github.com/swherdman/propolis))

## Links

- [Project page](https://swherdman.com/portfolio/whoah/)
- [Oxide Computer](https://oxide.computer)
- [Omicron](https://github.com/oxidecomputer/omicron) — Oxide's control plane
- [Propolis fork](https://github.com/swherdman/propolis) — Patched VMM with pre-built releases

## License

[MIT](LICENSE)
