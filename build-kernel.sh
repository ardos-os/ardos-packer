#!/usr/bin/env bash
set -euo pipefail

# ========================================
# Ardos OS Kernel Build Script
# ========================================
# Compila um kernel "headless" (sem TTYs visuais),
# com saída via serial e suporte a EFI / virtio.
# ========================================

KERNEL_VERSION="v6.12"          # Podes mudar aqui
KERNEL_DIR="linux-${KERNEL_VERSION}"
BUILD_DIR="$(pwd)/build"

# ----------------------------------------
# 1️⃣ Obter fonte do kernel
# ----------------------------------------
if [ ! -d "$KERNEL_DIR" ]; then
    echo "==> Clonando Linux kernel $KERNEL_VERSION..."
    git clone --depth=1 --branch "$KERNEL_VERSION" \
        https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git "$KERNEL_DIR"
fi

cd "$KERNEL_DIR"

# ----------------------------------------
# 2️⃣ Base config
# ----------------------------------------
echo "==> Gerando config base..."
make x86_64_defconfig

# ----------------------------------------
# 3️⃣ Aplicar opções Ardos OS
# ----------------------------------------
echo "==> Aplicando flags Ardos OS..."
scripts/config \
  --disable VT \
  --disable VT_CONSOLE \
  --disable VGA_CONSOLE \
  --disable HW_CONSOLE \
  --disable FRAMEBUFFER_CONSOLE \
  --disable UNIX98_PTYS \
  --disable LEGACY_PTYS \
  --enable TTY \
  --enable SERIAL_CORE \
  --enable SERIAL_8250 \
  --enable SERIAL_8250_CONSOLE \
  --enable EFI \
  --enable EFI_STUB \
  --enable EFI_MIXED \
  --enable EFI_PARTITION \
  --enable EFI_VARS \
  --enable EFI_GENERIC_STUB_INITRD_CMDLINE_LOADER \
  --enable VIRTIO_PCI \
  --enable VIRTIO_MMIO \
  --enable VIRTIO_NET \
  --enable VIRTIO_BLK \
  --enable VIRTIO_CONSOLE \
  --enable DRM_VIRTIO_GPU \
  --enable BLK_DEV_INITRD \
  --disable INITRAMFS_SOURCE \
  --enable EARLY_PRINTK \
  --enable EARLY_PRINTK_EFI \
  --enable DEBUG_INFO_BTF \
  --enable KALLSYMS \
  --enable PANIC_TIMEOUT

# ----------------------------------------
# 4️⃣ Regerar dependências
# ----------------------------------------
make olddefconfig

# ----------------------------------------
# 5️⃣ Compilar
# ----------------------------------------
echo "==> Compilando kernel..."
make -j"$(nproc)"

# ----------------------------------------
# 6️⃣ Copiar resultado
# ----------------------------------------
mkdir -p "$BUILD_DIR"
cp arch/x86/boot/bzImage "$BUILD_DIR/kernel.efi"

echo
echo "✅ Kernel compilado com sucesso!"
echo "  Saída: $BUILD_DIR/kernel.efi"
echo
echo "Para testar no QEMU:"
echo "  qemu-system-x86_64 -enable-kvm -m 2G -bios /usr/share/OVMF/OVMF.fd \\"
echo "     -serial mon:stdio -drive file=build/ardos.qcow2,format=qcow2"
echo
echo "Para bootar com Limine:"
echo "  CMDLINE=console=ttyS0,115200n8 earlyprintk=serial,ttyS0 quiet loglevel=3 panic=10"

