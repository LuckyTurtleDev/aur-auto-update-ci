use anyhow::{bail, Context};
use clap::Parser;
use core::sync::atomic::AtomicBool;
use gix::progress::Discard;
use regex::Regex;
use serde::{Deserialize, Serialize};
use srcinfo::Srcinfo;
use std::{
	ffi::OsStr,
	fs,
	fs::create_dir_all,
	io,
	path::{Path, PathBuf},
};

mod github;

#[derive(Debug, Default, Deserialize, Serialize)]
struct Index {
	#[serde(default)]
	tag: String,
}

trait Tags {
	fn get_tags(&self) -> anyhow::Result<Vec<String>>;
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Source {
	GithubRelease(github::GithubRelease),
}

impl Tags for Source {
	fn get_tags(&self) -> anyhow::Result<Vec<String>> {
		match self {
			Self::GithubRelease(value) => value.get_tags(),
		}
	}
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
	#[serde(default = "default_pkgver_regex")]
	pkgver_regex: String,
}

fn default_pkgver_regex() -> String {
	r#"^[0-9]+(\.[0-9]+)+$"#.to_owned()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
	source: Source,
	#[serde(default)]
	check: Check,
}

#[derive(Debug, Parser)]
struct Opt {
	/// aur package to be updated
	#[clap(required = true)]
	packages: Vec<String>,

	/// run at local directory instead
	#[clap(short = 'l', long)]
	local: bool,

	/// Does not commit or push changes
	#[clap(short = 'd', long)]
	dryrun: bool,

	/// force run, even if no update is aviable
	#[clap(short = 'f', long)]
	force: bool,
}

fn main() {
	let opt = Opt::parse();
	for package in &opt.packages {
		if let Err(err) = progess_package(package, &opt) {
			eprintln!("{:?}", err.context(format!("ERROR processing package {package}")));
		}
		println!();
		println!();
	}
}

fn run<P, S>(program: &str, args: Option<&[S]>, dir: P, show_output: bool) -> anyhow::Result<Vec<u8>>
where
	P: AsRef<Path>,
	S: AsRef<OsStr>,
{
	use std::process::Command;
	let args: &[S] = args.unwrap_or_default();
	let mut command = Command::new(program);
	let command = command.current_dir(dir).args(args);
	let output = if show_output {
		let child = command.spawn().with_context(|| format!("failed to start {program:?}"))?;
		child.wait_with_output()?
	} else {
		let output = command.output().with_context(|| format!("failed to start {program:?}"))?;
		let stderr = String::from_utf8_lossy(&output.stderr);
		print!("{}", stderr);
		output
	};
	if !output.status.success() {
		bail!("{program:?} has exit with exit code {}", output.status);
	}
	Ok(output.stdout)
}

fn progess_package(package: &str, opt: &Opt) -> anyhow::Result<()> {
	println!("==> process package: {}", package);
	println!("-> clone package");
	let (dir, _repo) = if opt.local {
		let repo = gix::open_opts(package, Default::default())?;
		(PathBuf::from(package), repo)
	} else {
		let dir = PathBuf::from("aur").join(package);
		create_dir_all("aur")?;
		let mut prepare_fetch = gix::prepare_clone(format!("ssh://aur@aur.archlinux.org/{package}.git"), &dir)?;
		let (mut prepare_checkout, _) = prepare_fetch.fetch_then_checkout(Discard, &AtomicBool::new(false))?;
		let (repo, _) = prepare_checkout.main_worktree(Discard, &AtomicBool::new(false))?;
		(dir, repo)
	};

	println!("-> load .index.json");
	let file_content = match fs::read_to_string(dir.join(".index.json")) {
		Ok(value) => value,
		Err(err) => {
			if err.kind() != io::ErrorKind::NotFound {
				return Err(anyhow::Error::from(err)).with_context(|| "failed to open .index.json");
			}
			"{}".to_owned()
		},
	};
	let index: Index = serde_json::from_str(&file_content).with_context(|| "failed to prase .index.json")?;

	println!("-> load config.toml");
	let config: Config =
		basic_toml::from_str(&fs::read_to_string(dir.join("ci.toml")).with_context(|| "failed to open `ci.toml`")?)
			.with_context(|| "failed to prase `ci.toml`")?;
	let pkgver_regex = Regex::new(&config.check.pkgver_regex).with_context(|| "invaild regex at field \"pkgver_regex\"")?;

	println!("-> load .SRCINFO");
	let old_pkgver = match fs::read(dir.join(".SRCINFO")) {
		Ok(value) => Some(
			Srcinfo::parse_buf(&*value)
				.with_context(|| "failed to prase .SRCINFO")?
				.base
				.pkgver,
		),
		Err(err) => {
			if err.kind() != io::ErrorKind::NotFound {
				return Err(anyhow::Error::from(err)).with_context(|| "failed to open .SRCINFO");
			}
			None
		},
	};
	let tags = config.source.get_tags().with_context(|| "failed to get tags")?;

	println!("tags: {tags:?}");
	let tag = tags.first().expect("no suitable tag found");
	if &index.tag == tag && !opt.force {
		println!("package is already uptodate");
		return Ok(());
	}

	println!("-> modify PKGBUILD");
	let mut pkgbuild = "".to_owned();
	for line in fs::read_to_string(dir.join("PKGBUILD"))
		.with_context(|| "failed to open `PKGBUILD`")?
		.split('\n')
	{
		if line.starts_with("_pkgtag=") {
			pkgbuild += &format!("_pkgtag={tag} #auto updated by CI");
		} else {
			pkgbuild += line;
		}
		pkgbuild += "\n";
	}
	pkgbuild.pop(); //avoid adding an additonal newline at file end
	fs::write(dir.join("PKGBUILD"), pkgbuild).with_context(|| "failed to write PKGGBUILD")?;

	println!("-> updpkgsums");
	run::<_, &str>("updpkgsums", None, &dir, true)?;

	println!("-> makepkg --printsrcinfo");
	let stdout = run("makepkg", Some(&["--printsrcinfo"]), &dir, false)?;
	let pkgver = Srcinfo::parse_buf(&*stdout)
		.with_context(|| "failed to prase .SRCINFO")?
		.base
		.pkgver;
	if !pkgver_regex.is_match(&pkgver) {
		bail!(format!("pkgver {pkgver:?} does not match regex {pkgver_regex}"));
	};
	fs::write(dir.join(".SRCINFO"), stdout).with_context(|| "failed to write .SRCINFO")?;

	println!("-> makepkg");
	run("makepkg", Some(&["--syncdeps", "--check", "--noarchive"]), &dir, true)?;

	println!("-> write index.json");
	let mut new_index = index;
	new_index.tag = tag.to_owned();
	fs::write(dir.join(".index.json"), serde_json::to_string_pretty(&new_index)?)
		.with_context(|| "failed to write .index.json")?;

	if !opt.dryrun {
		println!("-> git commit");
		//gitoxide has not impl this yet
		run("git", Some(&["add", ".index.json", "PKGBUILD", ".SRCINFO"]), &dir, true)?;
		run(
			"git",
			Some(&["commit", "--message", &format!("auto update to {pkgver}"), "."]),
			&dir,
			true,
		)?;

		println!("-> git push");
		run("git", Some(&["push"]), &dir, true)?;
	}
	Ok(())
}
