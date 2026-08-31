//! Reading the workspace. `cargo metadata` supplies the graph; the manifests supply
//! the lint declarations, which metadata does not carry.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub(crate) struct Member {
    pub(crate) name: String,
    /// The directory under `crates/`. `None` for tooling, which no layer rule reaches.
    pub(crate) layer: Option<String>,
    pub(crate) root: PathBuf,
    /// Declares `unsafe_code = "allow"` rather than taking the workspace lint set.
    pub(crate) allows_unsafe: bool,
    deps: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct Workspace {
    pub(crate) members: Vec<Member>,
    by_name: BTreeMap<String, usize>,
}

impl Workspace {
    /// The workspace-internal dependencies of a member, in manifest order.
    pub(crate) fn internal_deps(&self, m: &Member) -> Vec<&Member> {
        m.deps
            .iter()
            .filter_map(|d| self.by_name.get(d))
            .map(|i| &self.members[*i])
            .collect()
    }

    pub(crate) fn crates_in(&self, layers: &[&str]) -> Vec<&Member> {
        self.members
            .iter()
            .filter(|m| m.layer.as_deref().is_some_and(|l| layers.contains(&l)))
            .collect()
    }
}

impl Member {
    /// Build a member without going through cargo, so the rules can be tested against
    /// graphs nobody has to create on disk first.
    #[cfg(test)]
    pub(crate) fn synthetic(name: &str, layer: Option<&str>, deps: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            layer: layer.map(str::to_owned),
            root: PathBuf::from("/synthetic"),
            allows_unsafe: false,
            deps: deps.iter().map(|d| (*d).to_owned()).collect(),
        }
    }

    #[cfg(test)]
    pub(crate) fn allowing_unsafe(mut self) -> Self {
        self.allows_unsafe = true;
        self
    }
}

impl Workspace {
    #[cfg(test)]
    pub(crate) fn of(members: Vec<Member>) -> Self {
        let by_name = members
            .iter()
            .enumerate()
            .map(|(i, m)| (m.name.clone(), i))
            .collect();
        Self { members, by_name }
    }
}

pub(crate) fn workspace() -> Result<Workspace, String> {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .map_err(|e| format!("could not run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("cargo metadata is not JSON: {e}"))?;

    let mut members = Vec::new();
    for pkg in v["packages"].as_array().into_iter().flatten() {
        let name = pkg["name"].as_str().unwrap_or_default().to_owned();
        let manifest = PathBuf::from(pkg["manifest_path"].as_str().unwrap_or_default());
        let root = manifest.parent().unwrap_or(Path::new(".")).to_path_buf();
        let deps = pkg["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| d["name"].as_str())
            .map(str::to_owned)
            .collect();
        let allows_unsafe = std::fs::read_to_string(&manifest)
            .map(|s| declares_unsafe_allow(&s))
            .unwrap_or(false);
        members.push(Member {
            layer: layer_of(&root),
            name,
            root,
            allows_unsafe,
            deps,
        });
    }
    members.sort_by(|a, b| a.name.cmp(&b.name));
    let by_name = members
        .iter()
        .enumerate()
        .map(|(i, m)| (m.name.clone(), i))
        .collect();
    Ok(Workspace { members, by_name })
}

/// A crate's layer is the directory that holds it: `crates/<layer>/<name>`.
fn layer_of(root: &Path) -> Option<String> {
    let parent = root.parent()?;
    let layer = parent.file_name()?.to_str()?;
    if parent.parent()?.file_name()?.to_str()? == "crates" {
        Some(layer.to_owned())
    } else {
        None
    }
}

/// A deliberately narrow read of the manifest: does it opt out of the workspace's
/// `unsafe_code = "forbid"`? Anything subtler than this should not be being done.
fn declares_unsafe_allow(manifest: &str) -> bool {
    manifest.lines().any(|l| {
        let l = l.split('#').next().unwrap_or("").replace(' ', "");
        l.starts_with("unsafe_code=") && l.contains("\"allow\"")
    })
}
