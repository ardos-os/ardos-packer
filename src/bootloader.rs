use colored::*;
use std::path::PathBuf;

const LIMINE_BOOTLOADER_DOWNLOAD_URL: &str =
	"https://github.com/limine-bootloader/Limine/archive/refs/tags/v10.2.1-binary.tar.gz";
const LIMINE_BOOTLOADER_TARBALL_HASH: &str =
	"F5039B62E2BA7138CF1DC91FE715F5FE03FC4503EBA49441E71894DD7BCBB6F9";
const BOOTLOADER_TARBALL_PATH: &str = "build/bootloader/limine.tar.gz";
const BOOTLOADER_UNPACKED_DIR: &str = "build/bootloader/unpacked/";
const BOOTLOADER_EFI_FILE_HASH: &str =
	"771FFD71164D9441BCCF20C8302F7B7D4A6714024437BD58B74B20EB6A8C524E";
#[derive(Debug, thiserror::Error)]
pub enum BootloaderDownloadError {
	#[error("an io error ocurred: {0}")]
	IOError(#[from] std::io::Error),
	#[error("failed to download: {0}")]
	DownloadError(#[from] ureq::Error),
	#[error("hash mismatch: expected {expected}, got {actual}")]
	HashMismatch { expected: String, actual: String },
}

/// Print a pretty result for the bootloader download operation.
/// This is intentionally separate from download_bootloader() — the
/// download function will print progress only; the caller can choose
/// whether and when to call this to print the final result.
pub fn print_bootloader_download_result(res: &Result<PathBuf, BootloaderDownloadError>) {
	match res {
		Ok(path) => {
			println!(
				"{} {} {}",
				"󰇚".green().bold(),
				"Bootloader".cyan().bold(),
				format!("fetched to {}", path.display()).green()
			);
		}
		Err(BootloaderDownloadError::HashMismatch { expected, actual }) => {
			eprintln!(
				"{}:\n\n      {}: {}\n      {}:   {}\n\n      {}",
				"     Hash mismatch".red().bold(),
				"Expected".white(),
				expected.as_str().blue(),
				"Actual".white(),
				actual.as_str().white(),
				"(The file on the remote server may be corrupted, tampered with, or the URL may be incorrect.)".red()
			);
		}
		Err(e) => {
			eprintln!(
				"{} {}: {}",
				"    ".red().bold(),
				"Error fetching bootloader".red().bold(),
				format!("{}", e).red()
			);
		}
	}
}

pub fn download_bootloader() -> Result<PathBuf, BootloaderDownloadError> {
	let tarball_path = PathBuf::from(BOOTLOADER_TARBALL_PATH);
	let unpack_dir = PathBuf::from(BOOTLOADER_UNPACKED_DIR);
	std::fs::create_dir_all(BOOTLOADER_UNPACKED_DIR)?;
	let bootx_path = unpack_dir.join("Limine-10.2.1-binary").join("BOOTX64.EFI");

	// Start progress output
	println!(
		"{} {} {}",
		"󰇚".green().bold(),
		"Fetching bootloader".green().bold(),
		"...".green().bold()
	);

	// If we've already unpacked the bootloader, return it early.
	if bootx_path.exists()
		&& crate::hash::hash_file(&bootx_path)?.as_str() == BOOTLOADER_EFI_FILE_HASH
	{
		println!(
			"    {} {} {}",
			"󰇚".green().bold(),
			"Using cached bootloader at".green(),
			format!("{}", bootx_path.display()).cyan()
		);
		return Ok(bootx_path);
	}

	// Ensure unpack dir is clean for a fresh attempt.
	if std::path::Path::new(BOOTLOADER_UNPACKED_DIR).exists() {
		println!(
			"    {} {}",
			"󰇚".green().bold(),
			"Cleaning previous unpacked directory".green()
		);
		std::fs::remove_dir_all(BOOTLOADER_UNPACKED_DIR)?;
	}

	// Check whether we need to download the tarball.
	let mut need_download = true;
	if tarball_path.exists() {
		println!(
			"    {} {}",
			"󰇚".green().bold(),
			"Found existing tarball, verifying hash...".green()
		);
		let hash = crate::hash::hash_file(tarball_path.clone())?;
		let actual = hash.to_string();
		if actual == LIMINE_BOOTLOADER_TARBALL_HASH {
			need_download = false;
			println!(
				"    {} {}",
				"󰇚".green().bold(),
				"Tarball hash matches; using cached tarball".green()
			);
		} else {
			// Remove corrupt/mismatched tarball so we re-download.
			println!(
				"    {} {}",
				"󰇚".yellow().bold(),
				"Tarball hash mismatch; removing and re-downloading".yellow()
			);
			std::fs::remove_file(&tarball_path)?;
		}
	}

	if need_download {
		// Ensure parent directory exists.
		if let Some(parent) = tarball_path.parent() {
			std::fs::create_dir_all(parent)?;
		}

		// Download with ureq.
		println!(
			"    {} {}",
			"󰇚".green().bold(),
			"Downloading bootloader tarball...".green()
		);
		let resp = ureq::get(LIMINE_BOOTLOADER_DOWNLOAD_URL).call()?;
		let mut reader = resp.into_body().into_reader();

		let mut out = std::fs::File::create(&tarball_path)?;
		std::io::copy(&mut reader, &mut out)?;
		println!(
			"    {} {} {}",
			"󰇚".green().bold(),
			"Downloaded tarball to".green(),
			format!("{}", tarball_path.display()).cyan()
		);
	}

	// Verify downloaded tarball hash.
	println!(
		"    {} {}",
		"󰇚".green().bold(),
		"Verifying downloaded tarball hash...".green()
	);
	let hash = crate::hash::hash_file(tarball_path.clone())?;
	let actual = hash.to_string();
	if actual != LIMINE_BOOTLOADER_TARBALL_HASH {
		return Err(BootloaderDownloadError::HashMismatch {
			expected: LIMINE_BOOTLOADER_TARBALL_HASH.to_string(),
			actual,
		});
	}

	// Unpack the tarball if the unpacked bootloader doesn't already exist.
	if !bootx_path.exists() {
		println!(
			"    {} {}",
			"󰇚".green().bold(),
			"Unpacking bootloader...".green()
		);
		std::fs::create_dir_all(&unpack_dir)?;
		let tar_f = std::fs::File::open(&tarball_path)?;
		let gz = flate2::read::GzDecoder::new(tar_f);
		let mut archive = tar::Archive::new(gz);
		archive.unpack(&unpack_dir)?;
		println!(
			"    {} {}",
			"󰇚".green().bold(),
			"Unpacked bootloader".green()
		);
	}

	if bootx_path.exists() {
		Ok(bootx_path)
	} else {
		Err(BootloaderDownloadError::IOError(std::io::Error::new(
			std::io::ErrorKind::NotFound,
			"bootloader UEFI file not found after unpacking",
		)))
	}
}
