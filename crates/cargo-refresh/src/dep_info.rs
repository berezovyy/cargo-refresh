//! rustc make-style dependency inputs. Read at capture/rebase, never per body save.
use crate::config::Config;
use anyhow::Result;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

/// rustc escapes spaces and backslashes and may continue a make rule on the next line.
/// Ignore targets, phony rules and `# env-dep:` comments.
fn inputs(text: &str) -> BTreeSet<PathBuf> {
    let text = text.replace("\\\r\n", "").replace("\\\n", "");
    let mut paths = BTreeSet::new();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let Some((_, deps)) = line.split_once(": ") else {
            continue;
        };
        let mut word = String::new();
        let mut chars = deps.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\\' if chars
                    .peek()
                    .is_some_and(|c| c.is_whitespace() || *c == '\\' || *c == '#') =>
                {
                    word.push(chars.next().unwrap())
                }
                ch if ch.is_whitespace() => {
                    if !word.is_empty() {
                        paths.insert(PathBuf::from(std::mem::take(&mut word)));
                    }
                }
                _ => word.push(ch),
            }
        }
        if !word.is_empty() {
            paths.insert(word.into());
        }
    }
    paths
}

/// A changed include invocation can introduce an input absent from the old .d.
/// Recapture it through Cargo before claiming a restart/live save is delivered.
pub fn includes_changed(before: &[u8], after: &[u8]) -> bool {
    fn calls(bytes: &[u8]) -> Vec<Vec<String>> {
        let Ok(source) = std::str::from_utf8(bytes) else {
            return Vec::new();
        };
        let mut offset = 0;
        let tokens: Vec<_> = rustc_lexer::tokenize(source)
            .filter_map(|token| {
                let text = &source[offset..offset + token.len];
                offset += token.len;
                (!matches!(
                    token.kind,
                    rustc_lexer::TokenKind::Whitespace
                        | rustc_lexer::TokenKind::LineComment
                        | rustc_lexer::TokenKind::BlockComment { .. }
                ))
                .then_some(text)
            })
            .collect();
        let mut result = Vec::new();
        for (i, token) in tokens.iter().enumerate() {
            if !matches!(
                token.trim_start_matches("r#"),
                "include" | "include_str" | "include_bytes"
            ) || tokens.get(i + 1) != Some(&"!")
            {
                continue;
            }
            let mut depth = 0;
            let mut call = Vec::new();
            for token in &tokens[i..] {
                call.push((*token).to_owned());
                match *token {
                    "(" | "[" | "{" => depth += 1,
                    ")" | "]" | "}" => {
                        depth -= 1;
                        if depth <= 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            result.push(call);
        }
        result
    }
    calls(before) != calls(after)
}

fn captured_depfile(record: &serde_json::Value) -> Option<(PathBuf, PathBuf)> {
    let cwd = PathBuf::from(record.get("cwd")?.as_str()?);
    let args: Vec<_> = record
        .get("args")?
        .as_array()?
        .iter()
        .filter_map(|a| a.as_str())
        .collect();
    let value = |flag| args.windows(2).find_map(|p| (p[0] == flag).then_some(p[1]));
    let out = cwd.join(value("--out-dir")?);
    let name = record.get("crate_name")?.as_str()?;
    let mut extra = "";
    for (i, arg) in args.iter().enumerate() {
        let flag = if *arg == "-C" {
            args.get(i + 1).copied()
        } else {
            arg.strip_prefix("-C")
        };
        if let Some(value) = flag.and_then(|v| v.strip_prefix("extra-filename=")) {
            extra = value;
        }
    }
    Some((out.join(format!("{name}{extra}.d")), cwd))
}

pub fn extend(config: &mut Config) -> Result<()> {
    let mut files: BTreeSet<_> = config.input_files.iter().cloned().collect();
    let mut depfiles = Vec::new();
    let mut working_dirs = BTreeMap::new();
    // The stock ladder uses its own target under refresh/. Checker targets
    // refer to private mirrors and must not become application watch inputs.
    for scan_root in [
        config.target_directory.clone(),
        config.target_directory.join("refresh/stock"),
    ] {
        if !scan_root.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&scan_root)
            .into_iter()
            .filter_entry(|e| {
                !e.file_type().is_dir()
                    || !matches!(
                        e.file_name().to_str(),
                        Some("refresh" | "incremental" | ".fingerprint")
                    )
            })
        {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.file_name() == "output"
                && entry
                    .path()
                    .parent()
                    .and_then(|p| p.parent())
                    .is_some_and(|p| p.file_name().is_some_and(|n| n == "build"))
            {
                for line in fs::read_to_string(entry.path())?.lines() {
                    if let Some(input) = line
                        .strip_prefix("cargo:rerun-if-changed=")
                        .or_else(|| line.strip_prefix("cargo::rerun-if-changed="))
                    {
                        let input = PathBuf::from(input);
                        if input.is_absolute() {
                            files.insert(input);
                        } else if let Some(roots) = entry
                            .path()
                            .parent()
                            .and_then(|p| p.file_name())
                            .and_then(|n| n.to_str())
                            .and_then(|n| n.rsplit_once('-').map(|(name, _)| name))
                            .and_then(|name| config.build_package_roots.get(name))
                        {
                            // Cargo names this directory <package-name>-<unit-hash>.
                            // Relative directives belong to that package, not every
                            // workspace root (typenum's `tests` is not the app's tests).
                            for root in roots {
                                let path = root.join(&input);
                                files.insert(path.canonicalize().unwrap_or(path));
                            }
                        }
                    }
                }
            }
            if entry.path().extension().is_some_and(|ext| ext == "d") {
                depfiles.push(entry.into_path());
            } else if entry.file_name() == "record.json"
                && entry
                    .path()
                    .ancestors()
                    .any(|p| p.file_name().is_some_and(|n| n == ".rustd-units"))
            {
                // Captured rustc records preserve the actual working directory of
                // workspace members. Relative .d paths are relative to that cwd.
                let record: serde_json::Value = serde_json::from_slice(&fs::read(entry.path())?)?;
                if let Some((depfile, cwd)) = captured_depfile(&record) {
                    working_dirs.insert(depfile, cwd);
                }
            }
        }
    }
    for depfile in depfiles {
        let cwd = working_dirs.get(&depfile).unwrap_or(&config.workspace_root);
        for path in inputs(&fs::read_to_string(&depfile)?) {
            let path = if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            };
            let canonical = path
                .canonicalize()
                .or_else(|error| {
                    let Some(parent) = path.parent() else {
                        return Err(error);
                    };
                    let Some(name) = path.file_name() else {
                        return Err(error);
                    };
                    Ok(parent.canonicalize()?.join(name))
                })
                .unwrap_or_else(|_| path.clone());
            // Preserve the spelling for symlink replacement, and the resolved
            // path for events reported from canonical watched directories.
            for path in [path, canonical] {
                if path.extension().is_some_and(|ext| ext == "rs") {
                    config.captured_rust_inputs.insert(path.clone());
                }
                // Regular Rust inputs already watched by the compiler retain Rs classification.
                if path.extension().is_some_and(|ext| ext == "rs")
                    && config
                        .source_roots
                        .iter()
                        .any(|root| path.starts_with(root))
                {
                    continue;
                }
                files.insert(path);
            }
        }
    }
    config.input_files = files.into_iter().collect();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rustc_rules_include_root_and_escaped_external_inputs() {
        let text = "target/bin: src/main.rs message.txt ../shared/with\\ space.txt \\\n includes/bytes.bin\n\nmessage.txt:\n# env-dep:X=ignored\n";
        assert_eq!(
            inputs(text),
            [
                "src/main.rs",
                "message.txt",
                "../shared/with space.txt",
                "includes/bytes.bin"
            ]
            .map(PathBuf::from)
            .into()
        );
    }
    #[test]
    fn include_path_edits_recapture_new_dependencies_but_body_edits_do_not() {
        let base = br#"fn f() { include_str!("one.txt"); }"#;
        assert!(includes_changed(
            base,
            br#"fn f() { include_str!("two.txt"); }"#
        ));
        assert!(includes_changed(
            b"fn f() {}",
            br#"fn f() { include_bytes!(concat!("x", ".bin")); }"#
        ));
        assert!(includes_changed(base, b"fn f() {}"));
        assert!(!includes_changed(
            base,
            br#"fn f() { include_str! ( /* same */ "one.txt" ); 2; }"#
        ));
        assert!(!includes_changed(
            b"fn f() {}",
            br#"fn f() { let text = "include_str!(x)"; }"#
        ));
    }

    #[test]
    fn capture_restores_member_working_directory_for_relative_dep_info() {
        let record = serde_json::json!({"cwd":"/workspace/member", "crate_name":"member",
            "args":["--out-dir","/workspace/target/debug/deps","-C","extra-filename=-123"]});
        assert_eq!(
            captured_depfile(&record),
            Some((
                PathBuf::from("/workspace/target/debug/deps/member-123.d"),
                PathBuf::from("/workspace/member")
            ))
        );
    }

    #[test]
    fn stock_dep_info_is_read_and_missing_parent_relative_inputs_are_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("src")).unwrap();
        fs::create_dir_all(root.join("target/refresh/stock/debug")).unwrap();
        fs::create_dir_all(root.join("target/refresh/check/debug")).unwrap();
        fs::write(
            root.join("target/refresh/stock/debug/app.d"),
            "bin: future.dat src/../gone.txt\n",
        )
        .unwrap();
        fs::write(
            root.join("target/refresh/check/debug/app.d"),
            "bin: checker-only.dat\n",
        )
        .unwrap();
        let meta = serde_json::from_value(serde_json::json!({
            "packages":[{"id":"app-id","name":"app","manifest_path":root.join("Cargo.toml"),
                "source":null,"targets":[{"name":"app","kind":["bin"],"src_path":root.join("src/main.rs")}],
                "metadata":{},"default_run":null}],
            "workspace_root":root,"target_directory":root.join("target"),"workspace_default_members":["app-id"]
        })).unwrap();
        let mut config = Config::resolve(Default::default(), &meta, root).unwrap();
        extend(&mut config).unwrap();
        assert!(config.input_files.contains(&root.join("future.dat")));
        assert!(config.input_files.contains(&root.join("gone.txt")));
        assert!(!config.input_files.contains(&root.join("checker-only.dat")));
    }

    #[test]
    fn relative_build_inputs_belong_only_to_the_declaring_package() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let registry = root.join("registry/typenum");
        fs::create_dir_all(registry.join("tests")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        let output = root.join("target/refresh/stock/debug/build/typenum-abcdef");
        fs::create_dir_all(&output).unwrap();
        fs::write(output.join("output"), "cargo:rerun-if-changed=tests\n").unwrap();
        let meta = serde_json::from_value(serde_json::json!({
            "packages":[{"id":"app-id","name":"app","manifest_path":root.join("Cargo.toml"),
                "source":null,"targets":[{"name":"app","kind":["bin"],"src_path":root.join("src/main.rs")}],
                "metadata":{},"default_run":null}],
            "workspace_root":root,"target_directory":root.join("target"),"workspace_default_members":["app-id"]
        })).unwrap();
        let mut config = Config::resolve(Default::default(), &meta, root).unwrap();
        config
            .build_package_roots
            .insert("typenum".into(), vec![registry.clone()]);
        extend(&mut config).unwrap();
        assert!(config.input_files.contains(&registry.join("tests")));
        assert!(!config.input_files.contains(&root.join("tests")));
        let own_output = output.parent().unwrap().join("app-123456");
        fs::create_dir(&own_output).unwrap();
        fs::write(own_output.join("output"), "cargo::rerun-if-changed=tests\n").unwrap();
        extend(&mut config).unwrap();
        assert!(config.input_files.contains(&root.join("tests")));
    }

    #[test]
    fn embedded_input_outside_source_root_is_watched_and_requires_stock() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("message.txt"), "before").unwrap();
        let mut spec = crate::watcher::WatchSpec::new(
            vec![root.join("src")],
            vec![root.join("message.txt")],
            vec![],
        );
        spec.exclude_file(root.join("events"));
        let mut watcher = crate::watcher::SourceWatcher::new(spec).unwrap();
        let mut tracker =
            crate::changes::ChangeTracker::new(watcher.files().unwrap(), watcher.build_inputs())
                .unwrap();
        fs::write(root.join("message.txt"), "after").unwrap();
        let batch = watcher
            .next_batch(std::time::Duration::from_secs(2))
            .unwrap()
            .unwrap();
        let changes = tracker.observe(batch.paths).unwrap();
        assert_eq!(changes.changes.len(), 1);
        assert_eq!(changes.changes[0].kind, crate::changes::PathKind::Manifest);
    }
}
