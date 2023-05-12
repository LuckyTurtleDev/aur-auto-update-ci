use crate::Tags;
use attohttpc::get;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
struct Versions {
	versions: Vec<Release>,
}

#[derive(Debug, Default, Deserialize)]
struct Release {
	num: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CratesIoRelease {
	#[serde(rename = "crate")]
	krate: String,
}

impl Tags for CratesIoRelease {
	fn get_tags(&self) -> anyhow::Result<Vec<String>> {
		println!("-> get tags from crates.io");
		let versions: Versions = get(format!("https://crates.io/api/v1/crates/{}/versions", self.krate))
			.send()?
			.error_for_status()?
			.json()?;
		Ok(versions.versions.into_iter().map(|f| f.num).collect())
	}
}
