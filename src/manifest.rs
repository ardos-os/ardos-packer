use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use crate::hash::Sha256Hash;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Manifest {
	pub version: String,
	pub kernel: Kernel,
	pub initrd: InitrdOptions,
	#[serde(default)]
	pub hooks: Hooks,
	#[serde(rename = "package", default = "Vec::new")]
	pub packages: Vec<Package>,
	#[serde(skip, default)]
	pub manifest_dir: PathBuf,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Hooks {
	#[serde(rename = "post-processing")]
	pub post_processing: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Kernel {
	pub url: String,
	#[serde(default)]
	pub options: KernelOptions,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum KernelOptionValue {
	String(String),
	Number(u64),
}
pub type KernelOptions = BTreeMap<String, KernelOptionValue>;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Package {
	pub name: String,
	pub version: String,
	pub author: Option<String>,
	pub source: Source,
	#[serde(default)]
	pub docker: DockerSettings,
	#[serde(default)]
	pub build_deps: HashSet<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct InitrdOptions {
	pub build_script: PathBuf,
}

#[derive(Debug, Deserialize, Serialize, Clone, Hash)]
#[serde(untagged, rename_all = "snake_case")]
pub enum DockerSettings {
	DockerfilePath {
		#[serde(rename = "dockerfile_path")]
		path: PathBuf,
	},
	ImageName {
		#[serde(rename = "image_name")]
		name: String,
	},
}
impl Default for DockerSettings {
	fn default() -> Self {
		DockerSettings::ImageName {
			name: "archlinux:multilib-devel".to_string(),
		}
	}
}
#[derive(Debug, Deserialize, Serialize, Clone, Hash)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum Source {
	/// Pacote binário (já compilado)
	Binary {
		url: String,
		#[serde(default = "crate::hash::default_hash")]
		sha256: Sha256Hash,
	},
	/// PKGBUILD local
	PkgBuildLocal {
		path: PathBuf,
		pick_packages_from_group: Option<Vec<String>>,
	},
	/// PKGBUILD remoto via git
	PkgBuildGit {
		repo_url: String,
		rev: String,
		#[serde(default = "crate::hash::default_hash")]
		sha256: Sha256Hash,
		pick_packages_from_group: Option<Vec<String>>,
	},
}

#[derive(Debug, Error)]
pub enum InvalidSourceError {
	#[error("unsupported source type for this operation")]
	UnsupportedSourceType,
	#[error("failed to construct tarball url from git source")]
	InvalidGitSourceUrl,
}
#[derive(Debug, Error)]
pub enum SourceFetchError {
	#[error("IO error: {0}")]
	Io(#[from] std::io::Error),
	#[error("failed to fetch tarball")]
	FetchError(#[from] ureq::Error),
	#[error("Hash mismatch")]
	HashMismatch {
		expected: Sha256Hash,
		actual: Sha256Hash,
	},
	#[error("invalid source: {0}")]
	InvalidSource(#[from] InvalidSourceError),
}

pub struct GarbageCollectionStat {
	pub freed_bytes: u64,
	pub removed_out_folders: usize,
	pub removed_prepared_packages: usize,
	pub removed_sources_packages: usize,
}
