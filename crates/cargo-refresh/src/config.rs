use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::cli::RunArgs;

#[derive(Debug, Clone, Deserialize)]
pub struct Metadata {
    pub packages: Vec<Package>,
    pub workspace_root: PathBuf,
    pub target_directory: PathBuf,
    #[serde(default)]
    pub workspace_default_members: Vec<String>,
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
    pub strict: bool,
}

impl Default for RefreshMetadata {
    fn default() -> Self {
        Self {
            edit: Vec::new(),
            allow_tracing_metadata: true,
            after_patch_hook: false,
            strict: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Config {
    pub package: String,
    pub bin: String,
    pub edit: Vec<String>,
    pub allow_tracing_metadata: bool,
    pub after_patch_hook: bool,
    pub strict: bool,
    pub check: bool,
    pub events: PathBuf,
    pub verbose: u8,
    #[serde(serialize_with = "serialize_args")]
    pub args: Vec<OsString>,
    pub workspace_root: PathBuf,
    pub target_directory: PathBuf,
    pub source_roots: Vec<PathBuf>,
    pub input_files: Vec<PathBuf>,
}

fn serialize_args<S: serde::Serializer>(
    args: &[OsString],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.collect_seq(args.iter().map(|arg| arg.to_string_lossy()))
}

impl Config {
    pub fn load(args: RunArgs) -> Result<Self> {
        let cwd = std::env::current_dir().context("get current directory")?;
        let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["metadata", "--format-version", "1"])
            .output()
            .context("run cargo metadata")?;
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
        let edit = args.edit.unwrap_or(settings.edit);
        for name in &edit {
            if !local.iter().any(|p| &p.name == name) {
                bail!("editable crate {name} is not a local package in cargo metadata");
            }
        }
        let mut source_roots = BTreeSet::new();
        let mut input_files = BTreeSet::from([
            metadata.workspace_root.join("Cargo.toml"),
            metadata.workspace_root.join("Cargo.lock"),
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
        Ok(Self {
            package: package.name.clone(),
            bin,
            edit,
            allow_tracing_metadata: settings.allow_tracing_metadata,
            after_patch_hook: settings.after_patch_hook,
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
