use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use colored::Colorize;
use thiserror::Error;

use crate::{
    hash::hash_file, manifest::{KernelOptionValue, Manifest}, prefix_commands
};

const KERNEL_IMAGE_NAME: &str = "ardos-packer-kernel-builder:latest";
const KERNEL_DOCKERFILE_CONTENT: &str = include_str!("../../../docker/kernel.Dockerfile");
const BUILD_SCRIPT: &str = r##"set -euo pipefail


DOWNLOADS="/kernel/downloads"
SRC="/kernel/src"
OUT="/kernel/out"
CONFIG="/kernel/config/options.config"

TARBALL="$(find "${DOWNLOADS}" -maxdepth 1 -type f | head -n1)"
if [[ -z "${TARBALL}" ]]; then
  echo "kernel tarball not found in ${DOWNLOADS}" >&2
  exit 1
fi

# ==========================================
# Incremental rebuild based on .gitignore
# ==========================================
KERNEL_TREE="$(find "${SRC}" -maxdepth 1 -type d -name 'linux-*' | head -n1 || true)"
if [[ -n "${KERNEL_TREE}" && -f "${KERNEL_TREE}/.gitignore" ]]; then
  echo "󰈸 Refreshing kernel source incrementally..."
  pushd "${KERNEL_TREE}" >/dev/null

  # Cria repositório git temporário se não existir
  if [[ ! -d .git ]]; then
  	git config --global --add safe.directory "$PWD"
    git init -q
  fi

  # Remove tudo o que NÃO está no .gitignore (mantém objectos de build)
  mapfile -t TRACKED < <(git ls-files --cached --others --exclude-from=.gitignore --exclude-standard || true)
  if [[ ${#TRACKED[@]} -gt 0 ]]; then
    echo "󰄾 Removing old tracked source files in chunks..."
    CHUNK_SIZE=500
    total=${#TRACKED[@]}
    for ((i=0; i<total; i+=CHUNK_SIZE)); do
      chunk=("${TRACKED[@]:i:CHUNK_SIZE}")
      printf '%s\0' "${chunk[@]}" | xargs -0 rm -rf -- || true
    done
  fi


  popd >/dev/null
else
  echo "󰈸 No existing source found, doing full extract..."
  rm -rf "${SRC:?}"/*
  mkdir -p "${SRC}"
fi

# Extrai nova source por cima (preserva ficheiros ignorados)
tar -xf "${TARBALL}" -C "${SRC}"

# ==========================================
# Build kernel
# ==========================================
NEW_TREE="$(find "${SRC}" -mindepth 1 -maxdepth 1 -type d -name 'linux-*' | head -n1)"
pushd "${NEW_TREE}" >/dev/null

make olddefconfig
KCONFIG_FILE=".config"
if [[ -f "${CONFIG}" ]]; then
  echo "󰌹 Applying kernel config overrides..."
  while IFS='=' read -r key value; do
    [[ -z "${key}" ]] && continue
    [[ "${key}" =~ ^# ]] && continue
    key="${key//[[:space:]]/}"
    key=${key#CONFIG_}
    key=${key#config_}
    key=${key#Config_}
    key="${key//-/_}"
    key="${key//./_}"
    value="${value//[[:space:]]/}"
    symbol="CONFIG_${key^^}"

    if [[ "${value}" == "y" ]]; then
      replacement="${symbol}=y"
    elif [[ "${value}" == "n" ]]; then
      replacement="${symbol}=n"
    elif [[ "${value}" =~ ^[0-9]+$ ]]; then
      replacement="${symbol}=${value}"
    else
      if [[ "${value}" =~ ^\".*\"$ || "${value}" =~ ^\'.*\'$ ]]; then
        strval="${value}"
      else
        esc=$(printf '%s' "${value}" | sed 's/"/\\"/g')
        strval="\"${esc}\""
      fi
      replacement="${symbol}=${strval}"
    fi

    escaped_replacement=$(printf '%s' "${replacement}" | sed -e 's/[&/|]/\\&/g')
    sed -i -e "s|^${symbol}=.*|${escaped_replacement}|" \
           -e "s|^# ${symbol} is not set|${escaped_replacement}|" "${KCONFIG_FILE}" || true
    if ! grep -q -E "^(# ${symbol} is not set|${symbol}=)" "${KCONFIG_FILE}" 2>/dev/null; then
      echo "${replacement}" >> "${KCONFIG_FILE}"
    fi
  done < "${CONFIG}"
fi

make -j"$(nproc)"

if [[ -f arch/x86/boot/bzImage ]]; then
  cp arch/x86/boot/bzImage "${OUT}/kernel"
else
  echo "Kernel not found after build" >&2
  exit 1
fi

popd >/dev/null
"##;

#[derive(Debug, Error)]
pub enum KernelBuildError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("failed to download kernel sources: {0}")]
    Download(#[from] ureq::Error),
    #[error("docker build failed with status code {0:?}")]
    DockerBuildFailed(Option<i32>),
    #[error("docker run failed with status code {0:?}")]
    DockerRunFailed(Option<i32>),
    #[error("kernel artifact not produced at {0}")]
    MissingArtifact(PathBuf),
}

pub struct KernelBuildResult {
    pub artifact_path: PathBuf,
}

impl KernelBuildResult {
    pub fn print(&self) {
        let artifact = self.artifact_path.display().to_string().green().bold();
        println!(
            "{} {}",
            "✔🐧 Kernel image available at".green().bold(),
            artifact
        );
    }
}

pub fn build(manifest: &Manifest) -> Result<KernelBuildResult, KernelBuildError> {
    let kernel = &manifest.kernel;
    let kernel_root = PathBuf::from("build/kernel");
    let downloads_dir = kernel_root.join("downloads");
    let src_dir = kernel_root.join("src");
    let out_dir = kernel_root.join("out");
    let config_dir = kernel_root.join("config");

    fs::create_dir_all(&downloads_dir)?;
    fs::create_dir_all(&src_dir)?;
    fs::create_dir_all(&out_dir)?;
    fs::create_dir_all(&config_dir)?;

    let tarball_name = extract_filename(&kernel.url).unwrap_or_else(|| "kernel.tar".to_string());
    let tarball_path = downloads_dir.join(&tarball_name);

    // --- Download if needed ---
    let needs_download =
        !tarball_path.exists() || tarball_path.metadata().map(|m| m.len()).unwrap_or(0) == 0;
    if needs_download {
        println!(
            "{} {}",
            "󰇚 Downloading kernel sources".green().bold(),
            kernel.url.cyan()
        );

        let tmp_path = tarball_path.with_extension("partial");
        match (|| -> Result<(), KernelBuildError> {
            let response = ureq::get(&kernel.url).call().map_err(KernelBuildError::Download)?;
            let mut reader = response.into_body().into_reader();
            let mut file = File::create(&tmp_path)?;
            std::io::copy(&mut reader, &mut file)?;
            fs::rename(&tmp_path, &tarball_path)?;
            Ok(())
        })() {
            Ok(_) => {}
            Err(e) => {
                let _ = fs::remove_file(&tmp_path);
                let _ = fs::remove_file(&tarball_path);
                return Err(e);
            }
        }

        // --- Validate tarball type using `file` ---
        let output = Command::new("file")
            .arg("--brief")
            .arg("--mime-type")
            .arg(&tarball_path)
            .output()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "failed to run `file`"))?;

        let mime = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !mime.contains("gzip")
            && !mime.contains("xz")
            && !mime.contains("bzip2")
            && !mime.contains("tar")
        {
            println!(
                "{} {} ({})",
                "󰈸 Invalid kernel tarball detected!".red().bold(),
                tarball_path.display(),
                mime
            );
            let _ = fs::remove_file(&tarball_path);
            return Err(KernelBuildError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid tarball type: {mime}"),
            )));
        }

        println!(
            "{} {}",
            "󰜥 Valid kernel tarball detected:".green().bold(),
            mime
        );
    }

    // --- Calculate tarball hash ---
    let current_hash = hash_file(&tarball_path)?.to_string();

    let hash_path = out_dir.join("kernel.hash");
    if hash_path.exists() {
        let old_hash = fs::read_to_string(&hash_path).unwrap_or_default();
        if old_hash.trim() == current_hash {
            let artifact_path = locate_artifact(&out_dir)?;
            println!(
                "{} {}",
                "󰞇 Kernel source unchanged, skipping rebuild →".yellow().bold(),
                artifact_path.display()
            );
            return Ok(KernelBuildResult { artifact_path });
        }
    }

    // --- Write config options ---
    let options_path = config_dir.join("options.config");
    write_options_file(&options_path, &kernel.options)?;

    // --- Build Docker image if needed ---
    let dockerfile_path = kernel_root.join("kernel.Dockerfile");
    fs::write(&dockerfile_path, KERNEL_DOCKERFILE_CONTENT)?;
    ensure_kernel_builder_image(&dockerfile_path)?;

    // --- Canonical paths ---
    let downloads_dir = canonicalize(&downloads_dir)?;
    let src_dir = canonicalize(&src_dir)?;
    let out_dir = canonicalize(&out_dir)?;
    let config_dir = canonicalize(&config_dir)?;

    println!("{}", "🐧 Building kernel inside container".blue().bold());
    let mut command = Command::new("docker");
    command
        .arg("run")
        .arg("--rm")
        .arg("-v")
        .arg(format!("{}:/kernel/downloads:ro", downloads_dir.display()))
        .arg("-v")
        .arg(format!("{}:/kernel/src", src_dir.display()))
        .arg("-v")
        .arg(format!("{}:/kernel/out", out_dir.display()))
        .arg("-v")
        .arg(format!("{}:/kernel/config:ro", config_dir.display()))
        .arg(KERNEL_IMAGE_NAME)
        .arg("bash")
        .arg("-c")
        .arg(BUILD_SCRIPT);

    let status = prefix_commands::run_command_with_tag(
        command,
        "       [ 🐧 kernel-build ] ".blue().to_string(),
    )?;
    if !status.success() {
        return Err(KernelBuildError::DockerRunFailed(status.code()));
    }

    let artifact_path = locate_artifact(&out_dir)?;
    fs::write(&hash_path, &current_hash)?;
    Ok(KernelBuildResult { artifact_path })
}


fn write_options_file(
    path: &Path,
    options: &crate::manifest::KernelOptions,
) -> Result<(), io::Error> {
    let mut file = File::create(path)?;
    for (key, value) in options {
        let normalized_key = normalize_option_key(key);
        let val = match value {
            KernelOptionValue::Number(n) => n.to_string(),
            KernelOptionValue::String(s) => match s.as_str() {
                "m" | "y" | "n" => s.clone(),
                s => format!("\"{s}\""),
            },
        };
        writeln!(file, "{normalized_key}={val}")?;
    }
    Ok(())
}

fn normalize_option_key(raw: &str) -> String {
    let trimmed = raw.trim().trim_start_matches("CONFIG_");
    trimmed
        .trim_start_matches("config_")
        .trim_start_matches("Config_")
        .replace('-', "_")
        .replace('.', "_")
        .to_uppercase()
}

fn extract_filename(url: &str) -> Option<String> {
    let without_query = url.split('?').next().unwrap_or(url);
    without_query
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn ensure_kernel_builder_image(dockerfile_path: &Path) -> Result<(), KernelBuildError> {
    let inspect_status = Command::new("docker")
        .args(["image", "inspect", KERNEL_IMAGE_NAME])
        .stdout(Stdio::null())
        .status()?;
    if inspect_status.success() {
        return Ok(());
    }

    let dockerfile_path = dockerfile_path.canonicalize()?;
    let build_context = dockerfile_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    println!("{}", "󱌢 Building kernel builder Docker image".blue().bold());
    let mut command = Command::new("docker");
    command
        .arg("build")
        .arg("-f")
        .arg(&dockerfile_path)
        .arg("-t")
        .arg(KERNEL_IMAGE_NAME)
        .arg(&build_context);
    let status = prefix_commands::run_command_with_tag(
        command,
        "       [ 🐧 kernel-image ] ".blue().to_string(),
    )?;
    if !status.success() {
        return Err(KernelBuildError::DockerBuildFailed(status.code()));
    }
    Ok(())
}

fn canonicalize(path: &Path) -> Result<PathBuf, io::Error> {
    std::fs::canonicalize(path)
}

fn locate_artifact(out_dir: &Path) -> Result<PathBuf, KernelBuildError> {
    let kernel_path = out_dir.join("kernel");
    if kernel_path.exists() {
        return Ok(kernel_path);
    }
    Err(KernelBuildError::MissingArtifact(kernel_path))
}
