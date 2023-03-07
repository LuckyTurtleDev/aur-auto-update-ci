use crate::Tags;
use attohttpc::get;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
struct Release {
	draft: bool,
	prerelease: bool,
	tag_name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubRelease {
	repo: String,
	#[serde(default)]
	prerelease: bool,
}

impl Tags for GithubRelease {
	fn get_tags(&self) -> anyhow::Result<Vec<String>> {
		println!("-> get tags from github releases");
		let releases: Vec<Release> = get(format!("https://api.github.com/repos/{}/releases", self.repo))
			.send()?
			.error_for_status()?
			.json()?;
		Ok(releases
			.into_iter()
			.filter_map(|f| {
				if f.draft || (f.prerelease && !self.prerelease) {
					None
				} else {
					Some(f.tag_name)
				}
			})
			.collect())
	}
}

#[derive(Debug, Default, Deserialize)]
struct Tag {
	name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubTag {
	repo: String,
}

impl Tags for GithubTag {
	fn get_tags(&self) -> anyhow::Result<Vec<String>> {
		println!("-> get tags from github tags");
		let tags: Vec<Tag> = get(format!("https://api.github.com/repos/{}/tags", self.repo))
			.send()?
			.error_for_status()?
			.json()?;
		Ok(tags.into_iter().map(|f| f.name).collect())
	}
}
