use std::{
	collections::{HashMap, HashSet},
	fs::File,
	io::{Read, Result as IoResult},
	path::PathBuf,
	process::Command,
};

use colored::Colorize;
use thiserror::Error;

use crate::{
	credits,
	manifest::{Manifest, Package},
	prefix_commands,
};

fn is_elf_file(path: &std::path::Path) -> IoResult<bool> {
	let mut file = File::open(path)?;
	let mut magic = [0u8; 4];
	if file.read_exact(&mut magic).is_err() {
		return Ok(false);
	}
	Ok(magic == [0x7f, b'E', b'L', b'F'])
}

fn collect_files_recursive(root: &std::path::Path) -> IoResult<Vec<PathBuf>> {
	let mut out = Vec::new();
	let mut stack = vec![root.to_path_buf()];
	while let Some(dir) = stack.pop() {
		let entries = match std::fs::read_dir(&dir) {
			Ok(e) => e,
			Err(_) => continue,
		};
		for entry in entries {
			let entry = match entry {
				Ok(e) => e,
				Err(_) => continue,
			};
			let path = entry.path();
			let ty = match entry.file_type() {
				Ok(t) => t,
				Err(_) => continue,
			};
			if ty.is_dir() {
				stack.push(path);
			} else if ty.is_file() || ty.is_symlink() {
				out.push(path);
			}
		}
	}
	Ok(out)
}

fn parse_readelf_bracket_value(line: &str) -> Option<String> {
	let start = line.find('[')? + 1;
	let end = line[start..].find(']')? + start;
	Some(line[start..end].trim().to_string())
}

fn readelf_needed(path: &std::path::Path) -> IoResult<Vec<String>> {
	let output = Command::new("readelf").args(["-d"]).arg(path).output()?;
	if !output.status.success() {
		return Ok(Vec::new());
	}
	let stdout = String::from_utf8_lossy(&output.stdout);
	let mut needed = Vec::new();
	for line in stdout.lines() {
		if line.contains("(NEEDED)") {
			if let Some(val) = parse_readelf_bracket_value(line) {
				needed.push(val);
			}
		}
	}
	Ok(needed)
}

fn readelf_interp(path: &std::path::Path) -> IoResult<Option<String>> {
	let output = Command::new("readelf").args(["-l"]).arg(path).output()?;
	if !output.status.success() {
		return Ok(None);
	}
	let stdout = String::from_utf8_lossy(&output.stdout);
	for line in stdout.lines() {
		if let Some(idx) = line.find("Requesting program interpreter:") {
			let Some(rest) = line[idx..].splitn(2, ':').nth(1) else {
				continue;
			};
			let interp = rest
				.trim()
				.trim_start_matches('[')
				.trim_end_matches(']')
				.trim();
			return Ok(Some(interp.to_string()));
		}
	}
	Ok(None)
}

