//! Certificate-only classifier view for async impl methods. The ordinary guard
//! remains byte-for-byte compatible with its conservative impl hazard policy.
use crate::{FileSnapshot, is_rust};
use quote::ToTokens;
use std::{collections::BTreeMap, path::PathBuf};
use syn::{
    visit::{self, Visit},
    visit_mut::{self, VisitMut},
};

#[derive(Default)]
struct Counts(BTreeMap<String, usize>);
impl<'ast> Visit<'ast> for Counts {
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        *self.0.entry(f.sig.ident.to_string()).or_default() += 1;
        visit::visit_item_fn(self, f);
    }
    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        *self.0.entry(f.sig.ident.to_string()).or_default() += 1;
        visit::visit_impl_item_fn(self, f);
    }
    fn visit_trait_item_fn(&mut self, f: &'ast syn::TraitItemFn) {
        *self.0.entry(f.sig.ident.to_string()).or_default() += 1;
        visit::visit_trait_item_fn(self, f);
    }
}
fn declaration_lines(text: &str) -> BTreeMap<String, (u32, u32)> {
    use rustc_lexer::TokenKind as T;
    let mut at = 0;
    let mut tokens = Vec::new();
    for token in rustc_lexer::tokenize(text) {
        let start = at;
        at += token.len;
        if !matches!(
            token.kind,
            T::Whitespace | T::LineComment | T::BlockComment { .. }
        ) {
            tokens.push((&text[start..at], start));
        }
    }
    let mut lines = BTreeMap::new();
    for (index, triple) in tokens.windows(3).enumerate() {
        if triple[0].0 != "async" || triple[1].0 != "fn" {
            continue;
        }
        // Attribute macros can map the owning function to an attribute callsite.
        // Bind that anchor only within this unique method's original header.
        let mut first = index;
        loop {
            if first > 0 && tokens[first - 1].0 == "pub" {
                first -= 1;
                continue;
            }
            let Some(close) = first.checked_sub(1) else {
                break;
            };
            let (open, close_token) = match tokens[close].0 {
                "]" => ("[", "]"),
                ")" => ("(", ")"),
                _ => break,
            };
            let mut depth = 1;
            let mut at = close;
            while at > 0 && depth != 0 {
                at -= 1;
                if tokens[at].0 == close_token {
                    depth += 1;
                }
                if tokens[at].0 == open {
                    depth -= 1;
                }
            }
            let introducer = if open == "[" { "#" } else { "pub" };
            if depth != 0 || at == 0 || tokens[at - 1].0 != introducer {
                break;
            }
            first = at - 1;
        }
        let line = |at: usize| {
            u32::try_from(text[..tokens[at].1].bytes().filter(|b| *b == b'\n').count() + 1).ok()
        };
        if let (Some(first), Some(last)) = (line(first), line(index)) {
            lines.insert(triple[2].0.to_owned(), (first, last));
        }
    }
    lines
}
struct Lift<'a> {
    counts: &'a Counts,
    lines: &'a BTreeMap<String, (u32, u32)>,
    methods: Vec<syn::Item>,
    owners: BTreeMap<String, (u32, u32)>,
}
impl VisitMut for Lift<'_> {
    fn visit_impl_item_fn_mut(&mut self, f: &mut syn::ImplItemFn) {
        let name = f.sig.ident.to_string();
        if f.sig.asyncness.is_none() || self.counts.0.get(&name) != Some(&1) {
            return visit_mut::visit_impl_item_fn_mut(self, f);
        }
        let Some(&line) = self.lines.get(&name) else {
            return;
        };
        let body = std::mem::replace(&mut f.block, syn::parse_quote!({}));
        // Original impl headers/attributes stay in the structural outline. The
        // lifted signature is only a classifier view; it is never compiled.
        self.methods.push(syn::Item::Fn(syn::ItemFn {
            attrs: f.attrs.clone(),
            vis: f.vis.clone(),
            sig: f.sig.clone(),
            block: Box::new(body),
        }));
        self.owners.insert(format!("::{name}"), line);
    }
}
/// Only unambiguous method names are lifted. Bounded header coverage binds
/// the compiler proof to the edited method even when qualified type spellings
/// or trait aliases differ. Missing macro source locations fail closed.
pub(crate) fn view(
    files: &[FileSnapshot],
) -> (Vec<FileSnapshot>, BTreeMap<(PathBuf, String), (u32, u32)>) {
    let mut result = Vec::new();
    let mut owners = BTreeMap::new();
    for file in files {
        let mut bytes = file.bytes.clone();
        if is_rust(&file.path) {
            if let Ok(text) = std::str::from_utf8(&file.bytes) {
                if let Ok(mut ast) = syn::parse_file(text) {
                    let mut counts = Counts::default();
                    counts.visit_file(&ast);
                    let lines = declaration_lines(text);
                    let mut lift = Lift {
                        counts: &counts,
                        lines: &lines,
                        methods: Vec::new(),
                        owners: BTreeMap::new(),
                    };
                    lift.visit_file_mut(&mut ast);
                    if !lift.methods.is_empty() {
                        ast.items.extend(lift.methods);
                        bytes = ast.to_token_stream().to_string().into_bytes();
                        for (name, line) in lift.owners {
                            owners.insert((file.path.clone(), name), line);
                        }
                    }
                }
            }
        }
        result.push(FileSnapshot::new(file.path.clone(), bytes));
    }
    (result, owners)
}
