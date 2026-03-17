use std::{fs::DirEntry, path::Path, time::SystemTime};

pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
	copy_dir_all_with_filter(src, dst, |_| true)
}
pub fn copy_dir_all_with_filter(
	src: impl AsRef<Path>,
	dst: impl AsRef<Path>,
	filter: impl Fn(&DirEntry) -> bool,
) -> std::io::Result<()> {
	std::fs::create_dir_all(&dst)?;
	for entry in std::fs::read_dir(src)? {
		let entry = entry?;
		if !filter(&entry) {
			continue;
		}
		let ty = entry.file_type()?;
		if ty.is_symlink() {
			let link_target = std::fs::read_link(entry.path())?;
			let dest = dst.as_ref().join(entry.file_name());
			#[cfg(unix)]
			{
				std::os::unix::fs::symlink(link_target, dest)?;
			}
			#[cfg(not(unix))]
			{
				return Err(std::io::Error::new(
					std::io::ErrorKind::Unsupported,
					"copy_dir_all_with_filter: symlinks are only supported on unix",
				));
			}
		} else if ty.is_dir() {
			copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
		} else {
			std::fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::fs;

	#[test]
	fn copy_dir_preserves_symlinks() {
		let root = std::env::temp_dir().join(format!(
			"ardos-packer-fs-utils-{}-{}",
			std::process::id(),
			std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.unwrap()
				.as_nanos()
		));
		let src = root.join("src");
		let dst = root.join("dst");
		fs::create_dir_all(&src).unwrap();
		fs::create_dir_all(&dst).unwrap();

		fs::write(src.join("real.txt"), "hello").unwrap();
		#[cfg(unix)]
		std::os::unix::fs::symlink("real.txt", src.join("link.txt")).unwrap();

		copy_dir_all(&src, &dst).unwrap();

		let dst_link = dst.join("link.txt");
		let meta = fs::symlink_metadata(&dst_link).unwrap();
		assert!(meta.file_type().is_symlink());
		assert_eq!(fs::read_link(&dst_link).unwrap(), std::path::PathBuf::from("real.txt"));

		fs::remove_dir_all(&root).ok();
	}
}

pub fn has_file_newer_than(dir: &Path, timestamp: SystemTime) -> std::io::Result<bool> {
	if !dir.exists() {
		return Ok(false);
	}

	for entry in std::fs::read_dir(dir)? {
		let entry = entry?;
		let path = entry.path();
		let metadata = entry.metadata()?;

		if metadata.is_dir() {
			if has_file_newer_than(&path, timestamp)? {
				return Ok(true);
			}
		} else if let Ok(modified) = metadata.modified() {
			if modified > timestamp {
				return Ok(true);
			}
		}
	}

	Ok(false)
}
