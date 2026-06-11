use crate::sources::SourceType;
use crate::{
	commands::image::packages::build::{PackageSelectionError, selected_package_names},
	manifest::{Manifest, Package},
};

use colored::*;
use std::sync::Arc;
use std::sync::mpsc::channel;
pub struct FetchResult {
	pub downloaded_packages: usize,
	pub errors: usize,
	pub total_packages: usize,
}
impl FetchResult {
	pub fn print(&self) {
		if self.errors == self.total_packages && self.total_packages > 0 {
			println!(
				"{}: {}{} package{} failed to fetch",
				"ERROR".red().bold(),
				if self.errors != 1 { "All of the " } else { "" },
				self.errors,
				if self.errors != 1 { "s" } else { "" }
			);
		} else if self.errors > 0 {
			eprintln!(
				"{}: {} of the {} package{} failed to fetch",
				"ERROR".red().bold(),
				self.errors.to_string().blue(),
				self.total_packages.to_string().blue(),
				if self.total_packages != 1 { "s" } else { "" }
			);
		} else if self.downloaded_packages == 0 && self.errors == 0 {
			println!(
				"{}",
				"󰇚 No packages to fetch: already up-to-date"
					.green()
					.bold()
					.dimmed()
			);
		} else {
			println!(
				"{} {} {}{}{}",
				"󰇚 All".green(),
				self.downloaded_packages.to_string().cyan(),
				if self.downloaded_packages != 1 {
					"packages"
				} else {
					"package"
				}
				.green(),
				" fetched successfully!".green(),
				if self.downloaded_packages < self.total_packages {
					" (incremental download)"
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
pub fn fetch(manifest: &Manifest) -> FetchResult {
	fetch_selected(manifest, None).expect("fetching all packages cannot select an unknown package")
}

pub fn fetch_selected(
	manifest: &Manifest,
	package_name: Option<&str>,
) -> Result<FetchResult, PackageSelectionError> {
	Package::create_sources_dir().unwrap();
	let selected = selected_package_names(manifest, package_name)?;

	const CONCURRENCY_LIMIT: usize = 4;

	let packages = Arc::new(
		manifest
			.packages
			.iter()
			.filter(|package| selected.contains(&package.name))
			.filter(|p| !matches!(p.source_type(), Ok(SourceType::LocalFolder { .. })))
			.filter(|p| p.assert_source_tarball_matches_hash().is_err())
			.cloned()
			.collect::<Vec<Package>>(),
	);
	if packages.is_empty() {
		return Ok(FetchResult {
			downloaded_packages: 0,
			errors: 0,
			total_packages: selected.len(),
		});
	}
	println!(
		"{} {} {}",
		"󰇚 Fetching sources for".green().bold(),
		packages.len().to_string().cyan(),
		"packages...".green().bold()
	);
	let (tx, rx) = channel();

	// Use threadpool crate
	let pool = threadpool::ThreadPool::new(CONCURRENCY_LIMIT);

	for pkg in packages.iter().cloned() {
		let tx = tx.clone();
		pool.execute(move || {
			let fetch_res = pkg.fetch_sources();
			let prep_res = fetch_res.and_then(|_| pkg.prepare_sources());
			tx.send((pkg.name.clone(), prep_res)).unwrap();
		});
	}

	drop(tx);
	let mut downloaded_packages = 0;
	let mut errors = 0;
	for (name, result) in rx.iter().take(packages.len()) {
		match result {
			Ok(path) => {
				println!(
					"    {} '{}' {} {:?}",
					"󰇚".green().bold(),
					name.cyan().bold(),
					"fetched to".green(),
					path
				);
				downloaded_packages += 1;
			}
			Err(crate::manifest::SourceFetchError::HashMismatch { expected, actual }) => {
				eprintln!(
					"{} for package '{}':\n\n      {}: {}\n      {}:   {}\n\n      {}",
					"     Hash mismatch".red().bold(),
					name.yellow().bold(),
					"Expected".white(),
					expected.as_str().blue(),
					"Actual".white(),
					actual.as_str().white(),
					"(The file on the remote server may be corrupted, tampered with, or the URL may be incorrect.)".red()
				);
				eprintln!(
								"\n{} {}\n      {}",
								"      help:".cyan().bold(),
								"If you recently updated the manifest, make sure the 'sha256'\n            field matches the actual file. ".white(),
								"      You may need to update the hash or check the source URL.\n".white()
				);
				errors += 1;
			}
			Err(e) => {
				eprintln!(
					"{} '{}': {}",
					"      Error fetching package".red().bold(),
					name.yellow().bold(),
					format!("{}", e).red()
				);
				errors += 1;
			}
		}
	}

	Ok(FetchResult {
		downloaded_packages,
		errors,
		total_packages: selected.len(),
	})
}
