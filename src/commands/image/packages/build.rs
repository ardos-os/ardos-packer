use std::{
	collections::HashSet,
	hash::{DefaultHasher, Hash, Hasher},
	path::{Path, PathBuf},
	process::Command,
	time::UNIX_EPOCH,
};

use colored::Colorize;
use thiserror::Error;

use crate::{
	fs_utils::has_file_newer_than,
	hash::hash_file,
	manifest::{DockerSettings, InvalidSourceError, Manifest, Package, Source},
	prefix_commands,
};
pub struct BuildResult {
	total_packages: usize,
	built_packages: usize,
	errors: usize,
}

impl BuildResult {
	pub fn print(&self) {
		if self.errors == self.total_packages && self.total_packages > 0 {
			println!(
				"{}: {}{} package{} failed to build",
				"ERROR".red().bold(),
				if self.errors != 1 { "All of the " } else { "" },
				self.errors,
				if self.errors != 1 { "s" } else { "" }
			);
		} else if self.errors > 0 {
			eprintln!(
				"{}: {} of the {} package{} failed to build",
				"ERROR".red().bold(),
				self.errors.to_string().blue(),
				self.total_packages.to_string().blue(),
				if self.total_packages != 1 { "s" } else { "" }
			);
		} else if self.built_packages == 0 && self.errors == 0 {
			println!(
				"{}",
				"󱌢 No packages to build: already up-to-date"
					.green()
					.bold()
					.dimmed()
			);
		} else {
			println!(
				"{} {} {} {}{}",
				"󱌢 All".green(),
				self.built_packages.to_string().cyan(),
				if self.built_packages != 1 {
					"packages"
				} else {
					"package"
				}
				.green(),
				if self.built_packages != 1 {
					"were built successfully!"
				} else {
					"was built successfully!"
				}
				.green(),
				if self.built_packages < self.total_packages {
					" (incremental build)"
				} else {
					""
				}
				.dimmed()
			);
		}
	}
	pub fn exit_if_failure(&self) {
		if self.errors > 0 {
			std::process::exit(1);
		}
	}
}

pub fn build(manifest: &Manifest) -> BuildResult {
	build_selected(manifest, None).expect("building all packages cannot select an unknown package")
}

#[derive(Debug, Error)]
pub enum PackageSelectionError {
	#[error("package `{0}` was not found in the manifest")]
	UnknownPackage(String),
	#[error("failed to clean package `{package}`: {source}")]
	Clean {
		package: String,
		#[source]
		source: std::io::Error,
	},
}

pub fn validate_package_selection(
	manifest: &Manifest,
	package_name: &str,
) -> Result<(), PackageSelectionError> {
	if manifest
		.packages
		.iter()
		.any(|package| package.name == package_name)
	{
		Ok(())
	} else {
		Err(PackageSelectionError::UnknownPackage(
			package_name.to_owned(),
		))
	}
}

pub(crate) fn selected_package_names(
	manifest: &Manifest,
	package_name: Option<&str>,
) -> Result<HashSet<String>, PackageSelectionError> {
	let Some(package_name) = package_name else {
		return Ok(
			manifest
				.packages
				.iter()
				.map(|package| package.name.clone())
				.collect(),
		);
	};
	validate_package_selection(manifest, package_name)?;

	let mut selected = HashSet::new();
	let mut pending = vec![package_name.to_owned()];
	while let Some(name) = pending.pop() {
		if !selected.insert(name.clone()) {
			continue;
		}
		let package = manifest
			.packages
			.iter()
			.find(|package| package.name == name)
			.ok_or_else(|| PackageSelectionError::UnknownPackage(name.clone()))?;
		pending.extend(package.build_deps.iter().cloned());
	}

	Ok(selected)
}

