// src/vm.rs
//! Ardos Packer - vm run
//! - system.qcow2 hardcoded em build/vm/system.qcow2
//! - user disk path passado nas opções (criado por vm reset)
//! - usa run_privileged_script() para operações que exigem root (uma única elevação)
//! - não assume sudo: tenta sudo -> doas -> su -c

use std::{
	fs::{self, File},
	io::{self, Write},
	os::unix::fs::PermissionsExt,
	path::{Path, PathBuf},
	process::{Command, ExitStatus, Stdio},
	sync::OnceLock,
};

use colored::Colorize;
use thiserror::Error;

// -----------------------------
// Opções e erros
// -----------------------------

#[derive(Debug)]
pub struct RunCommandOptions {
	/// Limine EFI binary (BOOTX64.EFI)
	pub bootloader_path: PathBuf,
	/// OVMF CODE (readonly pflash)
	pub ovmf_code_path: PathBuf,
	/// OVMF VARS (writable pflash)
	pub ovmf_vars_path: PathBuf,
	/// Kernel (vmlinuz)
	pub kernel_path: PathBuf,
	/// Caminho para image squashfs gerada pelo assemble
	pub image_path: PathBuf,
	/// Initramfs (retornado por initrd::build_initrd)
	pub initrd_path: PathBuf,
	/// Caminho do disco de user (criado por vm reset)
	pub user_disk_path: PathBuf,
	/// Argumentos extra para qemu
	pub extra_qemu_args: Vec<String>,
}

