use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::cli::{Features, RunArgs};

#[derive(Debug, Clone, Deserialize)]
pub struct Metadata {
    pub packages: Vec<Package>,
    pub resolve: Option<Resolve>,
    pub workspace_root: PathBuf,
    pub target_directory: PathBuf,
    #[serde(default)]
    pub workspace_default_members: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Resolve {
    pub nodes: Vec<ResolveNode>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResolveNode {
    pub id: String,
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub manifest_path: PathBuf,
    pub source: Option<String>,
    pub targets: Vec<Target>,
    pub metadata: serde_json::Value,
    pub default_run: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Target {
    pub name: String,
    pub kind: Vec<String>,
    pub src_path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct RefreshMetadata {
    pub edit: Vec<String>,
    pub allow_tracing_metadata: bool,
    pub after_patch_hook: bool,
    pub after_patch_touch: Vec<PathBuf>,
    pub after_patch_http: Option<String>,
    pub strict: bool,
}

impl Default for RefreshMetadata {
    fn default() -> Self {
        Self {
            edit: Vec::new(),
            allow_tracing_metadata: true,
            after_patch_hook: false,
            after_patch_touch: Vec::new(),
            after_patch_http: None,
            strict: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Config {
    #[serde(skip)]
    pub support: Option<std::sync::Arc<crate::install::Support>>,
    #[serde(skip)]
    pub invocation: RunArgs,
    pub memory_estimate: crate::memory::Estimate,
    pub package: String,
    pub bin: String,
    pub features: Features,
    pub edit: Vec<String>,
    pub allow_tracing_metadata: bool,
    pub after_patch_hook: bool,
    pub after_patch_touch: Vec<PathBuf>,
    pub after_patch_http: Option<String>,
    pub strict: bool,
    pub check: bool,
    pub events: PathBuf,
    pub verbose: u8,
    #[serde(serialize_with = "serialize_args")]
    pub args: Vec<OsString>,
    pub workspace_root: PathBuf,
    pub target_directory: PathBuf,
    pub source_roots: Vec<PathBuf>,
    pub frontier_roots: Vec<PathBuf>,
    /// Conventional excluded targets, except paths captured as application inputs.
    pub excluded_target_roots: Vec<PathBuf>,
    pub captured_rust_inputs: BTreeSet<PathBuf>,
    pub opaque_build_roots: Vec<PathBuf>,
    /// Edited library crates with transitive callers outside compiler ownership.
    /// Without an instantiation/inlining proof, even a restart may be stale.
    pub stock_frontier_roots: Vec<PathBuf>,
    pub package_roots: Vec<PathBuf>,
    pub input_files: Vec<PathBuf>,
}

fn serialize_args<S: serde::Serializer>(
    args: &[OsString],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.collect_seq(args.iter().map(|arg| arg.to_string_lossy()))
}

fn normal_roots(package: &Package) -> Vec<PathBuf> {
    package
        .targets
        .iter()
        .filter(|t| {
            t.kind.iter().any(|k| {
                matches!(
                    k.as_str(),
                    "bin" | "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"
                )
            })
        })
        .filter_map(|t| t.src_path.parent().map(Path::to_path_buf))
        .collect()
}

impl Config {
    pub fn excluded_target_only(&self, changes: &crate::changes::ChangeSet) -> bool {
        !changes.changes.is_empty()
            && changes.changes.iter().all(|c| {
                c.kind == crate::changes::PathKind::Rs
                    && self
                        .excluded_target_roots
                        .iter()
                        .any(|root| c.path.starts_with(root))
                    && !self.captured_rust_inputs.contains(&c.path)
                    && !self
                        .opaque_build_roots
                        .iter()
                        .any(|root| c.path.starts_with(root))
                    && !self
                        .input_files
                        .iter()
                        .any(|input| c.path.starts_with(input))
            })
    }

    pub fn load(args: RunArgs) -> Result<Self> {
        let cwd = std::env::current_dir().context("get current directory")?;
        let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        command
            .args(["metadata", "--format-version", "1"])
            .args(args.features.cargo_args())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let output = crate::process::spawn(&mut command).context("run cargo metadata")?;
        let output = output.wait_with_output()?;
        if !output.status.success() {
            bail!(
                "cargo metadata failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let metadata: Metadata =
            serde_json::from_slice(&output.stdout).context("parse cargo metadata")?;
        Self::resolve(args, &metadata, &cwd)
    }

    pub fn resolve(args: RunArgs, metadata: &Metadata, cwd: &Path) -> Result<Self> {
        Self::resolve_with_memory(
            args,
            metadata,
            cwd,
            crate::memory::sample().ok().map(|s| s.available),
        )
    }

    fn resolve_with_memory(
        args: RunArgs,
        metadata: &Metadata,
        cwd: &Path,
        available: Option<u64>,
    ) -> Result<Self> {
        let invocation = args.clone();
        let local: Vec<_> = metadata
            .packages
            .iter()
            .filter(|p| p.source.is_none())
            .collect();
        let package = if let Some(name) = &args.package {
            local
                .iter()
                .copied()
                .find(|p| &p.name == name)
                .with_context(|| format!("no local package named {name}"))?
        } else {
            let nearest = cwd
                .ancestors()
                .map(|p| p.join("Cargo.toml"))
                .find(|p| p.is_file());
            let current = nearest.as_ref().and_then(|manifest| {
                let manifest = manifest.canonicalize().unwrap_or_else(|_| manifest.clone());
                local.iter().copied().find(|p| p.manifest_path == manifest)
            });
            if let Some(current) = current {
                current
            } else if metadata.workspace_default_members.len() == 1 {
                local
                    .iter()
                    .copied()
                    .find(|p| p.id == metadata.workspace_default_members[0])
                    .context("default workspace package missing from metadata")?
            } else if local.len() == 1 {
                local[0]
            } else {
                bail!("workspace has multiple default packages; select one with --package");
            }
        };
        let settings: RefreshMetadata = package
            .metadata
            .get("refresh")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .context("parse [package.metadata.refresh]")?
            .unwrap_or_default();
        if let Some(url) = &settings.after_patch_http {
            crate::triggers::Http::parse(url)?;
        }
        let package_root = package
            .manifest_path
            .parent()
            .context("manifest has no parent")?;
        let after_patch_touch = settings
            .after_patch_touch
            .iter()
            .map(|path| {
                if path.is_absolute() {
                    path.clone()
                } else {
                    package_root.join(path)
                }
            })
            .collect();
        let bins: Vec<_> = package
            .targets
            .iter()
            .filter(|t| t.kind.iter().any(|k| k == "bin"))
            .collect();
        let bin = if let Some(name) = args.bin.as_ref().or(package.default_run.as_ref()) {
            bins.iter()
                .find(|t| &t.name == name)
                .with_context(|| format!("package {} has no binary named {name}", package.name))?
                .name
                .clone()
        } else if bins.len() == 1 {
            bins[0].name.clone()
        } else {
            bail!(
                "package {} has {} binaries; select one with --bin",
                package.name,
                bins.len()
            );
        };
        let automatic = args.edit.is_none() && settings.edit.is_empty();
        let mut edit = args.edit.clone().unwrap_or(settings.edit);
        for name in &edit {
            if !local.iter().any(|p| &p.name == name) {
                bail!("editable crate {name} is not a local package in cargo metadata");
            }
        }
        let mut linked = BTreeSet::from([package.id.as_str()]);
        if let Some(resolve) = &metadata.resolve {
            loop {
                let count = linked.len();
                for node in &resolve.nodes {
                    if linked.contains(node.id.as_str()) {
                        linked.extend(node.dependencies.iter().map(String::as_str));
                    }
                }
                if linked.len() == count {
                    break;
                }
            }
        }
        let debug_off =
            crate::memory::debug_info_disabled(&metadata.workspace_root.join("Cargo.toml"))?;
        let estimate_for = |edit: &[String]| -> Result<crate::memory::Estimate> {
            let roots: Vec<_> = local
                .iter()
                .filter(|p| p.name == package.name || edit.contains(&p.name))
                .flat_map(|p| normal_roots(p))
                .collect();
            let owned_count = local
                .iter()
                .filter(|p| {
                    linked.contains(p.id.as_str())
                        && (p.name == package.name || edit.contains(&p.name))
                })
                .count();
            let estimate =
                crate::memory::Estimate::project(&roots, linked.len().saturating_sub(owned_count))?;
            Ok(if debug_off {
                estimate.without_debug_info()
            } else {
                estimate
            })
        };
        if automatic && let Some(available) = available {
            // Add callers before their dependencies; otherwise inlined/generic
            // code can remain in a precompiled caller outside the frontier.
            loop {
                let mut added = false;
                for candidate in &local {
                    if candidate.name == package.name
                        || edit.contains(&candidate.name)
                        || !linked.contains(candidate.id.as_str())
                        || !candidate
                            .targets
                            .iter()
                            .any(|t| t.kind.iter().any(|k| matches!(k.as_str(), "lib" | "rlib")))
                    {
                        continue;
                    }
                    let Some(resolve) = &metadata.resolve else {
                        continue;
                    };
                    if resolve.nodes.iter().any(|node| {
                        linked.contains(node.id.as_str())
                            && node.dependencies.contains(&candidate.id)
                            && !local.iter().any(|p| {
                                p.id == node.id
                                    && (p.name == package.name || edit.contains(&p.name))
                            })
                    }) {
                        continue;
                    }
                    let mut expanded = edit.clone();
                    expanded.push(candidate.name.clone());
                    if crate::memory::allows(available, estimate_for(&expanded)?.peak_bytes) {
                        edit = expanded;
                        added = true;
                    }
                }
                if !added {
                    break;
                }
            }
        }
        let owned: BTreeSet<_> = local
            .iter()
            .filter(|p| p.name == package.name || edit.contains(&p.name))
            .map(|p| p.id.as_str())
            .collect();
        let frontier_roots = local
            .iter()
            .filter(|p| owned.contains(p.id.as_str()))
            .flat_map(|p| normal_roots(p))
            .collect();
        let memory_estimate = estimate_for(&edit)?;
        let normal_roots: Vec<_> = local.iter().flat_map(|p| normal_roots(p)).collect();
        let excluded_target_roots = local
            .iter()
            .flat_map(|p| {
                let root = p.manifest_path.parent().unwrap();
                [
                    root.join("tests"),
                    root.join("benches"),
                    root.join("examples"),
                ]
            })
            .filter(|root| !normal_roots.iter().any(|normal| root.starts_with(normal)))
            .collect();
        let stock_frontier_roots = local
            .iter()
            .filter(|p| owned.contains(p.id.as_str()))
            .filter(|p| {
                let Some(resolve) = &metadata.resolve else {
                    // Older/incomplete metadata cannot prove the dependency closure.
                    return local.iter().any(|other| !owned.contains(other.id.as_str()));
                };
                let mut callers = BTreeSet::from([p.id.as_str()]);
                loop {
                    let count = callers.len();
                    for node in &resolve.nodes {
                        if node
                            .dependencies
                            .iter()
                            .any(|dep| callers.contains(dep.as_str()))
                        {
                            callers.insert(node.id.as_str());
                        }
                    }
                    if callers.len() == count {
                        break;
                    }
                }
                callers
                    .iter()
                    .any(|id| linked.contains(id) && !owned.contains(id))
            })
            .filter_map(|p| p.manifest_path.parent().map(Path::to_path_buf))
            .collect();
        let opaque_build_roots = local
            .iter()
            .filter(|p| {
                let features = metadata
                    .resolve
                    .as_ref()
                    .and_then(|r| r.nodes.iter().find(|n| n.id == p.id))
                    .map(|n| n.features.as_slice());
                p.targets
                    .iter()
                    .filter(|t| t.kind.iter().any(|k| k == "custom-build"))
                    .any(|t| {
                        let Ok(text) = std::fs::read_to_string(&t.src_path) else {
                            return true;
                        };
                        features.is_none_or(|features| {
                            !refresh_guard::empty_build_script(&text, features)
                        })
                    })
            })
            .filter_map(|p| p.manifest_path.parent().map(Path::to_path_buf))
            .collect();
        let mut source_roots = BTreeSet::new();
        let mut input_files = BTreeSet::from([
            metadata.workspace_root.join("Cargo.toml"),
            metadata.workspace_root.join("Cargo.lock"),
            metadata.workspace_root.join("rust-toolchain"),
            metadata.workspace_root.join("rust-toolchain.toml"),
        ]);
        for p in &local {
            let root = p.manifest_path.parent().context("manifest has no parent")?;
            input_files.extend([p.manifest_path.clone(), root.join("build.rs")]);
            for target in &p.targets {
                if target.kind.iter().any(|kind| kind == "custom-build") {
                    input_files.insert(target.src_path.clone());
                } else if let Some(parent) = target.src_path.parent() {
                    source_roots.insert(parent.to_path_buf());
                }
            }
            // Cargo searches config from the invocation directory through its ancestors.
            // Include package ancestry too, for runs from within a workspace member.
            for ancestor in root.ancestors() {
                input_files.extend([
                    ancestor.join(".cargo/config.toml"),
                    ancestor.join(".cargo/config"),
                ]);
            }
        }
        for ancestor in cwd.ancestors() {
            input_files.extend([
                ancestor.join(".cargo/config.toml"),
                ancestor.join(".cargo/config"),
            ]);
        }
        if let Some(home) = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cargo")))
        {
            input_files.extend([home.join("config"), home.join("config.toml")]);
        }
        Ok(Self {
            memory_estimate,
            support: None,
            invocation,
            package: package.name.clone(),
            bin,
            features: args.features,
            edit,
            allow_tracing_metadata: settings.allow_tracing_metadata,
            after_patch_hook: settings.after_patch_hook,
            after_patch_touch,
            after_patch_http: settings.after_patch_http,
            strict: args.strict || settings.strict,
            check: !args.no_check,
            events: args
                .events
                .map(|p| if p.is_absolute() { p } else { cwd.join(p) })
                .unwrap_or_else(|| metadata.target_directory.join("refresh/events.jsonl")),
            verbose: args.verbose,
            args: args.args,
            workspace_root: metadata.workspace_root.clone(),
            target_directory: metadata.target_directory.clone(),
            source_roots: source_roots.into_iter().collect(),
            frontier_roots,
            excluded_target_roots,
            captured_rust_inputs: BTreeSet::new(),
            opaque_build_roots,
            stock_frontier_roots,
            package_roots: local
                .iter()
                .filter_map(|p| p.manifest_path.parent().map(Path::to_path_buf))
                .collect(),
            input_files: input_files.into_iter().collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metadata(refresh: serde_json::Value) -> Metadata {
        serde_json::from_value(json!({
            "packages": [{"id": "app-id", "name": "app", "manifest_path": "/demo/Cargo.toml",
                "source": null, "targets": [
                    {"name": "app", "kind": ["bin"], "src_path": "/demo/src/main.rs"},
                    {"name": "build-script-build", "kind": ["custom-build"], "src_path": "/demo/build.rs"}
                ], "metadata": {"refresh": refresh}, "default_run": null}],
            "workspace_root": "/demo", "target_directory": "/demo/target", "workspace_default_members": ["app-id"]
        })).unwrap()
    }

    #[test]
    fn defaults_and_watch_roots() {
        let config =
            Config::resolve(RunArgs::default(), &metadata(json!({})), Path::new("/demo")).unwrap();
        assert!(config.allow_tracing_metadata && config.check);
        assert!(!config.after_patch_hook && !config.strict);
        assert!(config.edit.is_empty());
        assert_eq!(config.bin, "app");
        assert_eq!(config.source_roots, [PathBuf::from("/demo/src")]);
        for path in ["Cargo.toml", "Cargo.lock", "build.rs", ".cargo/config.toml"] {
            assert!(config.input_files.contains(&Path::new("/demo").join(path)));
        }
        assert_eq!(
            config.events,
            PathBuf::from("/demo/target/refresh/events.jsonl")
        );
    }

    #[test]
    fn excluded_transitive_caller_requires_stock_even_for_owned_source() {
        let mut meta = metadata(json!({}));
        let mut library = meta.packages[0].clone();
        library.id = "lib-id".into();
        library.name = "lib".into();
        library.manifest_path = "/demo/lib/Cargo.toml".into();
        library.targets[0].src_path = "/demo/lib/src/lib.rs".into();
        let mut caller = library.clone();
        caller.id = "caller-id".into();
        caller.name = "caller".into();
        caller.manifest_path = "/demo/caller/Cargo.toml".into();
        caller.targets[0].src_path = "/demo/caller/src/lib.rs".into();
        meta.packages.extend([library, caller]);
        meta.resolve = Some(
            serde_json::from_value(json!({"nodes": [
                {"id":"app-id","dependencies":["caller-id","lib-id"]},
                {"id":"caller-id","dependencies":["lib-id"]},
                {"id":"lib-id","dependencies":[]}
            ]}))
            .unwrap(),
        );
        let mut args = RunArgs {
            edit: Some(vec!["lib".into()]),
            ..Default::default()
        };
        let config = Config::resolve(args.clone(), &meta, Path::new("/demo")).unwrap();
        assert_eq!(config.stock_frontier_roots, [PathBuf::from("/demo/lib")]);
        args.edit.as_mut().unwrap().push("caller".into());
        // An unrelated workspace package cannot retain code in this executable.
        let mut unrelated = meta.clone();
        unrelated.resolve.as_mut().unwrap().nodes[0].dependencies = vec!["lib-id".into()];
        assert!(
            Config::resolve(
                RunArgs {
                    edit: Some(vec!["lib".into()]),
                    ..Default::default()
                },
                &unrelated,
                Path::new("/demo")
            )
            .unwrap()
            .stock_frontier_roots
            .is_empty()
        );
        assert!(
            Config::resolve(args, &meta, Path::new("/demo"))
                .unwrap()
                .stock_frontier_roots
                .is_empty()
        );
    }

    #[test]
    fn automatic_frontier_uses_memory_budget_and_keeps_callers_owned() {
        let mut meta = metadata(json!({}));
        let mut lib = meta.packages[0].clone();
        lib.id = "lib-id".into();
        lib.name = "lib".into();
        lib.manifest_path = "/demo/lib/Cargo.toml".into();
        lib.targets = vec![Target {
            name: "lib".into(),
            kind: vec!["lib".into()],
            src_path: "/demo/lib/src/lib.rs".into(),
        }];
        let mut caller = lib.clone();
        caller.id = "caller-id".into();
        caller.name = "caller".into();
        caller.manifest_path = "/demo/caller/Cargo.toml".into();
        caller.targets[0].src_path = "/demo/caller/src/lib.rs".into();
        meta.packages.extend([lib, caller]);
        meta.resolve = Some(
            serde_json::from_value(json!({"nodes": [
                {"id":"app-id","dependencies":["caller-id"]},
                {"id":"caller-id","dependencies":["lib-id"]},
                {"id":"lib-id","dependencies":[]}
            ]}))
            .unwrap(),
        );
        let load = |memory| {
            Config::resolve_with_memory(Default::default(), &meta, Path::new("/demo"), memory)
                .unwrap()
        };
        let roomy = load(Some(20 * crate::memory::GIB));
        assert_eq!(roomy.edit, ["caller", "lib"]);
        assert!(roomy.stock_frontier_roots.is_empty());
        assert!(load(Some(crate::memory::GIB)).edit.is_empty());
        assert!(load(None).edit.is_empty());
        let explicit = Config::resolve_with_memory(
            RunArgs {
                edit: Some(vec![]),
                ..Default::default()
            },
            &meta,
            Path::new("/demo"),
            Some(20 * crate::memory::GIB),
        )
        .unwrap();
        assert!(explicit.edit.is_empty());
    }

    #[test]
    fn excluded_targets_skip_only_when_not_application_or_build_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut meta = metadata(json!({}));
        meta.workspace_root = root.into();
        meta.target_directory = root.join("target");
        meta.packages[0].manifest_path = root.join("Cargo.toml");
        meta.packages[0].targets[0].src_path = root.join("src/main.rs");
        meta.packages[0]
            .targets
            .retain(|t| !t.kind.iter().any(|k| k == "custom-build"));
        let mut config = Config::resolve(Default::default(), &meta, root).unwrap();
        for folder in ["tests", "benches", "examples"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
            let file = root.join(folder).join("case.rs");
            std::fs::write(&file, "before").unwrap();
            let mut tracker =
                crate::changes::ChangeTracker::new([file.clone()], Default::default()).unwrap();
            std::fs::write(&file, "after").unwrap();
            let changes = tracker.observe([file.clone()]).unwrap();
            assert!(config.excluded_target_only(&changes));
            config.captured_rust_inputs.insert(file.clone());
            assert!(!config.excluded_target_only(&changes));
            config.captured_rust_inputs.remove(&file);
            config.input_files.push(root.join(folder));
            assert!(!config.excluded_target_only(&changes));
            config.input_files.pop();
        }
        // A binary or library whose source root is tests/ keeps that directory active.
        meta.packages[0].targets[0].src_path = root.join("tests/main.rs");
        let config = Config::resolve(Default::default(), &meta, root).unwrap();
        assert!(!config.excluded_target_roots.contains(&root.join("tests")));
    }

    #[test]
    fn metadata_and_cli_precedence() {
        let meta = metadata(json!({"edit": ["app"], "strict": true,
            "allow_tracing_metadata": false, "after_patch_hook": true}));
        let args = RunArgs {
            edit: Some(vec![]),
            no_check: true,
            events: Some("out.jsonl".into()),
            verbose: 1,
            ..Default::default()
        };
        let config = Config::resolve(args, &meta, Path::new("/demo")).unwrap();
        assert!(config.edit.is_empty());
        assert!(config.strict && config.after_patch_hook);
        assert!(!config.check && !config.allow_tracing_metadata);
        assert_eq!(config.events, PathBuf::from("/demo/out.jsonl"));
        let config = Config::resolve(
            RunArgs {
                strict: true,
                ..Default::default()
            },
            &metadata(json!({})),
            Path::new("/demo"),
        )
        .unwrap();
        assert!(config.strict);
    }

    #[test]
    fn metadata_edit_is_used_without_cli() {
        let config = Config::resolve(
            RunArgs::default(),
            &metadata(json!({"edit": ["app"]})),
            Path::new("/demo"),
        )
        .unwrap();
        assert_eq!(config.edit, ["app"]);
    }

    #[test]
    fn invalid_config_and_missing_selections_fail() {
        for (args, refresh) in [
            (RunArgs::default(), json!({"strict": "yes"})),
            (
                RunArgs {
                    package: Some("missing".into()),
                    ..Default::default()
                },
                json!({}),
            ),
            (
                RunArgs {
                    bin: Some("missing".into()),
                    ..Default::default()
                },
                json!({}),
            ),
            (
                RunArgs {
                    edit: Some(vec!["missing".into()]),
                    ..Default::default()
                },
                json!({}),
            ),
        ] {
            assert!(Config::resolve(args, &metadata(refresh), Path::new("/demo")).is_err());
        }
    }

    #[test]
    fn external_triggers_resolve_from_selected_package() {
        let config = Config::resolve(
            RunArgs::default(),
            &metadata(json!({"after_patch_hook": true,
                "after_patch_touch": ["book/src/chapter.md", "/tmp/site/config.toml"],
                "after_patch_http": "http://127.0.0.1:8080/rebuild"})),
            Path::new("/demo/src"),
        )
        .unwrap();
        assert_eq!(
            config.after_patch_touch,
            [
                PathBuf::from("/demo/book/src/chapter.md"),
                PathBuf::from("/tmp/site/config.toml")
            ]
        );
        assert_eq!(
            config.after_patch_http.as_deref(),
            Some("http://127.0.0.1:8080/rebuild")
        );
        assert!(config.after_patch_hook);
        assert!(
            Config::resolve(
                RunArgs::default(),
                &metadata(json!({"after_patch_http": "https://host/"})),
                Path::new("/demo")
            )
            .is_err()
        );
    }

    #[test]
    fn binary_default_run_and_ambiguity() {
        let mut meta = metadata(json!({}));
        meta.packages[0].targets.push(Target {
            name: "second".into(),
            kind: vec!["bin".into()],
            src_path: "/demo/src/bin/second.rs".into(),
        });
        assert!(Config::resolve(RunArgs::default(), &meta, Path::new("/demo")).is_err());
        meta.packages[0].default_run = Some("second".into());
        assert_eq!(
            Config::resolve(RunArgs::default(), &meta, Path::new("/demo"))
                .unwrap()
                .bin,
            "second"
        );
        let args = RunArgs {
            bin: Some("app".into()),
            ..Default::default()
        };
        assert_eq!(
            Config::resolve(args, &meta, Path::new("/demo"))
                .unwrap()
                .bin,
            "app"
        );
    }
}