pub fn clean_package_build(
	manifest: &Manifest,
	package_name: &str,
) -> Result<(), PackageSelectionError> {
	let package = manifest
		.packages
		.iter()
		.find(|package| package.name == package_name)
		.ok_or_else(|| PackageSelectionError::UnknownPackage(package_name.to_owned()))?;
	let output_dir = package.get_out_dir();

	if output_dir.exists() {
		make_tree_writable(&output_dir).map_err(|source| PackageSelectionError::Clean {
			package: package_name.to_owned(),
			source,
		})?;
		std::fs::remove_dir_all(&output_dir).map_err(|source| PackageSelectionError::Clean {
			package: package_name.to_owned(),
			source,
		})?;
	}

	println!(
		"{} {}",
		"󰃢 Cleaned build output for".green().bold(),
		package_name.cyan().bold()
	);
	Ok(())
}

pub fn build_selected(
	manifest: &Manifest,
	package_name: Option<&str>,
) -> Result<BuildResult, PackageSelectionError> {
	println!();
	let selected = selected_package_names(manifest, package_name)?;
	let stale = stale_package_names(manifest);

	let packages = manifest
		.packages
		.iter()
		.filter(|package| selected.contains(&package.name))
		.filter(|package| stale.contains(&package.name))
		.cloned()
		.collect::<Vec<Package>>();

	if packages.is_empty() {
		return Ok(BuildResult {
			total_packages: selected.len(),
			built_packages: 0,
			errors: 0,
		});
	}
	println!(
		"{} {} {}",
		"󱌢  Compiling".green().bold(),
		packages.len().to_string().cyan(),
		if packages.len() == 1 {
			"package..."
		} else {
			"packages..."
		}
		.green()
		.bold()
	);

	let mut built_packages = 0;
	let mut errors = 0;
	for pkg in packages {
		println!(
			"    {} {} {}",
			"󱌢  Compiling".green().bold(),
			pkg.name,
			pkg.version.dimmed()
		);
		match pkg.build(&manifest) {
			Ok(()) => built_packages += 1,
			Err(error) => {
				errors += 1;
				println!(
					"\n    {} {}: {}\n",
					"  Error building package".red().bold(),
					pkg.name.cyan().bold().italic(),
					error.to_string().dimmed()
				);
			}
		}
	}
	Ok(BuildResult {
		total_packages: selected.len(),
		built_packages: built_packages,
		errors,
	})
}

#[cfg(unix)]
fn make_tree_writable(path: &Path) -> std::io::Result<()> {
	use std::os::unix::fs::PermissionsExt;

	let metadata = std::fs::symlink_metadata(path)?;
	if metadata.file_type().is_symlink() {
		return Ok(());
	}

	let mut mode = metadata.permissions().mode();
	if metadata.is_dir() {
		mode |= 0o700;
		std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
		for entry in std::fs::read_dir(path)? {
			make_tree_writable(&entry?.path())?;
		}
	} else {
		mode |= 0o600;
		std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
	}

	Ok(())
}

#[cfg(not(unix))]
fn make_tree_writable(_path: &Path) -> std::io::Result<()> {
	Ok(())
}

