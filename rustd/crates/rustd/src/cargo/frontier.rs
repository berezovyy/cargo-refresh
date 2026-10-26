//! EXP-110: select runtime workspace roots before the first worker is started.
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use cargo_metadata::{DependencyKind, Metadata, Package, PackageId, Target};
use serde::{Deserialize, Serialize};

/// D7 may change this single product default.
pub(crate) const DEFAULT_FRONTIER: Frontier = Frontier::Primary;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Frontier {
    Primary,
    Closure,
}

impl Default for Frontier {
    fn default() -> Self {
        DEFAULT_FRONTIER
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Options {
    pub(crate) frontier: Frontier,
    pub(crate) edit: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct OwnedCrate {
    pub(crate) package: String,
    pub(crate) name: String,
    pub(crate) root: PathBuf,
}

pub(crate) struct Selection {
    pub(crate) options: Options,
    pub(crate) allowed: Vec<OwnedCrate>,
}

fn root(target: &Target) -> PathBuf {
    let path = target.src_path.as_std_path();
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

impl Selection {
    /// Metadata normal edges exclude dev/build dependencies. Proc macros terminate traversal;
    /// captured link inputs below remain the upper bound (including target-specific edges).
    pub(crate) fn of(
        metadata: &Metadata,
        primary: &Package,
        target: &Target,
        options: &Options,
    ) -> Result<Self, String> {
        let resolve = metadata.resolve.as_ref().ok_or("cargo metadata has no resolved graph")?;
        let packages = &metadata.packages;
        let mut reached = BTreeSet::<PackageId>::new();
        let mut todo = vec![primary.id.clone()];
        while let Some(id) = todo.pop() {
            if reached.contains(&id) {
                continue;
            }
            let package =
                packages.iter().find(|p| p.id == id).ok_or("resolved package is missing")?;
            if package.targets.iter().any(Target::is_proc_macro) {
                continue;
            }
            reached.insert(id.clone());
            let node =
                resolve.nodes.iter().find(|n| n.id == id).ok_or("resolved node is missing")?;
            todo.extend(
                node.deps
                    .iter()
                    .filter(|d| d.dep_kinds.iter().any(|k| k.kind == DependencyKind::Normal))
                    .map(|d| d.pkg.clone()),
            );
        }
        let members: BTreeSet<_> = metadata.workspace_members.iter().collect();
        let mut owned = BTreeSet::from([primary.id.clone()]);
        if options.frontier == Frontier::Closure {
            owned.extend(reached.iter().cloned());
        }
        for name in &options.edit {
            let normalized = name.replace('-', "_");
            let matches: Vec<_> = packages
                .iter()
                .filter(|p| {
                    members.contains(&p.id)
                        && (p.name.as_str() == name
                            || p.targets.iter().any(|t| {
                                (t.is_lib() || t.is_rlib())
                                    && t.name.replace('-', "_") == normalized
                            }))
                })
                .collect();
            let [package] = matches.as_slice() else {
                return Err(format!(
                    "--edit {name}: expected one workspace package or library crate"
                ));
            };
            if !reached.contains(&package.id) {
                return Err(format!(
                    "--edit {name}: out-of-frontier (no normal runtime dependency path)"
                ));
            }
            owned.insert(package.id.clone());
        }
        let mut allowed = vec![OwnedCrate {
            package: primary.name.to_string(),
            name: target.name.clone(),
            root: root(target),
        }];
        for package in packages.iter().filter(|p| owned.contains(&p.id) && members.contains(&p.id))
        {
            for target in package.targets.iter().filter(|t| t.is_lib() || t.is_rlib()) {
                let path = root(target);
                if !allowed.iter().any(|c| c.root == path) {
                    allowed.push(OwnedCrate {
                        package: package.name.to_string(),
                        name: target.name.clone(),
                        root: path,
                    });
                }
            }
        }
        Ok(Self { options: options.clone(), allowed })
    }

    pub(crate) fn owns(&self, source: &Path) -> bool {
        self.allowed.iter().any(|c| c.root == source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metadata() -> Metadata {
        let mut packages = Vec::new();
        let mut nodes = Vec::new();
        for name in ["app", "normal", "dev", "build", "macro", "macro_dep", "unlinked"] {
            let target = |kind: &str, file: &str| {
                json!({
                    "kind": [kind], "crate_types": [kind], "name": name,
                    "src_path": format!("/ws/{name}/{file}"), "edition": "2021",
                    "doctest": true, "test": true, "doc": true
                })
            };
            let mut targets =
                vec![target(if name == "macro" { "proc-macro" } else { "lib" }, "lib.rs")];
            if name == "app" {
                targets.push(target("bin", "main.rs"));
            }
            packages.push(json!({
                "name": name, "version": "0.1.0", "id": name,
                "source": null, "dependencies": [], "targets": targets,
                "features": {}, "manifest_path": format!("/ws/{name}/Cargo.toml")
            }));
            let dep = |pkg: &str, kind: Option<&str>| {
                json!({
                    "name": pkg, "pkg": pkg, "dep_kinds": [{"kind": kind, "target": null}]
                })
            };
            let deps = match name {
                "app" => vec![
                    dep("normal", None),
                    dep("dev", Some("dev")),
                    dep("build", Some("build")),
                    dep("macro", None),
                ],
                "macro" => vec![dep("macro_dep", None)],
                _ => vec![],
            };
            nodes.push(json!({"id": name, "deps": deps, "dependencies": [], "features": []}));
        }
        serde_json::from_value(json!({
            "packages": packages,
            "workspace_members": ["app", "normal", "dev", "build", "macro", "macro_dep", "unlinked"],
            "workspace_default_members": ["app"],
            "workspace_root": "/ws", "target_directory": "/ws/target", "version": 1,
            "resolve": {"nodes": nodes, "root": "app"}
        })).unwrap()
    }

    #[test]
    fn runtime_closure_primary_and_explicit_edits() {
        let m = metadata();
        let p = &m.packages[0];
        let t = &p.targets[1];
        let select = |options| Selection::of(&m, p, t, &options).unwrap();
        let names = |s: Selection| s.allowed.into_iter().map(|c| c.root).collect::<Vec<_>>();
        assert_eq!(
            names(select(Options::default())),
            ["/ws/app/main.rs", "/ws/app/lib.rs"].map(PathBuf::from)
        );
        let closure = names(select(Options { frontier: Frontier::Closure, edit: vec![] }));
        assert_eq!(closure.len(), 3);
        assert!(closure.contains(&PathBuf::from("/ws/normal/lib.rs")));
        assert_eq!(
            names(select(Options { frontier: Frontier::Primary, edit: vec!["normal".into()] })),
            closure
        );
        for excluded in ["dev", "build", "macro", "macro_dep", "unlinked", "missing"] {
            assert!(
                Selection::of(
                    &m,
                    p,
                    t,
                    &Options { frontier: Frontier::Primary, edit: vec![excluded.into()] }
                )
                .is_err()
            );
        }
    }
}
