
# Ardos Packer

**Ardos Packer** is the official build system of the **Ardos OS** operating system.
It compiles the kernel, assembles the immutable system image, builds the initramfs, and can boot the OS inside a QEMU VM for testing — all through a unified CLI.

---

## ✨ Features

- **Modular package management**:
  - Local PKGBUILDs
  - Remote Git repositories
  - Precompiled Arch Linux binary packages
- **Incremental build** with cached sources
- **Final system image** built as a SquashFS filesystem
- **Containerized kernel build pipeline** (Docker)
- **Initrd build automation** via manifest-defined script
- **Fully automated VM boot** (kernel + image + initrd + UEFI)
- **Unified CLI** with intuitive subcommands

---

## 🧭 Command Structure

```bash
ardos-packer <command> [subcommand] [options]
```

| Main Command | Description                              |
| ------------ | ---------------------------------------- |
| `image`      | Image and package management operations  |
| `kernel`     | Build the kernel defined in the manifest |
| `initrd`     | Build the initramfs using a script       |
| `vm`         | Virtual machine utilities (QEMU)         |
| `clean`      | Remove the build directory               |

### `image` Subcommands

| Subcommand       | Description                                                   |
| ---------------- | ------------------------------------------------------------- |
| `assemble`       | Builds all packages and assembles the final `.squashfs` image |
| `packages fetch` | Pre-downloads all sources and validates the manifest          |
| `packages build` | Builds all packages without assembling the image              |
| `packages gc`    | Removes unused source tarballs                                |
| `push`           | *(Unimplemented)* Pushes the image to an update server        |

### `kernel` Subcommands

| Subcommand | Description                                       |
| ---------- | ------------------------------------------------- |
| `build`    | Compiles the Linux kernel defined in the manifest |

### `initrd` Subcommands

| Subcommand | Description                                                             |
| ---------- | ----------------------------------------------------------------------- |
| `build`    | Runs the build script defined in the manifest to generate the initramfs |

### `vm` Subcommands

| Subcommand | Description                                                               |
| ---------- | ------------------------------------------------------------------------- |
| `run`      | Builds everything (kernel, initrd, image) and launches the system in QEMU |
| `reset`    | Recreates the user data disk (`user.qcow2`)                               |

---

## 🧾 Example Manifest (`manifest.toml`)

```toml
version = "0.1-dev"

[kernel]
url = "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.10.tar.xz"

[kernel.options]
DEBUG_INFO = false
KALLSYMS_ALL = false

[initrd]
build_script = "scripts/build-initramfs.sh"

[[packages]]
name = "glibc"
version = "2.39"
source = { type = "binary", url = "https://archlinux.org/packages/core/x86_64/glibc/download" }

[[packages]]
name = "mesa"
version = "git"
source = { type = "git", repo = "https://gitlab.freedesktop.org/mesa/mesa.git", rev = "main" }

[[packages]]
name = "tibs"
version = "0.1"
source = { type = "pkgbuild", path = "./pkgs/tibs" }
```

---

## ⚙️ Requirements

* **Rust Compiler**
* **Docker** (for kernel and package builds)
* **squashfs-tools** (for final image creation)
* **QEMU** (for VM testing)

---

## 🚀 Usage Examples

```bash
# Download all sources
ardos-packer image packages fetch

# Build packages only
ardos-packer image packages build

# Assemble the final system image
ardos-packer image assemble

# Build the kernel
ardos-packer kernel build

# Build the initramfs
ardos-packer initrd build

# Run the full system inside a UEFI QEMU VM
ardos-packer vm run

# Recreate the VM user data disk
ardos-packer vm reset

# Clean the build directory
ardos-packer clean
```

All build artifacts are stored inside the `./build` directory.

---

## 📁 Generated Directory Layout

```
build/
 ├── downloads/      # Source tarballs
 ├── src/            # Source code and temporary build trees
 ├── out/            # Build artifacts
 ├── images/         # Final SquashFS system image
 ├── kernel/         # Kernel build output
 ├── vm/             # Virtual machine files (OVMF, qcow2 disks, etc.)
 └── sysroot/        # Temporary root used during image assembly
```

---

## 📜 License

Ardos Packer is distributed under the **MIT License**.
