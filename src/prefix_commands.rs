use std::{
	io::{BufRead, BufReader},
	os::unix::process::CommandExt,
	os::unix::process::ExitStatusExt,
	process::{Command, ExitStatus, Stdio},
	sync::{Mutex, OnceLock},
};

use colored::Colorize;

fn active_child_groups() -> &'static Mutex<Vec<i32>> {
	static ACTIVE_CHILD_GROUPS: OnceLock<Mutex<Vec<i32>>> = OnceLock::new();
	ACTIVE_CHILD_GROUPS.get_or_init(|| Mutex::new(Vec::new()))
}

fn active_docker_containers() -> &'static Mutex<Vec<String>> {
	static ACTIVE_DOCKER_CONTAINERS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
	ACTIVE_DOCKER_CONTAINERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn install_signal_cleanup_handler() {
	static HANDLER_INSTALLED: OnceLock<()> = OnceLock::new();
	HANDLER_INSTALLED.get_or_init(|| {
		ctrlc::set_handler(|| {
			let containers = active_docker_containers()
				.lock()
				.map(|containers| containers.clone())
				.unwrap_or_default();
			let groups = active_child_groups()
				.lock()
				.map(|groups| groups.clone())
				.unwrap_or_default();
			if groups.is_empty() && containers.is_empty() {
				std::process::exit(130);
			}
			for pgid in groups {
				unsafe {
					libc::killpg(pgid, libc::SIGTERM);
				}
			}
			std::thread::sleep(std::time::Duration::from_millis(250));
			for pgid in active_child_groups()
				.lock()
				.map(|groups| groups.clone())
				.unwrap_or_default()
			{
				unsafe {
					libc::killpg(pgid, libc::SIGKILL);
				}
			}
			for container in containers {
				let _ = Command::new("docker")
					.args(["rm", "-f", &container])
					.stdout(Stdio::null())
					.stderr(Stdio::null())
					.status();
			}
			std::process::exit(130);
		})
		.expect("failed to install Ctrl-C handler");
	});
}

fn register_child_group(pgid: i32) {
	let mut groups = active_child_groups()
		.lock()
		.expect("failed to lock active child group registry");
	if !groups.contains(&pgid) {
		groups.push(pgid);
	}
}

fn unregister_child_group(pgid: i32) {
	if let Ok(mut groups) = active_child_groups().lock() {
		groups.retain(|group| *group != pgid);
	}
}

pub fn register_docker_container(name: &str) {
	let mut containers = active_docker_containers()
		.lock()
		.expect("failed to lock active docker container registry");
	if !containers.iter().any(|container| container == name) {
		containers.push(name.to_owned());
	}
}

pub fn unregister_docker_container(name: &str) {
	if let Ok(mut containers) = active_docker_containers().lock() {
		containers.retain(|container| container != name);
	}
}

pub fn unique_docker_container_name(prefix: &str) -> String {
	use std::sync::atomic::{AtomicU64, Ordering};
	static COUNTER: AtomicU64 = AtomicU64::new(0);
	let count = COUNTER.fetch_add(1, Ordering::Relaxed);
	let sanitized_prefix: String = prefix
		.chars()
		.map(|c| match c {
			'a'..='z' | '0'..='9' => c,
			'A'..='Z' => c.to_ascii_lowercase(),
			_ => '-',
		})
		.collect();
	format!(
		"ardos-packer-{}-{}-{}",
		sanitized_prefix.trim_matches('-'),
		std::process::id(),
		count
	)
}

/// Runs commands but adds a tag to each log line the process prints to the stdout/stderr
pub fn run_command_with_tag(
	mut command: Command,
	tag: String,
) -> Result<ExitStatus, std::io::Error> {
	install_signal_cleanup_handler();
	command.stdout(Stdio::piped());
	command.stderr(Stdio::piped());
	command.stdin(Stdio::piped());
	unsafe {
		command.pre_exec(|| {
			if libc::setpgid(0, 0) != 0 {
				return Err(std::io::Error::last_os_error());
			}
			Ok(())
		});
	}
	let mut child = command.spawn()?;
	let child_group = child.id() as i32;
	register_child_group(child_group);
	let stderr = child.stderr.take().unwrap();
	let stdout = child.stdout.take().unwrap();
	std::thread::scope(|s| {
		s.spawn(|| {
			let buf_reader = BufReader::new(stderr);
			for line in buf_reader.lines().filter_map(Result::ok) {
				let line = line
					.replace("\r\n", "\n")
					.replace("\r", &format!("\r{tag}"))
					.replace("\n", &format!("\n{tag}"));
				eprintln!("{}", format!("{tag}{line}").dimmed());
			}
		});
		s.spawn(|| {
			let buf_reader = BufReader::new(stdout);
			for line in buf_reader.lines().filter_map(Result::ok) {
				let line = line
					.replace("\r\n", "\n")
					.replace("\r", &format!("\r{tag}"))
					.replace("\n", &format!("\n{tag}"));
				println!("{}", format!("{tag}{line}").dimmed());
			}
		});
	});
	let status = child.wait()?;
	unregister_child_group(child_group);
	if let Some(signal) = status.signal() {
		std::process::exit(128 + signal);
	}
	if status.code() == Some(130) {
		std::process::exit(130);
	}
	Ok(status)
}
