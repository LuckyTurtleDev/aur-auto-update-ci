use anyhow::{bail, Context};
use clap::Parser;
use package_version::{Source, Sources};
use regex::Regex;
use serde::{Deserialize, Serialize};
use srcinfo::Srcinfo;
use std::{
	fs,
	fs::create_dir_all,
	io,
	path::{Path, PathBuf},
	process::exit,
};

#[derive(Debug, Default, Deserialize, Serialize)]
struct Index {
	#[serde(default)]
	tag: String,
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
	source: Sources,
	#[serde(default)]
	check: Check,
}

#[derive(Debug, Parser)]
struct Opt {
	/// aur package to be updated
	#[clap(required = true)]
	packages: Vec<String>,

	/// Treat <PACKAGES> like folders which inculde a PKGBUILD, instead cloning the aur repos which are associated with the packages.
	#[clap(short, long)]
	local: bool,

	/// Do not commit or push changes.
	#[clap(short, long)]
	dryrun: bool,

	/// Force run, even if no update is aviable.
	#[clap(short, long)]
	force: bool,

	/// Do not ask for confirmation when resolving dependencie.
	#[clap(short, long)]
	noconfirm: bool,
}

fn main() {
	let opt = Opt::parse();
	let mut error = 0;
	for package in &opt.packages {
		if let Err(err) = progess_package(package, &opt) {
			eprintln!("{:?}", err.context(format!("ERROR processing package {package}")));
			error += 1;
		}
		println!();
		println!();
	}
	if error != 0 {
		eprintln!("failed to process {error} packages");
		exit(1);
	}
}

fn run<P>(program: &str, args: &Vec<&str>, dir: P, show_output: bool) -> anyhow::Result<Vec<u8>>
where
	P: AsRef<Path>,
{
	use std::process::Command;
	let args = args;
	print!("Run {program}");
	for arg in args {
		print!(" {arg}");
	}
	println!();
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
		bail!("{program:?} has exit with {}", output.status);
	}
	Ok(output.stdout)
}

fn progess_package(package: &str, opt: &Opt) -> anyhow::Result<()> {
	println!("==> process package: {}", package);
	println!("-> clone package");
	let dir = if opt.local {
		PathBuf::from(package)
	} else {
		let dir = PathBuf::from("aur").join(package);
		create_dir_all("aur")?;
		run(
			"git",
			&vec![
				"clone",
				&format!("ssh://aur@aur.archlinux.org/{package}.git"),
				dir.to_str().unwrap(),
			],
			".",
			true,
		)?;
		dir
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
	if index.tag == tag.version && !opt.force {
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
			pkgbuild += &format!("_pkgtag={} #auto updated by CI", tag.version);
		} else {
			pkgbuild += line;
		}
		pkgbuild += "\n";
	}
	pkgbuild.pop(); //avoid adding an additonal newline at file end
	fs::write(dir.join("PKGBUILD"), pkgbuild).with_context(|| "failed to write PKGGBUILD")?;

	println!("-> updpkgsums");
	run("updpkgsums", &vec![], &dir, true)?;

	println!("-> makepkg --printsrcinfo");
	let mut stdout = run("makepkg", &vec!["--printsrcinfo"], &dir, false)?;
	let pkgver = Srcinfo::parse_buf(&*stdout)
		.with_context(|| "failed to prase .SRCINFO")?
		.base
		.pkgver;
	if old_pkgver.as_ref() != Some(&pkgver) {
		println!("set pkgrel to 1 (pkgver has change)");
		println!("-> modify PKGBUILD (again)");
		let mut pkgbuild = "".to_owned();
		for line in fs::read_to_string(dir.join("PKGBUILD"))
			.with_context(|| "failed to open `PKGBUILD`")?
			.split('\n')
		{
			if line.starts_with("pkgrel=") {
				pkgbuild += &format!("pkgrel=1 #auto reset by CI");
			} else {
				pkgbuild += line;
			}
			pkgbuild += "\n";
		}
		pkgbuild.pop(); //avoid adding an additonal newline at file end
		fs::write(dir.join("PKGBUILD"), pkgbuild).with_context(|| "failed to write PKGGBUILD")?;
		println!("-> makepkg --printsrcinfo");
		stdout = run("makepkg", &vec!["--printsrcinfo"], &dir, false)?;
	}
	if !pkgver_regex.is_match(&pkgver) {
		bail!(format!("pkgver {pkgver:?} does not match regex {pkgver_regex}"));
	};
	fs::write(dir.join(".SRCINFO"), stdout).with_context(|| "failed to write .SRCINFO")?;

	println!("-> makepkg");
	let mut args = vec!["--syncdeps", "--check", "--noarchive"];
	if opt.noconfirm {
		args.push("--noconfirm");
	}
	run("makepkg", &args, &dir, true)?;

	println!("-> write index.json");
	let mut new_index = index;
	new_index.tag = tag.version.to_owned();
	fs::write(dir.join(".index.json"), serde_json::to_string_pretty(&new_index)?)
		.with_context(|| "failed to write .index.json")?;

	if !opt.dryrun {
		println!("-> git commit");
		//gitoxide has not impl this yet
		run("git", &vec!["add", ".index.json", "PKGBUILD", ".SRCINFO"], &dir, true)?;
		run(
			"git",
			&vec!["commit", "--message", &format!("auto update to {pkgver}"), "."],
			&dir,
			true,
		)?;

		println!("-> git push");
		run("git", &vec!["push"], &dir, true)?;
	}
	Ok(())
}
