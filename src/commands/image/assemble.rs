use std::{
	io::{Write, stdout},
	path::PathBuf,
	process::Command,
};

use colored::Colorize;
use thiserror::Error;

use crate::{
	credits, fs_utils,
	manifest::{Manifest, Package},
	prefix_commands,
	privilage_escalation::ensure_root,
};
fn get_git_commit_hash() -> Option<String> {
	let output = Command::new("git")
		.args(["rev-parse", "--short", "HEAD"])
		.output()
		.ok()?; // falha ao executar o comando → None

	if !output.status.success() {
		return None; // git retornou erro (ex: não é repositório)
	}

	let hash = String::from_utf8(output.stdout).ok()?; // converte bytes em string
	Some(hash.trim().to_string()) // remove \n e espaços
}
#[derive(Debug, Error)]
pub enum SquashFsError {
	#[error("Non-zero exit code: {exit_code}")]
	Non0ExitCode { exit_code: i32 },
	#[error("Command error: io error: {0}")]
	CommandError(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum AssembleError<'m> {
	#[error("Failed to copy package {} to sysroot: {error}", package.name)]
	CopyError {
		package: &'m Package,
		error: std::io::Error,
	},
	#[error("Failed to create squashfs image: {0}")]
	SquashfsError(#[from] SquashFsError),
	#[error("io error: {0}")]
	Io(#[from] std::io::Error),
}

pub fn assemble<'m>(manifest: &'m Manifest) -> Result<PathBuf, AssembleError<'m>> {
	let sysroot_folder = PathBuf::from("build/sysroot");
	std::fs::remove_dir_all(&sysroot_folder).ok();
	let image_file_name = format!(
		"ardos-{}-{}.squashfs",
		manifest.version,
		get_git_commit_hash().unwrap_or(String::from("unknown"))
	);

	for pkg in manifest.packages.iter() {
		let unpacked_path = pkg.get_out_unpacked_dir();
		print!(
			"    {} {} {}\r",
			"󱁥  Copying".yellow().bold(),
			pkg.name,
			pkg.version.dimmed()
		);
		stdout().flush().ok();
		fs_utils::copy_dir_all_with_filter(unpacked_path, &sysroot_folder, |d| {
			d.file_name()
				.into_string()
				.is_ok_and(|n| !n.starts_with("."))
		})
		.map_err(|e| AssembleError::CopyError {
			package: pkg,
			error: e,
		})?;
		println!(
			"  {} 󱁥  {} {} {}",
			"  ".blue(),
			pkg.name.bold(),
			format!("({})", pkg.version).dimmed().italic(),
			"copied successfully".green()
		);
	}

	let credits = credits::generate_credits(&manifest.packages);
	let credits_json = serde_json::to_string_pretty(&credits).unwrap();
	let credits_file = sysroot_folder.join("etc/credits.json");
	std::fs::create_dir_all(sysroot_folder.join("etc"))?;
	std::fs::write(&credits_file, credits_json)?;
	let images_path = PathBuf::from("build/images");
	std::fs::create_dir_all(images_path)?;
	let image_path = PathBuf::from("build/images").join(&image_file_name);
	println!(
		"     {} {}",
		"→󰋩← Creating image".yellow().bold(),
		image_file_name
	);
	let mut command = Command::new("mksquashfs");
	command
		.arg(&sysroot_folder)
		.arg(&image_path)
		.args(["-comp", "zstd", "-b", "1M", "-noappend"]);
	let status =
		prefix_commands::run_command_with_tag(command, "       [ →󰋩← mksquashfs ] ".blue().to_string())
			.map_err(SquashFsError::CommandError)?;
	if !status.success() {
		return Err(AssembleError::SquashfsError(SquashFsError::Non0ExitCode {
			exit_code: status.code().unwrap_or(-1),
		}));
	}
	// rodar comando do squashfs aqui
	Ok(image_path)
}