fn verify_sysroot_shared_deps(sysroot: &std::path::Path) -> Result<(), String> {
	let files = collect_files_recursive(sysroot).map_err(|e| e.to_string())?;

	let mut names_in_sysroot: HashSet<String> = HashSet::new();
	for path in &files {
		if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
			names_in_sysroot.insert(name.to_string());
		}
	}

	let mut missing: HashMap<String, Vec<PathBuf>> = HashMap::new();
	let mut missing_interpreters: HashMap<String, Vec<PathBuf>> = HashMap::new();

	for path in files {
		let meta = match std::fs::metadata(&path) {
			Ok(m) => m,
			Err(_) => continue,
		};
		if !meta.is_file() {
			continue;
		}
		if !is_elf_file(&path).unwrap_or(false) {
			continue;
		}

		if let Ok(Some(interp)) = readelf_interp(&path) {
			if interp.starts_with('/') {
				let interp_in_sysroot = sysroot.join(interp.trim_start_matches('/'));
				if !interp_in_sysroot.exists() {
					missing_interpreters.entry(interp).or_default().push(path.clone());
				}
			}
		}

		let needed = match readelf_needed(&path) {
			Ok(n) => n,
			Err(_) => continue,
		};
		for lib in needed {
			if lib == "linux-vdso.so.1" {
				continue;
			}
			if !names_in_sysroot.contains(&lib) {
				missing.entry(lib).or_default().push(path.clone());
			}
		}
	}

	if missing.is_empty() && missing_interpreters.is_empty() {
		return Ok(());
	}

	let mut msg = String::new();
	if !missing_interpreters.is_empty() {
		msg.push_str("Missing ELF interpreters (PT_INTERP):\n");
		let mut keys: Vec<_> = missing_interpreters.keys().cloned().collect();
		keys.sort();
		for k in keys {
			msg.push_str(&format!("  - {k}\n"));
			let mut deps = missing_interpreters.get(&k).cloned().unwrap_or_default();
			deps.sort();
			for p in deps.iter().take(25) {
				msg.push_str(&format!("      * {}\n", p.display()));
			}
			let total = missing_interpreters.get(&k).map(|v| v.len()).unwrap_or(0);
			if total > 25 {
				msg.push_str(&format!("      * … +{} more\n", total - 25));
			}
		}
	}

	if !missing.is_empty() {
		if !msg.is_empty() {
			msg.push('\n');
		}
		msg.push_str("Missing DT_NEEDED shared libraries:\n");
		let mut keys: Vec<_> = missing.keys().cloned().collect();
		keys.sort();
		for k in keys {
			msg.push_str(&format!("  - {k}\n"));
			let mut deps = missing.get(&k).cloned().unwrap_or_default();
			deps.sort();
			for p in deps.iter().take(25) {
				msg.push_str(&format!("      * {}\n", p.display()));
			}
			let total = missing.get(&k).map(|v| v.len()).unwrap_or(0);
			if total > 25 {
				msg.push_str(&format!("      * … +{} more\n", total - 25));
			}
		}
	}

	Err(msg)
}

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
	#[error("Sysroot has missing shared library dependencies:\n{details}")]
	SysrootDeps { details: String },
	#[error("Failed to create squashfs image: {0}")]
	SquashfsError(#[from] SquashFsError),
	#[error("io error: {0}")]
	Io(#[from] std::io::Error),
}

pub fn assemble<'m>(manifest: &'m Manifest) -> Result<PathBuf, AssembleError<'m>> {
	let sysroot_folder = PathBuf::from("build/sysroot");
	std::fs::remove_dir_all(&sysroot_folder).ok();
	std::fs::create_dir_all(&sysroot_folder)?;
	let image_file_name = format!(
		"ardos-{}-{}.squashfs",
		manifest.version,
		get_git_commit_hash().unwrap_or(String::from("unknown"))
	);

	// Build credits JSON now (written into the sysroot later in fakeroot).
	let credits = credits::generate_credits(&manifest.packages);
	let credits_json = serde_json::to_string_pretty(&credits).unwrap();
	let credits_tmp = PathBuf::from("build").join("credits.json");
	std::fs::create_dir_all("build")?;
	std::fs::write(&credits_tmp, credits_json)?;

	// Extract packages into the sysroot inside a fakeroot session.
	// This preserves per-file uid/gid/mode from the package archives without making the on-disk files root-owned.
	let fakeroot_state = PathBuf::from("build").join("fakeroot.state");
	std::fs::remove_file(&fakeroot_state).ok();

	let mut all_archives: Vec<PathBuf> = Vec::new();
	for pkg in manifest.packages.iter() {
		let paths = pkg.get_built_archlinux_pkgs_paths().unwrap_or_default();
		all_archives.extend(paths);
	}

	println!(
		"    {} {}",
		"󱁥  Extracting packages into sysroot".yellow().bold(),
		format!("({} archives)", all_archives.len()).dimmed()
	);
	let mut extract_cmd = Command::new("fakeroot");
	extract_cmd
		.env("SYSROOT", &sysroot_folder)
		.env("CREDITS_SRC", &credits_tmp)
		.args(["-s", fakeroot_state.to_string_lossy().as_ref()])
		.arg("sh")
		.arg("-ec")
		.arg(
			r#"
mkdir -p "$SYSROOT"
for archive in "$@"; do
  tar --zstd --numeric-owner --same-owner --same-permissions -xpf "$archive" -C "$SYSROOT"
done

# Drop Arch package metadata files if present
rm -f "$SYSROOT/.BUILDINFO" "$SYSROOT/.MTREE" "$SYSROOT/.PKGINFO"

# Merged-/usr compatibility: provide /lib and /lib64 when missing.
if [ ! -e "$SYSROOT/lib" ]; then ln -s usr/lib "$SYSROOT/lib"; fi
if [ ! -e "$SYSROOT/lib64" ]; then ln -s usr/lib "$SYSROOT/lib64"; fi

# Ensure a canonical runtime linker path exists in the sysroot.
if [ -e "$SYSROOT/usr/lib/ld-linux-x86-64.so.2" ] && [ ! -e "$SYSROOT/lib64/ld-linux-x86-64.so.2" ]; then
  mkdir -p "$SYSROOT/lib64"
  ln -s ../usr/lib/ld-linux-x86-64.so.2 "$SYSROOT/lib64/ld-linux-x86-64.so.2"
fi

mkdir -p "$SYSROOT/etc"
cp -f "$CREDITS_SRC" "$SYSROOT/etc/credits.json"

# Virtual filesystem mountpoints
mkdir -p "$SYSROOT/proc" "$SYSROOT/sys" "$SYSROOT/dev" "$SYSROOT/tmp"
chmod 0555 "$SYSROOT/proc" "$SYSROOT/sys" || true
chmod 0755 "$SYSROOT/dev" || true
chmod 1777 "$SYSROOT/tmp" || true

if [ -x scripts/fakeroot-sysroot.sh ]; then
  sh scripts/fakeroot-sysroot.sh
fi
"#,
		)
		.arg("--");
	for archive in &all_archives {
		extract_cmd.arg(archive);
	}
	let extract_status = prefix_commands::run_command_with_tag(
		extract_cmd,
		"       [ fakeroot | extract ] ".blue().to_string(),
	)
	.map_err(SquashFsError::CommandError)?;
	if !extract_status.success() {
		return Err(AssembleError::SquashfsError(SquashFsError::Non0ExitCode {
			exit_code: extract_status.code().unwrap_or(-1),
		}));
	}

	verify_sysroot_shared_deps(&sysroot_folder).map_err(|details| AssembleError::SysrootDeps { details })?;

	let images_path = PathBuf::from("build/images");
	std::fs::create_dir_all(images_path)?;
	let image_path = PathBuf::from("build/images").join(&image_file_name);
	println!(
		"     {} {}",
		"→󰋩← Creating image".yellow().bold(),
		image_file_name
	);

	// Run mksquashfs inside the same fakeroot state used for extraction so uid/gid/mode are preserved.
	let mut command = Command::new("fakeroot");
	command
		.args(["-i", fakeroot_state.to_string_lossy().as_ref()])
		.arg("mksquashfs")
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
	std::fs::remove_file(&fakeroot_state).ok();
	// rodar comando do squashfs aqui
	Ok(image_path)
}
