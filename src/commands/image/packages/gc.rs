use std::{
	fs::read_dir,
	path::{Path, PathBuf},
};

use colored::Colorize;

use crate::{
	manifest::{GarbageCollectionStat, Manifest, Package},
	size,
};

#[cfg(unix)]
fn make_tree_writable(path: &Path) -> std::io::Result<()> {
	use std::os::unix::fs::PermissionsExt;

	let meta = std::fs::symlink_metadata(path)?;
	let ft = meta.file_type();
	if ft.is_symlink() {
		return Ok(());
	}

	// Ensure we can traverse and delete as the current user.
	let mut mode = meta.permissions().mode();
	if ft.is_dir() {
		mode |= 0o700;
		std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
		for entry in std::fs::read_dir(path)? {
			let entry = entry?;
			make_tree_writable(&entry.path())?;
		}
	} else if ft.is_file() {
		mode |= 0o600;
		std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
	}

	Ok(())
}

#[cfg(not(unix))]
fn make_tree_writable(_path: &Path) -> std::io::Result<()> {
	Ok(())
}

pub fn calculate_folder_size<P>(path: P) -> std::io::Result<u64>
where
	P: AsRef<Path>,
{
	let mut result = 0;

	if path.as_ref().is_dir() {
		for entry in read_dir(&path)? {
			let _path = entry?.path();
			if _path.is_file() {
				result += _path.metadata()?.len();
			} else {
				result += calculate_folder_size(_path)?;
			}
		}
	} else {
		result = path.as_ref().metadata()?.len();
	}
	Ok(result)
}
pub fn gc_command(manifest: &Manifest) {
	match manifest.garbage_collect_sources() {
		Err(e) => {
			eprintln!(
				"{}: Failed to run garbage collector: {}",
				"ERROR".red().bold(),
				e.to_string().white()
			);
		}
		Ok(GarbageCollectionStat {
			freed_bytes,
			removed_out_folders,
			removed_prepared_packages,
			removed_sources_packages,
		}) => {
			if removed_out_folders == 0 && removed_prepared_packages == 0 && removed_sources_packages == 0
			{
				println!(
					"{}",
					"No packages removed during garbage collection.".dimmed()
				);
				return;
			}
			let package_counter = |n: usize| format!(" {} package{}", n, if n == 1 { "" } else { "s" });
			println!(
				"{} {} {}",
				"🧹 Garbage Collector:",
				"Freed".green(),
				size::human_readable_size(freed_bytes).to_string().cyan()
			);
			println!();
			println!(
				"    {} {}",
				package_counter(removed_out_folders).bold(),
				"output folders removed".green()
			);
			println!(
				"    {} {}",
				package_counter(removed_prepared_packages).bold(),
				"prepared packages removed".green()
			);
			println!(
				"    {} {}",
				package_counter(removed_sources_packages).bold(),
				"source packages removed".green()
			);
			println!();
		}
	}
}

impl Manifest {
	pub fn garbage_collect_sources(&self) -> std::io::Result<GarbageCollectionStat> {
		let sources_dir = PathBuf::from(Package::sources_path());
		if !sources_dir.exists() {
			return Ok(GarbageCollectionStat {
				freed_bytes: 0,
				removed_prepared_packages: 0,
				removed_sources_packages: 0,
				removed_out_folders: 0,
			});
		}
		let mut referenced = std::collections::HashSet::new();
		for pkg in &self.packages {
			if let Ok(path) = pkg.source_tarball_path() {
				referenced.insert(path);
			}
		}
		let mut prepared_referenced = std::collections::HashSet::new();
		let prepared_dir = Package::prepared_sources_dir();
		for pkg in &self.packages {
			let mut d = prepared_dir.clone();
			d.push(format!("{}-{}", pkg.name, pkg.version));
			prepared_referenced.insert(d);
		}

		let mut freed_bytes = 0u64;
		let mut removed_sources_packages = 0usize;
		for entry in std::fs::read_dir(&sources_dir)
			.into_iter()
			.flatten()
			.flatten()
		{
			let path = entry.path();
			let metadata = entry.metadata()?;
			if entry.file_name() == "prepared" && metadata.is_dir() {
				// Handle prepared directory separately below
				continue;
			}
			if !referenced.contains(&path) {
				// Try to remove file, but ignore errors and continue
				match if path.is_file() {
					std::fs::remove_file(&path)
				} else {
					make_tree_writable(&path).ok();
					std::fs::remove_dir_all(&path)
				} {
					Ok(()) => {
						freed_bytes += metadata.len();
						removed_sources_packages += 1;
					}
					Err(e) => {
						eprintln!(
							"{}: {} {:?}: {}",
							"ERROR".red().bold(),
							"Failed to remove file".white(),
							path.display().to_string().bright_black(),
							e.to_string().bright_black()
						);
					}
				}
			}
		}
		let mut removed_prepared_packages = 0usize;
		// Garbage collect prepared folder
		if prepared_dir.exists() {
			for entry in std::fs::read_dir(&prepared_dir)
				.into_iter()
				.flatten()
				.flatten()
			{
				let path = entry.path();
				if !prepared_referenced.contains(&path) {
					let metadata = entry.metadata()?;
					make_tree_writable(&path).ok();
					match std::fs::remove_dir_all(&path) {
						Ok(()) => {
							freed_bytes += metadata.len();
							removed_prepared_packages += 1;
						}
						Err(e) => {
							eprintln!(
								"{}: {} {:?}: {}",
								"ERROR".red().bold(),
								"Failed to remove prepared directory".white(),
								path.display().to_string().bright_black(),
								e.to_string().bright_black()
							);
						}
					}
				}
			}
		}
		let mut removed_out_folders = 0usize;
		let referenced_folders = self
			.packages
			.iter()
			.filter_map(|p| {
				p.get_out_dir()
					.file_name()
					.map(|n| n.to_string_lossy().to_string())
			})
			.collect::<Vec<_>>();
		for entry in std::fs::read_dir("build/out")
			.into_iter()
			.flatten()
			.flatten()
		{
			let path = entry.path();
			if !referenced_folders.contains(&path.file_name().unwrap().to_string_lossy().to_string()) {
				let size_bytes = calculate_folder_size(&path).unwrap_or_default();
				make_tree_writable(&path).ok();
				match std::fs::remove_dir_all(&path) {
					Ok(()) => {
						freed_bytes += size_bytes;
						removed_out_folders += 1;
					}
					Err(e) => {
						eprintln!(
							"{}: {} {:?}: {}",
							"ERROR".red().bold(),
							"Failed to remove output folder".white(),
							path.display().to_string().bright_black(),
							e.to_string().bright_black()
						);
					}
				}
			}
		}
		Ok(GarbageCollectionStat {
			freed_bytes,
			removed_out_folders,
			removed_prepared_packages,
			removed_sources_packages,
		})
	}
}