#[derive(Error, Debug)]
pub enum RunCommandError {
	#[error("I/O error: {0}")]
	Io(#[from] io::Error),

	#[error("qemu exited with non-zero status {0}")]
	QemuNonZero(ExitStatus),

	#[error("missing required file: {0}")]
	MissingFile(String),
}

// -----------------------------
// Privilege helpers
// -----------------------------

static PRIV_TOOL: OnceLock<String> = OnceLock::new();

/// Tenta executar um comando com privilégio, cacheando a ferramenta que funcionar.
pub fn run_as_root(args: &[&str]) -> io::Result<ExitStatus> {
	if let Some(tool) = PRIV_TOOL.get() {
		return run_with_tool(tool, args);
	}

	for tool in ["sudo", "doas", "su"] {
		if let Ok(status) = run_with_tool(tool, args) {
			PRIV_TOOL.set(tool.to_string()).ok();
			return Ok(status);
		}
	}

	Err(io::Error::new(
		io::ErrorKind::NotFound,
		"no privilege escalation tool found",
	))
}

fn run_with_tool(tool: &str, args: &[&str]) -> io::Result<ExitStatus> {
	match tool {
		"sudo" | "doas" => Command::new(tool).args(args).status(),
		"su" => {
			let joined = args
				.iter()
				.map(|a| format!("'{}'", a.replace('\'', "'\\''")))
				.collect::<Vec<_>>()
				.join(" ");
			Command::new("su").args(["-c", &joined]).status()
		}
		_ => unreachable!(),
	}
}

/// Agrupa vários comandos num único script e executa-o com privilégio (uma só elevação).
pub fn run_privileged_script(commands: &[&str]) -> io::Result<ExitStatus> {
	let tmp_path = std::env::temp_dir().join("ardos-packer-vm-setup.sh");
	{
		let mut file = File::create(&tmp_path)?;
		writeln!(file, "#!/usr/bin/env bash")?;
		writeln!(file, "set -euo pipefail")?;
		for &cmd in commands {
			writeln!(file, "{}", cmd)?;
		}
		file.flush()?;

		let mut perms = file.metadata()?.permissions();
		perms.set_mode(0o700);
		fs::set_permissions(&tmp_path, perms)?;
	}
	let script_str = tmp_path.to_string_lossy().to_string();
	let status = run_as_root(&["su", "-c", &script_str])?;

	let _ = fs::remove_file(&tmp_path);
	Ok(status)
}

// -----------------------------
// Função principal: run_command
// -----------------------------

pub fn run_command(opts: RunCommandOptions) -> Result<(), RunCommandError> {
	let system_disk = Path::new("build/vm/system.qcow2");
	let user_disk = opts.user_disk_path.as_path();

	// valida artefactos obrigatórios
	for p in [
		&opts.bootloader_path,
		&opts.ovmf_code_path,
		&opts.ovmf_vars_path,
		&opts.kernel_path,
		&opts.image_path,
		&opts.initrd_path,
	] {
		if !p.exists() {
			return Err(RunCommandError::MissingFile(format!("{}", p.display())));
		}
	}

	fs::create_dir_all("build/vm").map_err(RunCommandError::Io)?;

	// Cria o system.qcow2 se não existir
	println!("{}", " Creating system qcow2 image".blue().bold());

	let setup_script = [
			"set -xe",
			"qemu-nbd --disconnect /dev/nbd0 || true",
			"qemu-nbd --disconnect /dev/nbd1 || true",
			"umount /mnt/ardos-user || true",
			"umount /mnt/ardos-vm || true",
			"modprobe -r nbd || true",
			"sleep 0.1s",
			"modprobe nbd",
			"qemu-img create -f qcow2 build/vm/system.qcow2 2G",
			"chmod 777 build/vm -R",
			&format!("qemu-nbd --connect /dev/nbd1 {}", user_disk.display()),
			"qemu-nbd --connect /dev/nbd0 build/vm/system.qcow2",
			"sleep 0.1s",
			"parted -s /dev/nbd0 mklabel gpt",
			"parted -s /dev/nbd0 mkpart EFI fat32 1MiB 300MiB",
			"parted -s /dev/nbd0 set 1 esp on",
			"parted -s /dev/nbd0 mkpart SYSTEM btrfs 300MiB 100%",
			"sleep 0.1s",
			"mkfs.vfat -F32 /dev/nbd0p1",
			"mkfs.btrfs -f /dev/nbd0p2",
			"mkdir -p /mnt/ardos-vm",
			"mount /dev/nbd0p1 /mnt/ardos-vm",
			"mkdir -p /mnt/ardos-vm/EFI/BOOT",
			&format!(
				"cp {} /mnt/ardos-vm/EFI/BOOT/BOOTX64.EFI",
				opts.bootloader_path.display()
			),
			&format!("cp {} /mnt/ardos-vm/vmlinuz", opts.kernel_path.display()),
			&format!(
				"cp {} /mnt/ardos-vm/initramfs.img",
				opts.initrd_path.display()
			),
			"SYSTEM_PARTITION=$(blkid -s PARTUUID -o value /dev/nbd0p2)",
			"USER_PARTITION=$(blkid -s PARTUUID -o value /dev/nbd1p1 || true)",
			"cat > /mnt/ardos-vm/limine.conf <<EOF
timeout: 0
/Ardos
    protocol: linux
    path: boot():/vmlinuz
    cmdline: console=ttyS0 system_partition=UUID=$SYSTEM_PARTITION user_partition=UUID=$USER_PARTITION
    module_path: boot():/initramfs.img
EOF",
			"umount /mnt/ardos-vm || true",
			"mount /dev/nbd0p2 /mnt/ardos-vm",
			&format!(
				"cp {} /mnt/ardos-vm/system.squashfs",
				opts.image_path.display()
			),
			"umount /mnt/ardos-vm || true",
			"qemu-nbd --disconnect /dev/nbd0 || true",
			"qemu-nbd --disconnect /dev/nbd1 || true",
			"sleep 0.2s"
		];

	run_privileged_script(&setup_script).map_err(RunCommandError::Io)?;
	println!("{}", "✔ System qcow2 ready".green().bold());

	if !user_disk.exists() {
		eprintln!(
			"{} {}",
			" Missing user disk".yellow().bold(),
			"(run `ardos-packer vm reset` first)".dimmed()
		);
		return Err(RunCommandError::MissingFile(format!(
			"{} missing",
			user_disk.display()
		)));
	}

	println!("{}", "🚀 Launching QEMU".blue().bold());
	let mut args: Vec<String> = vec![
		"-enable-kvm".into(),
		"-cpu".into(),
		"host".into(),
		"-smp".into(),
		"4".into(),
		"-m".into(),
		"2048".into(),
		"-machine".into(),
		"type=q35,accel=kvm".into(),
		"-device".into(),
		"virtio-vga-gl".into(),
		"-display".into(),
		"gtk,gl=on".into(),
		"-device".into(),
		"virtio-net-pci,netdev=net0".into(),
		"-netdev".into(),
		"user,id=net0".into(),
		"-drive".into(),
		format!("if=virtio,file={},format=qcow2", system_disk.display()),
		"-drive".into(),
		format!("if=virtio,file={},format=qcow2", user_disk.display()),
		"-drive".into(),
		format!(
			"if=pflash,format=raw,readonly=on,file={}",
			opts.ovmf_code_path.display()
		),
		"-drive".into(),
		format!(
			"if=pflash,format=raw,file={}",
			opts.ovmf_vars_path.display()
		),
		"-serial".into(),
		"stdio".into(),
		"-boot".into(),
		"d".into(),
	];

	args.extend(opts.extra_qemu_args.clone());

	let status = Command::new("qemu-system-x86_64")
		.args(&args)
		.stdin(Stdio::inherit())
		.stdout(Stdio::inherit())
		.stderr(Stdio::inherit())
		.status()
		.map_err(RunCommandError::Io)?;

	if !status.success() {
		return Err(RunCommandError::QemuNonZero(status));
	}

	Ok(())
}

/// Cria ou reseta o disco de user data (Btrfs com subvolumes)
pub fn reset_vm() -> Result<PathBuf, RunCommandError> {
	let user_disk = PathBuf::from("build/vm/user.qcow2");
	let size_gb = 10; // tamanho padrão

	println!("{}", " Resetting user data disk".blue().bold());

	fs::create_dir_all("build/vm").map_err(RunCommandError::Io)?;

	// Apagar o disco antigo se existir
	if user_disk.exists() {
		println!("{}", " Removing old user.qcow2".dimmed());
		fs::remove_file(&user_disk).map_err(RunCommandError::Io)?;
	}

	// Script de criação
	let setup_script = [
		"modprobe nbd max_part=8",
		&format!(
			"qemu-img create -f qcow2 {} {}G",
			user_disk.display(),
			size_gb
		),
		&format!("chmod 777 {}", user_disk.display()),
		"chmod 777 build/vm -R",
		&format!("qemu-nbd --disconnect /dev/nbd1"),
		&format!("qemu-nbd --connect /dev/nbd1 {}", user_disk.display()),
		"parted -s /dev/nbd1 mklabel gpt",
		"parted -s /dev/nbd1 mkpart USER btrfs 1MiB 100%",
		"sleep 0.1s",
		"mkfs.btrfs -f /dev/nbd1p1",
		"mkdir -p /mnt/ardos-user",
		"mount /dev/nbd1p1 /mnt/ardos-user",
		// subvolumes principais
		"btrfs subvolume create /mnt/ardos-user/@home",
		"btrfs subvolume create /mnt/ardos-user/@var",
		"btrfs subvolume create /mnt/ardos-user/@config",
		"btrfs subvolume create /mnt/ardos-user/@cache",
		"btrfs subvolume create /mnt/ardos-user/@data",
		// desmontar e limpar
		"umount /mnt/ardos-user",
		"qemu-nbd --disconnect /dev/nbd1",
	];

	run_privileged_script(&setup_script).map_err(RunCommandError::Io)?;

	println!(
		"{} {}",
		"✔ User data disk ready:".green().bold(),
		user_disk.display()
	);

	Ok(user_disk)
}
