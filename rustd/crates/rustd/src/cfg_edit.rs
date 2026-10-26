//! Conservative proof that an edit changes only cfg-disabled source.
use ide_db::{
    RootDatabase,
    base_db::{SourceDatabase as _, relevant_crates},
};
use syntax::{AstNode, Edition, SyntaxNode, WalkEvent, ast};
use vfs::FileId;

/// Check every crate reading the file: a shared module can have different cfgs.
pub(crate) fn no_effect(db: &RootDatabase, file: FileId, after: &str) -> bool {
    let crates = relevant_crates(db, file);
    !crates.is_empty()
        && crates.iter().all(|krate| {
            let edition = krate.data(db).edition;
            let before = db.file_text(file);
            let options = krate.cfg_options(db).clone();
            only_disabled_changes(before.text(db), after, edition, &options)
        })
}

fn only_disabled_changes(
    before: &str,
    after: &str,
    edition: Edition,
    options: &cfg::CfgOptions,
) -> bool {
    // Identical rewrites are the controller's hash skip. They must not suppress
    // an explicit maintenance promotion that happens to notice their new mtime.
    before != after
        && projection(before, edition, options)
            .zip(projection(after, edition, options))
            .is_some_and(|(before, after)| before == after)
}

fn projection(text: &str, edition: Edition, options: &cfg::CfgOptions) -> Option<String> {
    let parsed = ast::SourceFile::parse(text, edition);
    if !parsed.errors().is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut walk = parsed.syntax_node().preorder_with_tokens();
    while let Some(event) = walk.next() {
        match event {
            WalkEvent::Enter(syntax::NodeOrToken::Node(node)) if disabled(&node, options) => {
                // An attribute macro on an active ancestor can inspect this
                // item's tokens before nested cfg stripping. Such input is opaque.
                const INERT: &[&str] = &[
                    "allow",
                    "cfg",
                    "cold",
                    "deny",
                    "deprecated",
                    "doc",
                    "expect",
                    "forbid",
                    "inline",
                    "must_use",
                    "track_caller",
                    "warn",
                ];
                if node.ancestors().skip(1).any(|ancestor| {
                    ancestor.children().filter_map(ast::Attr::cast).any(|attr| {
                        attr.simple_name().is_none_or(|name| !INERT.contains(&name.as_str()))
                    })
                }) {
                    return None;
                }
                // Keep the attributes themselves: toggling cfg is an effective edit.
                for attr in node.children().filter_map(ast::Attr::cast) {
                    out.push_str(&attr.syntax().to_string());
                }
                out.push('\0');
                // Preserve line/column positions after the excluded item (line!,
                // column! and location metadata can observe them).
                let raw = node.to_string();
                out.extend(raw.chars().filter(|&c| c == '\n'));
                let suffix = &text[usize::from(node.text_range().end())..];
                // A following newline resets the column before any active code.
                if !suffix.split('\n').next()?.trim().is_empty() {
                    let tail = raw.rsplit('\n').next()?;
                    out.push_str(&format!("{}:{}", tail.len(), tail.chars().count()));
                }
                walk.skip_subtree();
            }
            WalkEvent::Enter(syntax::NodeOrToken::Token(token)) => out.push_str(token.text()),
            _ => {}
        }
    }
    Some(out)
}
fn disabled(node: &SyntaxNode, options: &cfg::CfgOptions) -> bool {
    // Restrict to items: cfg on expressions and macro input is not this proof.
    if ast::Item::cast(node.clone()).is_none() {
        return false;
    }
    node.children().filter_map(ast::Attr::cast).any(|attr| {
        // An inner cfg applies to its container, rather than this item.
        attr.excl_token().is_none()
            && attr
                .meta()
                .and_then(|meta| match meta {
                    ast::Meta::CfgMeta(meta) => Some(meta),
                    _ => None,
                })
                .and_then(|meta| meta.cfg_predicate())
                .is_some_and(|pred| {
                    options.check(&cfg::CfgExpr::parse_from_ast(pred)) == Some(false)
                })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cfg_projection_proves_only_disabled_item_edits() {
        let options = cfg::CfgOptions::default();
        let p = |s: &str| projection(s, Edition::CURRENT, &options);
        assert!(!only_disabled_changes("fn main() {}", "fn main() {}", Edition::CURRENT, &options));
        for condition in ["windows", "test", "not(all())"] {
            let before =
                format!("#[cfg({condition})]\nfn off() {{\n    missing(1);\n}}\nfn main() {{}}\n");
            assert_eq!(p(&before), p(&before.replace("missing(1)", "different(200)")));
            assert_ne!(p(&before), p(&before.replace("fn main() {}", "fn main() { panic!(); }")));
            assert_ne!(p(&before), p(&before.replace("missing(1);", "missing(1);\n")));
        }
        assert!(p("#[opaque] mod outer { #[cfg(test)] fn off() { 1; } }").is_none());
        assert!(p("#[cfg_attr(windows, opaque)] mod outer { #[cfg(test)] fn off() {} }").is_none());
        let single_line = "#[cfg(test)] fn off() { 1; }\nfn main() {}\n";
        assert_eq!(p(single_line), p(&single_line.replace('1', "200")));
        let enabled = "#[cfg(all())]\nfn on() { 1; }\n";
        assert_ne!(p(enabled), p(&enabled.replace('1', "2")));
        let cfg = "#[cfg(test)]\nfn off() {\n    1;\n}\n";
        assert_ne!(p(cfg), p(&cfg.replace("test", "all()")));
        assert!(p("#[cfg(test)] fn invalid(").is_none());
        let nested_cfg_attr = "#[cfg_attr(windows, cfg(test))] fn on() { 1 }";
        assert_ne!(p(nested_cfg_attr), p(&nested_cfg_attr.replace('1', "2")));
        let unicode = "#[cfg(test)] fn off() { let _ = \"é\"; } fn main() {}";
        assert_ne!(p(unicode), p(&unicode.replace('é', "ab")));
        let macro_input = "m! { #[cfg(test)] fn off() { 1 } }";
        assert_ne!(p(macro_input), p(&macro_input.replace('1', "2")));
    }
}
