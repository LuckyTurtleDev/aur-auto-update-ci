use anyhow::{self, Context};
use basic_toml;
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json;
use srcinfo::Srcinfo;
use std::{fs, io, path::PathBuf};

mod github;

#[derive(Debug, Default, Deserialize, Serialize)]
struct Index {
	#[serde(default)]
	i: u8,
	#[serde(default)]
	version: String,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
	source: Source,
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
}

fn main() {
	let opt = Opt::parse();
	for package in &opt.packages {
		if let Err(err) = progess_package(package, &opt) {
			eprintln!("{:?}", err.context(format!("ERROR processing package {package}")));
		}
		println!();
	}
}

fn progess_package(package: &str, opt: &Opt) -> anyhow::Result<()> {
	println!("==> process package: {}", package);
	println!("-> load .index.json");
	let file_content = match fs::read_to_string(PathBuf::from(package).join(".index.json")) {
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
	let config: Config = basic_toml::from_str(
		&fs::read_to_string(PathBuf::from(package).join("ci.toml")).with_context(|| "failed to open `ci.toml`")?,
	)
	.with_context(|| "failed to prase `ci.toml`")?;
	println!("-> load .SRCINFO");
	let old_pkgver = match fs::read(PathBuf::from(package).join(".SRCINFO")) {
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
	Ok(())
}