#[derive(Debug, Error)]
pub enum BuildError {
	#[error("io error: {0}")]
	Io(#[from] std::io::Error),
	#[error("process exited with non-zero code: {0}")]
	Non0ExitCode(i32),
	#[error("invalid source: {0}")]
	InvalidSource(#[from] InvalidSourceError),
	#[error("failed to unpack binary: {0}")]
	UnpackBinaryError(std::io::Error),
	#[error("failed to build docker image: {0}")]
	DockerError(#[from] BuildDockerImageError),
	#[error("no package found in out directory")]
	NoPackageFound,
	#[error("failed to inspect Cargo workspace {workspace}: {details}")]
	CargoWorkspaceMetadata { workspace: PathBuf, details: String },
}
#[derive(Debug, Error)]
pub enum BuildDockerImageError {
	#[error("io error: {0}")]
	Io(#[from] std::io::Error),
	#[error("process exited with non-zero code: {0}")]
	Non0ExitCode(i32),
	#[error("invalid dockerfile path")]
	InvalidDockerfilePath(PathBuf),
}
impl Package {
	pub fn get_out_dir(&self) -> PathBuf {
		// calculate hash of self using Hash trait
		let mut hasher = DefaultHasher::new();
		self.source.hash(&mut hasher);
		self.docker.hash(&mut hasher);
		if !self.cargo_workspaces.is_empty() {
			self.cargo_workspaces.hash(&mut hasher);
		}
		let hash = hasher.finish();
		[
			"build",
			"out",
			format!("{}-{}-{}", self.name, self.version, hash).as_str(),
		]
		.iter()
		.collect()
	}
	pub fn create_out_dir(&self) -> Result<PathBuf, std::io::Error> {
		let build_dir = self.get_out_dir();
		std::fs::create_dir_all(&build_dir)?;
		Ok(build_dir)
	}
	pub fn get_out_unpacked_dir(&self) -> PathBuf {
		let mut build_dir = self.get_out_dir();
		build_dir.push("unpacked");
		build_dir
	}
	pub fn create_out_unpacked_dir(&self) -> Result<PathBuf, std::io::Error> {
		let build_dir = self.get_out_unpacked_dir();
		std::fs::create_dir_all(&build_dir)?;
		Ok(build_dir)
	}
	pub fn get_this_package_src_root(&self) -> PathBuf {
		let pkg_build_root = if let Source::PkgBuildLocal { path, .. } = &self.source {
			path.clone()
		} else {
			self.get_package_prepared_dir()
		};
		pkg_build_root
	}
	pub fn get_built_archlinux_pkgs_paths(&self) -> std::io::Result<Vec<PathBuf>> {
		let build_dir = self.create_out_dir()?;
		macro_rules! files_listing {
			() => {
				std::fs::read_dir(&build_dir)?
					.filter_map(|entry| entry.ok())
					.filter(|entry| {
						entry
							.file_name()
							.to_string_lossy()
							.ends_with(".pkg.tar.zst")
					})
			};
		}

		let files_to_unpack: Vec<_> = match &self.source {
			Source::PkgBuildGit {
				pick_packages_from_group: Some(pkg_names),
				..
			}
			| Source::PkgBuildLocal {
				pick_packages_from_group: Some(pkg_names),
				..
			} => files_listing!()
				.filter(|entry| {
					let file_name = entry.file_name();
					let file_name = file_name.to_string_lossy();
					pkg_names.iter().any(|pkg| {
						let pattern_prefix = format!("{}-{}-", pkg, self.version);
						file_name.starts_with(&pattern_prefix)
					})
				})
				.map(|e| e.path())
				.collect::<Vec<PathBuf>>(),
			Source::Binary { .. } => self.source_tarball_path().into_iter().collect(),
			_ => files_listing!()
				.filter(|entry| {
					let file_name = entry.file_name();
					let file_name = file_name.to_string_lossy();
					let pattern_prefix = format!("{}-debug-{}-", self.name, self.version);
					!file_name.starts_with(&pattern_prefix)
				})
				.map(|e| e.path())
				.collect::<Vec<PathBuf>>(),
		};
		Ok(files_to_unpack)
	}

	pub fn get_deps_paths(&self, manifest: &Manifest) -> Vec<PathBuf> {
		let deps_paths = manifest
			.packages
			.iter()
			.filter(|p| self.build_deps.contains(&p.name))
			.flat_map(|p| {
				p.get_built_archlinux_pkgs_paths()
					.into_iter()
					.flatten()
					.chain(p.get_deps_paths(manifest).into_iter())
			})
			.collect::<Vec<_>>();
		deps_paths
	}

	pub fn build(&self, manifest: &Manifest) -> Result<(), BuildError> {
		let build_dir = self.create_out_dir()?;
		match &self.source {
			Source::Binary { .. } => {
				let unpacked_dir = self.create_out_unpacked_dir()?;
				let archlinux_pkg_path = self.source_tarball_path()?;
				// extract arch linux .pkg.tar.zst into the build_dir (streaming)
				let zstd = zstd::Decoder::new(std::fs::File::open(&archlinux_pkg_path)?)
					.map_err(BuildError::UnpackBinaryError)?;
				let mut tar = tar::Archive::new(zstd);

				println!(
					"    {} {}",
					"  Unpacking".green().bold(),
					archlinux_pkg_path
						.file_name()
						.unwrap()
						.display()
						.to_string()
						.italic()
				);
				tar
					.unpack(unpacked_dir)
					.map_err(BuildError::UnpackBinaryError)?;

				println!(
					"  {}  {} {}",
					" ".green(),
					archlinux_pkg_path
						.file_name()
						.unwrap()
						.display()
						.to_string()
						.italic(),
					"unpacked successfully".green().bold()
				);

				// Mark as successfully built so `needs_rebuild` can skip it next run.
				std::fs::write(
					build_dir.join("last_successful_build_time"),
					std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap()
						.as_millis()
						.to_string(),
				)?;
			}
			Source::PkgBuildGit { .. } | Source::PkgBuildLocal { .. } => {
				let docker_image_name = self.build_docker_image_if_needed()?;
				let pkg_src_root = self.get_this_package_src_root();
				let cargo_workspaces = if !self.cargo_workspaces.is_empty() {
					self.cargo_workspace_mounts(manifest, &pkg_src_root)?
				} else {
					vec![]
				};
				let mut command = Command::new("docker");
				let container_name =
					prefix_commands::unique_docker_container_name(&format!("pkg-{}", self.name));
				let deps_paths = self.get_deps_paths(&manifest);
				let build_script = include_str!("./build_script.sh");
				command
					.arg("run")
					.arg("--rm")
					.arg("--name")
					.arg(&container_name)
					.arg("-v")
					.arg(format!(
						"{}:/src:ro",
						pkg_src_root.canonicalize()?.display()
					))
					.arg("-v")
					.arg(format!("{}:/out", build_dir.canonicalize()?.display()));
				// map all dependencies to volumes inside /deps/
				for dep_path in deps_paths {
					command.arg("-v").arg(format!(
						"{}:/deps/{}:ro",
						dep_path.canonicalize()?.display(),
						dep_path.file_name().unwrap().to_string_lossy()
					));
				}
				let mut cargo_override_paths = Vec::new();
				for (index, workspace) in cargo_workspaces.iter().enumerate() {
					let container_root = format!("/cargo-workspaces/{index}");
					command.arg("-v").arg(format!(
						"{}:{container_root}:ro",
						workspace.root.canonicalize()?.display()
					));
					cargo_override_paths.extend(workspace.members.iter().flat_map(|member| {
						member.patch_sources.iter().map(|source| {
							format!(
								"{source}|{}={container_root}/{}",
								member.name,
								member.path.display()
							)
						})
					}));
				}
				cargo_override_paths.sort();
				cargo_override_paths.dedup();
				command
					.arg("-e")
					.arg("PKGDEST=/out")
					.arg("-e")
					.arg("BUILDDIR=/out/makepkg")
					.args(if cargo_override_paths.is_empty() {
						vec![]
					} else {
						vec![
							"-e".to_string(),
							format!("ARDOS_CARGO_OVERRIDES={}", cargo_override_paths.join("\n")),
						]
					})
					.arg(docker_image_name)
					.arg("bash")
					.arg("-c")
					.arg(build_script);
				prefix_commands::register_docker_container(&container_name);
				let exit_status = prefix_commands::run_command_with_tag(
					command,
					format!(
						"{}{}{}{}{}",
						"[".dimmed(),
						self.name.bold(),
						"@".dimmed(),
						self.version.dimmed(),
						" | makepkg] ".dimmed()
					),
				)
				.map_err(BuildError::Io)?;
				prefix_commands::unregister_docker_container(&container_name);
				if !exit_status.success() {
					return Err(BuildError::Non0ExitCode(exit_status.code().unwrap_or(-1)));
				}
				let files_to_unpack = self.get_built_archlinux_pkgs_paths()?;
				if files_to_unpack.is_empty() {
					return Err(BuildError::NoPackageFound);
				}

				let unpacked_dir = self.get_out_unpacked_dir();
				std::fs::remove_dir_all(&unpacked_dir).ok();
				std::fs::create_dir_all(&unpacked_dir)?;

				for path in files_to_unpack {
					println!(
						"    {} {}",
						"  Unpacking".yellow().bold(),
						path.file_name().unwrap().display().to_string().italic()
					);

					std::fs::remove_file(unpacked_dir.join(".BUILDINFO")).ok();
					std::fs::remove_file(unpacked_dir.join(".MTREE")).ok();
					std::fs::remove_file(unpacked_dir.join(".PKGINFO")).ok();
					let file = std::fs::File::open(&path).map_err(BuildError::UnpackBinaryError)?;
					let zstd = zstd::Decoder::new(file).map_err(BuildError::UnpackBinaryError)?;
					let mut tar = tar::Archive::new(zstd);
					tar
						.unpack(&unpacked_dir)
						.map_err(BuildError::UnpackBinaryError)?;
					println!(
						"  {}  {} {}",
						" ".green().bold(),
						path.file_name().unwrap().display().to_string().italic(),
						"unpacked successfully".green().bold()
					);
				}

				// save the current time in a "last_successful_build_time" file
				std::fs::write(
					build_dir.join("last_successful_build_time"),
					std::time::SystemTime::now()
						.duration_since(std::time::UNIX_EPOCH)
						.unwrap()
						.as_millis()
						.to_string(),
				)?;
			}
		}
		Ok(())
	}

	fn inputs_changed(&self, manifest: &Manifest) -> bool {
		let build_dir = self.get_out_dir();
		let last_successful_build_time_path = build_dir.join("last_successful_build_time");

		if !last_successful_build_time_path.exists() {
			return true;
		}

		let Some(last_successful_build_time) =
			std::fs::read_to_string(&last_successful_build_time_path)
				.ok()
				.and_then(|s| s.parse::<u128>().ok())
		else {
			return true;
		};

		let timestamp =
			UNIX_EPOCH + std::time::Duration::from_millis(last_successful_build_time as u64);

		let source_path = match &self.source {
			Source::PkgBuildLocal { path, .. } => manifest.manifest_dir.join(path),
			Source::PkgBuildGit { .. } | Source::Binary { .. } => self
				.source_tarball_path()
				.ok()
				.unwrap_or_else(|| self.get_package_prepared_dir()),
		};

		let needs_rebuild = has_file_newer_than(&source_path, timestamp)
			.map_err(|e| dbg!(e))
			.unwrap_or(true);

		needs_rebuild
			|| self.cargo_workspaces.iter().any(|workspace| {
				has_file_newer_than(&manifest.manifest_dir.join(workspace), timestamp)
					.map_err(|e| dbg!(e))
					.unwrap_or(true)
			})
	}

	fn cargo_workspace_mounts(
		&self,
		manifest: &Manifest,
		consumer_root: &Path,
	) -> Result<Vec<CargoWorkspaceMount>, BuildError> {
		let dependency_sources = cargo_dependency_sources(consumer_root).map_err(|details| {
			BuildError::CargoWorkspaceMetadata {
				workspace: consumer_root.to_path_buf(),
				details,
			}
		})?;
		self
			.cargo_workspaces
			.iter()
			.map(|workspace| {
				let root = manifest.manifest_dir.join(workspace).canonicalize()?;
				let (metadata, metadata_root) =
					cargo_metadata(&root).map_err(|details| BuildError::CargoWorkspaceMetadata {
						workspace: workspace.clone(),
						details,
					})?;
				let packages =
					metadata["packages"]
						.as_array()
						.ok_or_else(|| BuildError::CargoWorkspaceMetadata {
							workspace: workspace.clone(),
							details: "Cargo metadata did not contain a packages array".to_string(),
						})?;
				let mut members = packages
					.iter()
					.filter_map(|package| {
						Some((
							package["name"].as_str()?.to_string(),
							Path::new(package["manifest_path"].as_str()?).parent()?,
						))
					})
					.map(|(name, path)| {
						path
							.strip_prefix(&metadata_root)
							.map(Path::to_path_buf)
							.map(|path| CargoWorkspaceMember {
								patch_sources: dependency_sources.get(&name).cloned().unwrap_or_default(),
								name,
								path,
							})
							.map_err(|_| BuildError::CargoWorkspaceMetadata {
								workspace: workspace.clone(),
								details: format!(
									"workspace member {} is outside {}",
									path.display(),
									root.display()
								),
							})
					})
					.collect::<Result<Vec<_>, _>>()?;
				members.sort_by(|a, b| a.name.cmp(&b.name));
				members.dedup_by(|a, b| a.name == b.name);
				Ok(CargoWorkspaceMount { root, members })
			})
			.collect()
	}
	pub fn get_docker_image_name(&self) -> Result<String, BuildDockerImageError> {
		Ok(
			match &self.docker {
				DockerSettings::DockerfilePath {
					path: dockerfile_path,
				} => {
					format!("ardos-packer-{}", hash_file(dockerfile_path)?)
				}
				DockerSettings::ImageName { name } => name.clone(),
			}
			.to_lowercase(),
		)
	}
	pub fn build_docker_image_if_needed(&self) -> Result<String, BuildDockerImageError> {
		match &self.docker {
			DockerSettings::DockerfilePath {
				path: dockerfile_path,
			} => {
				let docker_image_name = self.get_docker_image_name()?;
				let dockerfile_folder = dockerfile_path
					.parent()
					.ok_or_else(|| BuildDockerImageError::InvalidDockerfilePath(dockerfile_path.clone()))?;
				let mut command = Command::new("docker");
				command.args([
					"build",
					"-t",
					&docker_image_name,
					"-f",
					dockerfile_path
						.to_str()
						.ok_or_else(|| BuildDockerImageError::InvalidDockerfilePath(dockerfile_path.clone()))?,
					dockerfile_folder
						.to_str()
						.ok_or_else(|| BuildDockerImageError::InvalidDockerfilePath(dockerfile_path.clone()))?,
				]);
				let output = prefix_commands::run_command_with_tag(
					command,
					format!(
						"{}{}{}{}{}",
						"[".dimmed(),
						self.name.bold(),
						"@".dimmed(),
						self.version.dimmed(),
						" | Dockerfile] ".dimmed()
					),
				)
				.map_err(BuildDockerImageError::Io)?;
				if output.success() {
					Ok(docker_image_name)
				} else {
					Err(BuildDockerImageError::Non0ExitCode(
						output.code().unwrap_or(-1),
					))
				}
			}
			DockerSettings::ImageName { name } => Ok(name.clone()),
		}
	}
}

struct CargoWorkspaceMount {
	root: PathBuf,
	members: Vec<CargoWorkspaceMember>,
}

struct CargoWorkspaceMember {
	name: String,
	path: PathBuf,
	patch_sources: Vec<String>,
}

fn cargo_dependency_sources(
	root: &Path,
) -> Result<std::collections::HashMap<String, Vec<String>>, String> {
	let (metadata, _) = cargo_metadata(root)?;
	let mut sources = std::collections::HashMap::<String, Vec<String>>::new();
	for dependency in metadata["packages"]
		.as_array()
		.into_iter()
		.flatten()
		.filter_map(|package| package["dependencies"].as_array())
		.flatten()
	{
		let Some(name) = dependency["name"].as_str() else {
			continue;
		};
		let Some(source) = dependency["source"]
			.as_str()
			.and_then(normalize_cargo_source)
		else {
			continue;
		};
		let entry = sources.entry(name.to_string()).or_default();
		if !entry.contains(&source) {
			entry.push(source);
		}
	}
	for values in sources.values_mut() {
		values.sort();
	}
	Ok(sources)
}

fn cargo_metadata(root: &Path) -> Result<(serde_json::Value, PathBuf), String> {
	let metadata_home = std::env::temp_dir().join(format!(
		"ardos-packer-cargo-metadata-{}-{}",
		std::process::id(),
		std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.map_err(|error| error.to_string())?
			.as_nanos()
	));
	let metadata_root = metadata_home.join("source");
	copy_cargo_metadata_source(root, &metadata_root)?;
	let output = Command::new("cargo")
		.current_dir(&metadata_home)
		.env("CARGO_HOME", metadata_home.join("cargo-home"))
		.args([
			"metadata",
			"--format-version",
			"1",
			"--no-deps",
			"--manifest-path",
		])
		.arg(metadata_root.join("Cargo.toml"))
		.output()
		.map_err(|error| error.to_string());
	let output = output?;
	if !output.status.success() {
		std::fs::remove_dir_all(&metadata_home).ok();
		return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
	}

	let metadata: serde_json::Value =
		serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
	std::fs::remove_dir_all(&metadata_home).ok();
	Ok((metadata, metadata_root))
}

fn copy_cargo_metadata_source(source: &Path, destination: &Path) -> Result<(), String> {
	std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
	for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
		let entry = entry.map_err(|error| error.to_string())?;
		let file_type = entry.file_type().map_err(|error| error.to_string())?;
		let name = entry.file_name();
		if matches!(
			name.to_str(),
			Some(".git" | ".cargo" | "target" | "Cargo.lock")
		) {
			continue;
		}
		let target = destination.join(&name);
		if file_type.is_dir() {
			copy_cargo_metadata_source(&entry.path(), &target)?;
		} else if file_type.is_file() {
			std::fs::copy(entry.path(), target).map_err(|error| error.to_string())?;
		}
	}
	Ok(())
}

fn normalize_cargo_source(source: &str) -> Option<String> {
	if source.starts_with("registry+") {
		return Some("crates-io".to_string());
	}
	let git = source.strip_prefix("git+")?;
	let end = git.find(['?', '#']).unwrap_or(git.len());
	Some(git[..end].trim_end_matches('/').to_string())
}

fn stale_package_names(manifest: &Manifest) -> HashSet<String> {
	let directly_stale = manifest
		.packages
		.iter()
		.filter(|package| package.inputs_changed(manifest))
		.map(|package| package.name.clone())
		.collect();
	propagate_stale_packages(&manifest.packages, directly_stale)
}

fn propagate_stale_packages(packages: &[Package], mut stale: HashSet<String>) -> HashSet<String> {
	loop {
		let newly_stale = packages
			.iter()
			.filter(|package| !stale.contains(&package.name))
			.filter(|package| {
				package
					.build_deps
					.iter()
					.any(|dependency| stale.contains(dependency))
			})
			.map(|package| package.name.clone())
			.collect::<Vec<_>>();
		if newly_stale.is_empty() {
			return stale;
		}
		stale.extend(newly_stale);
	}
}

#[cfg(test)]
mod tests {
	use super::{cargo_dependency_sources, normalize_cargo_source, propagate_stale_packages};
	use crate::manifest::{DockerSettings, Hooks, InitrdOptions, Kernel, Manifest, Package, Source};
	use std::collections::HashSet;
	use std::path::PathBuf;

	fn package(name: &str, build_deps: &[&str]) -> Package {
		Package {
			name: name.to_string(),
			version: "1".to_string(),
			author: None,
			source: Source::PkgBuildLocal {
				path: name.into(),
				pick_packages_from_group: None,
			},
			docker: DockerSettings::default(),
			build_deps: build_deps
				.iter()
				.map(|dependency| dependency.to_string())
				.collect(),
			cargo_workspaces: Vec::new(),
		}
	}

	fn repository_root() -> PathBuf {
		std::env::var_os("ARDOS_REPOSITORY_ROOT")
			.map(PathBuf::from)
			.unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
			.canonicalize()
			.unwrap()
	}

	#[test]
	fn stale_state_propagates_to_transitive_dependents() {
		let packages = vec![
			package("library", &[]),
			package("consumer", &["library"]),
			package("image-service", &["consumer"]),
			package("unrelated", &[]),
		];
		let stale = propagate_stale_packages(&packages, HashSet::from(["library".to_string()]));

		assert_eq!(
			stale,
			HashSet::from([
				"library".to_string(),
				"consumer".to_string(),
				"image-service".to_string(),
			])
		);
	}

	#[test]
	fn dependency_cycles_do_not_recurse_forever() {
		let packages = vec![package("a", &["b"]), package("b", &["a"])];
		let stale = propagate_stale_packages(&packages, HashSet::from(["a".to_string()]));

		assert_eq!(stale, HashSet::from(["a".to_string(), "b".to_string()]));
	}

	#[test]
	fn discovers_crates_from_a_local_cargo_workspace() {
		let mut consumer = package("consumer", &[]);
		consumer.cargo_workspaces = vec!["packages/shift".into()];
		let repository_root = repository_root();
		let manifest = Manifest {
			version: "test".to_string(),
			kernel: Kernel {
				url: String::new(),
				options: Default::default(),
			},
			initrd: InitrdOptions {
				build_script: PathBuf::new(),
			},
			hooks: Hooks::default(),
			packages: vec![consumer.clone()],
			manifest_dir: repository_root.clone(),
		};

		let workspaces = consumer
			.cargo_workspace_mounts(&manifest, &repository_root.join("packages/tibs"))
			.unwrap();
		let member_names = workspaces[0]
			.members
			.iter()
			.map(|member| member.name.as_str())
			.collect::<HashSet<_>>();

		assert!(member_names.contains("tab-app-framework"));
		assert!(member_names.contains("tab-client"));
		assert!(!member_names.contains("tibs"));
		let app_framework = workspaces[0]
			.members
			.iter()
			.find(|member| member.name == "tab-app-framework")
			.unwrap();
		assert_eq!(
			app_framework.patch_sources,
			vec!["https://github.com/ardos-os/shift"]
		);
	}

	#[test]
	fn discovers_git_dependency_sources() {
		let repository_root = repository_root();
		let sources = cargo_dependency_sources(&repository_root.join("packages/shift")).unwrap();

		assert_eq!(
			sources["easydrm"],
			vec!["https://github.com/ardos-os/easydrm"]
		);
	}

	#[test]
	fn overrides_shift_git_dependency_with_local_easydrm() {
		let mut shift = package("shift", &[]);
		shift.cargo_workspaces = vec!["packages/easydrm".into()];
		let repository_root = repository_root();
		let manifest = Manifest {
			version: "test".to_string(),
			kernel: Kernel {
				url: String::new(),
				options: Default::default(),
			},
			initrd: InitrdOptions {
				build_script: PathBuf::new(),
			},
			hooks: Hooks::default(),
			packages: vec![shift.clone()],
			manifest_dir: repository_root.clone(),
		};

		let workspaces = shift
			.cargo_workspace_mounts(&manifest, &repository_root.join("packages/shift"))
			.unwrap();
		assert_eq!(workspaces[0].members[0].name, "easydrm");
		assert_eq!(
			workspaces[0].members[0].patch_sources,
			vec!["https://github.com/ardos-os/easydrm"]
		);
	}

	#[test]
	fn normalizes_cargo_git_sources_for_patch_tables() {
		assert_eq!(
			normalize_cargo_source("git+https://github.com/ardos-os/easydrm?branch=main#0123456789"),
			Some("https://github.com/ardos-os/easydrm".to_string())
		);
	}
}
