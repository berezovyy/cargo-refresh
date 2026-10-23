use crate::admission::Names;
use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};
use quote::ToTokens;
use std::collections::BTreeMap;
use syn::{
    visit::Visit,
    visit_mut::{self, VisitMut},
};

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Function {
    pub header: String,
    pub async_shape: String,
    pub hazards: Vec<String>,
    pub attributed_body: String,
    pub simple: bool,
    pub body: blake3::Hash,
}
impl Function {
    pub fn same_policy(&self, other: &Self) -> bool {
        self.header == other.header
            && self.async_shape == other.async_shape
            && self.hazards == other.hazards
            && self.attributed_body == other.attributed_body
            && self.simple == other.simple
    }
}

#[derive(Default, Debug, Eq, PartialEq)]
pub(crate) struct Analysis {
    pub names: Names,
    pub structure: String,
    pub functions: BTreeMap<String, Function>,
    pub hazards: Vec<String>,
}
impl Analysis {
    /// Ignore only newly introduced private identity aliases, with no prior
    /// references/bindings and no uses outside bodies. Arbitrary import edits
    /// still change the layout outline. Function headers/hazards are checked
    /// independently by the classifier.
    pub fn only_body_imports_added(&self, old: &Self) -> bool {
        if !self.structure.contains("identity") {
            return false;
        }
        let Ok(mut tree) = syn::parse_file(&self.structure) else {
            return false;
        };
        let tokens: TokenStream = match self.structure.parse() {
            Ok(tokens) => tokens,
            Err(_) => return false,
        };
        fn count(tokens: TokenStream, name: &str) -> usize {
            tokens
                .into_iter()
                .map(|token| match token {
                    TokenTree::Ident(id) => usize::from(id == name),
                    TokenTree::Group(g) => count(g.stream(), name),
                    _ => 0,
                })
                .sum()
        }
        fn remove(items: &mut Vec<syn::Item>, old: &Analysis, tokens: &TokenStream) {
            items.retain(|item| {
                let syn::Item::Use(item) = item else {
                    return true;
                };
                let Some(alias) = identity_alias(item) else {
                    return true;
                };
                old.names.mentions(&alias) || count(tokens.clone(), &alias) != 1
            });
            for item in items {
                if let syn::Item::Mod(module) = item
                    && let Some((_, items)) = &mut module.content
                {
                    remove(items, old, tokens);
                }
            }
        }
        remove(&mut tree.items, old, &tokens);
        tree.to_token_stream().to_string() == old.structure
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let source = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        if source.len() >= 8192
            && source.contains("mod ")
            // A parent module can shadow a prelude macro used by a child.
            && !source.contains("macro") && !source.contains("use ")
            && let Some(result) = Self::parallel(source)
        {
            return result;
        }
        Self::serial(source, &mut Vec::new())
    }

    fn serial(source: &str, module_slots: &mut Vec<usize>) -> Result<Self, String> {
        Self::with_outline::<true>(source, module_slots)
    }

    fn with_outline<const FAST: bool>(
        source: &str,
        module_slots: &mut Vec<usize>,
    ) -> Result<Self, String> {
        let mut tree = syn::parse_file(source).map_err(|error| error.to_string())?;
        let mut result = Self::default();
        result.names.visit_file(&tree);
        let mut shadowing = MacroShadowing::default();
        shadowing.visit_file(&tree);
        Outline::<FAST> {
            allow_bare_macros: !shadowing.0,
            path: Vec::new(),
            result: &mut result,
            module_slots,
        }
        .visit_file_mut(&mut tree);
        if !FAST {
            Remove.visit_file_mut(&mut tree);
        }
        result.structure = tree.to_token_stream().to_string();
        Ok(result)
    }

    /// Lex once to find top-level inline modules. Their contents are
    /// independent syn grammar scopes, so parse them on at most three workers
    /// while the calling thread parses the surrounding file. No AST or spans
    /// cross threads. The serial path remains the grammar/policy oracle.
    fn parallel(source: &str) -> Option<Result<Self, String>> {
        let (skeleton, modules) = partition_modules(source)?;
        Some(std::thread::scope(|scope| {
            let chunk_size = modules.len().div_ceil(3);
            let mut workers = Vec::new();
            for chunk in modules.chunks(chunk_size) {
                let worker = std::thread::Builder::new()
                    .name("refresh-guard".to_owned())
                    .stack_size(8 * 1024 * 1024)
                    .spawn_scoped(scope, move || {
                        chunk
                            .iter()
                            .map(|source| Self::serial(source, &mut Vec::new()))
                            .collect::<Result<Vec<_>, _>>()
                    });
                match worker {
                    Ok(worker) => workers.push(worker),
                    Err(_) => return Self::serial(source, &mut Vec::new()),
                }
            }
            let mut slots = Vec::new();
            let mut root = Self::serial(&skeleton, &mut slots)?;
            let mut analyzed = Vec::new();
            for worker in workers {
                analyzed.extend(
                    worker
                        .join()
                        .map_err(|_| "syntax worker panicked".to_owned())??,
                );
            }
            // Require an exact module boundary correspondence before merging.
            if slots.len() != analyzed.len() {
                return Self::serial(source, &mut Vec::new());
            }
            let base_hazards = std::mem::take(&mut root.hazards);
            let mut previous = 0;
            let mut bodies = Vec::new();
            for (module, slot) in analyzed.into_iter().zip(slots) {
                root.names.merge(module.names);
                root.functions.extend(module.functions);
                root.hazards
                    .extend_from_slice(&base_hazards[previous..slot]);
                root.hazards.extend(module.hazards);
                previous = slot;
                let outlined = module
                    .structure
                    .parse::<TokenStream>()
                    .map_err(|e| e.to_string())?;
                let body = outlined
                    .into_iter()
                    .find_map(|token| match token {
                        TokenTree::Group(group) if group.delimiter() == Delimiter::Brace => {
                            Some(group.stream())
                        }
                        _ => None,
                    })
                    .ok_or_else(|| "missing module outline".to_owned())?;
                bodies.push(body);
            }
            root.hazards.extend_from_slice(&base_hazards[previous..]);
            // Reinsert outlined module contents to keep the exact same structure
            // representation as serial analysis, including threshold crossings.
            let mut structure: Vec<_> = root
                .structure
                .parse::<TokenStream>()
                .map_err(|e| e.to_string())?
                .into_iter()
                .collect();
            let mut next = bodies.into_iter();
            for index in 0..structure.len().saturating_sub(2) {
                if matches!(&structure[index], TokenTree::Ident(id) if id == "mod")
                    && matches!(&structure[index + 1], TokenTree::Ident(_))
                    && matches!(&structure[index + 2], TokenTree::Group(group) if group.delimiter() == Delimiter::Brace)
                {
                    let Some(body) = next.next() else {
                        return Self::serial(source, &mut Vec::new());
                    };
                    structure[index + 2] = TokenTree::Group(Group::new(Delimiter::Brace, body));
                }
            }
            if next.next().is_some() {
                return Self::serial(source, &mut Vec::new());
            }
            root.structure = structure.into_iter().collect::<TokenStream>().to_string();
            Ok(root)
        }))
    }
    pub fn weight(&self) -> usize {
        self.names.weight()
            + self.structure.capacity()
            + 512
            + self
                .hazards
                .iter()
                .map(|s| s.capacity() + 128)
                .sum::<usize>()
            + self
                .functions
                .iter()
                .map(|(name, f)| {
                    name.capacity()
                        + f.header.capacity()
                        + f.async_shape.capacity()
                        + f.attributed_body.capacity()
                        + 512
                        + f.hazards.iter().map(|s| s.capacity() + 128).sum::<usize>()
                })
                .sum::<usize>()
    }
}

pub(crate) fn identity_alias(item: &syn::ItemUse) -> Option<String> {
    if !item.attrs.is_empty() || !matches!(item.vis, syn::Visibility::Inherited) {
        return None;
    }
    let syn::UseTree::Path(root) = &item.tree else {
        return None;
    };
    if root.ident != "std" && root.ident != "core" {
        return None;
    }
    let syn::UseTree::Path(module) = &*root.tree else {
        return None;
    };
    if module.ident != "convert" {
        return None;
    }
    let syn::UseTree::Rename(rename) = &*module.tree else {
        return None;
    };
    (rename.ident == "identity").then(|| rename.rename.to_string())
}

/// The compiler lexer supplies byte boundaries only; syn still validates every
/// module and the entire outside skeleton. Unknown/unsupported lexical forms,
/// shebang/BOM handling, and mismatched delimiters use the serial parser.
fn partition_modules(source: &str) -> Option<(String, Vec<String>)> {
    use rustc_lexer::{LiteralKind, TokenKind as T};
    if source.starts_with("#!") || source.starts_with('\u{feff}') {
        return None;
    }
    let mut delimiters = Vec::new();
    let mut pending = None;
    let mut named = false;
    let mut active = None;
    let mut ranges = Vec::new();
    let mut offset = 0;
    for token in rustc_lexer::tokenize(source) {
        let start = offset;
        offset += token.len;
        // This lexer predates C/guarded strings. Let syn handle all newer
        // literal prefixes rather than guessing their delimiter boundaries.
        if (token.kind == T::Ident && source[offset..].starts_with(['"', '#', '\'']))
            || (token.kind == T::Pound && source[offset..].starts_with(['"', '#']))
        {
            return None;
        }
        match token.kind {
            T::Whitespace
            | T::LineComment
            | T::BlockComment {
                terminated: true, ..
            } => continue,
            T::Unknown
            | T::BlockComment {
                terminated: false, ..
            } => return None,
            T::Literal {
                kind:
                    LiteralKind::Char { terminated: false }
                    | LiteralKind::Byte { terminated: false }
                    | LiteralKind::Str { terminated: false }
                    | LiteralKind::ByteStr { terminated: false }
                    | LiteralKind::RawStr { started: false, .. }
                    | LiteralKind::RawStr {
                        terminated: false, ..
                    }
                    | LiteralKind::RawByteStr { started: false, .. }
                    | LiteralKind::RawByteStr {
                        terminated: false, ..
                    },
                ..
            } => return None,
            T::OpenBrace => {
                if delimiters.is_empty() && named {
                    active = pending.map(|module_start| (module_start, offset));
                }
                pending = None;
                named = false;
                delimiters.push(T::OpenBrace);
            }
            T::OpenParen | T::OpenBracket => {
                pending = None;
                named = false;
                delimiters.push(token.kind);
            }
            T::CloseBrace | T::CloseParen | T::CloseBracket => {
                let expected = match token.kind {
                    T::CloseBrace => T::OpenBrace,
                    T::CloseParen => T::OpenParen,
                    _ => T::OpenBracket,
                };
                if delimiters.pop() != Some(expected) {
                    return None;
                }
                if delimiters.is_empty()
                    && let Some((module_start, body_start)) = active.take()
                {
                    ranges.push((module_start, body_start, offset));
                }
                pending = None;
                named = false;
            }
            T::Ident | T::RawIdent if delimiters.is_empty() => {
                if pending.is_some() && !named {
                    named = true;
                } else {
                    pending = (token.kind == T::Ident && &source[start..offset] == "mod")
                        .then_some(start);
                    named = false;
                }
            }
            _ if delimiters.is_empty() => {
                pending = None;
                named = false;
            }
            _ => (),
        }
    }
    if !delimiters.is_empty() || ranges.len() < 2 {
        return None;
    }
    let mut skeleton = String::new();
    let mut modules = Vec::with_capacity(ranges.len());
    let mut previous = 0;
    for (module_start, body_start, end) in ranges {
        modules.push(source[module_start..end].to_owned());
        skeleton.push_str(&source[previous..body_start]);
        skeleton.push('}');
        previous = end;
    }
    skeleton.push_str(&source[previous..]);
    Some((skeleton, modules))
}

/// Only these standard expression macros may omit the opaque-token hazard.
fn expression_macro(path: &syn::Path, allow_bare: bool) -> bool {
    let parts: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    let name = match parts.as_slice() {
        [name] if allow_bare => name,
        [root, name] if root == "std" || root == "core" => name,
        _ => return false,
    };
    matches!(
        name.as_str(),
        "format"
            | "format_args"
            | "print"
            | "println"
            | "eprint"
            | "eprintln"
            | "write"
            | "writeln"
            | "panic"
            | "assert"
            | "assert_eq"
            | "assert_ne"
            | "debug_assert"
            | "debug_assert_eq"
            | "debug_assert_ne"
            | "unreachable"
            | "todo"
            | "unimplemented"
            | "vec"
            | "matches"
            | "dbg"
            | "concat"
            | "stringify"
            | "line"
            | "column"
            | "file"
    )
}

#[derive(Default)]
struct MacroShadowing(bool);
impl<'ast> Visit<'ast> for MacroShadowing {
    fn visit_item_macro(&mut self, item: &'ast syn::ItemMacro) {
        self.0 |= item.ident.is_some();
    }
    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        self.0 |= attr.path().is_ident("macro_use");
    }
    fn visit_use_tree(&mut self, tree: &'ast syn::UseTree) {
        let name = match tree {
            syn::UseTree::Name(n) => Some(&n.ident),
            syn::UseTree::Rename(n) => Some(&n.rename),
            syn::UseTree::Glob(_) => {
                self.0 = true;
                None
            }
            _ => None,
        };
        if let Some(name) = name {
            let path: syn::Path = syn::parse_quote!(#name);
            self.0 |= expression_macro(&path, true);
        }
        syn::visit::visit_use_tree(self, tree);
    }
}

/// Standard macros still evaluate expressions that can contain local items,
/// closures, async blocks or opaque nested macros. Parse their argument grammar
/// rather than treating the entire token stream as an expression or ignoring it.
fn safe_macro_arguments(item: &syn::Macro, allow_bare: bool) -> bool {
    use syn::parse::Parser;
    let name = item.path.segments.last().unwrap().ident.to_string();
    // stringify never expands or evaluates its input tokens.
    if name == "stringify" {
        return true;
    }
    let tokens = item.tokens.clone();
    let mut hazards = Hazards {
        items: Vec::new(),
        allow_bare,
    };
    if name == "vec" {
        let Ok(array) = syn::parse2::<syn::Expr>(quote::quote!([#tokens])) else {
            return false;
        };
        hazards.visit_expr(&array);
    } else if name == "matches" {
        let parser = |input: syn::parse::ParseStream<'_>| -> syn::Result<_> {
            let value: syn::Expr = input.parse()?;
            input.parse::<syn::Token![,]>()?;
            let pattern = syn::Pat::parse_multi_with_leading_vert(input)?;
            let guard = if input.peek(syn::Token![if]) {
                input.parse::<syn::Token![if]>()?;
                Some(input.parse::<syn::Expr>()?)
            } else {
                None
            };
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
            }
            Ok((value, pattern, guard))
        };
        let Ok((value, pattern, guard)) = parser.parse2(tokens) else {
            return false;
        };
        hazards.visit_expr(&value);
        hazards.visit_pat(&pattern);
        if let Some(guard) = guard {
            hazards.visit_expr(&guard);
        }
    } else {
        let Ok(expressions) =
            syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated
                .parse2(tokens)
        else {
            return false;
        };
        for expression in expressions {
            hazards.visit_expr(&expression);
        }
    }
    hazards.items.is_empty()
}

struct Hazards {
    items: Vec<String>,
    allow_bare: bool,
}
impl<'ast> Visit<'ast> for Hazards {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        self.items.push(item.to_token_stream().to_string());
    }
    fn visit_expr_closure(&mut self, item: &'ast syn::ExprClosure) {
        self.items.push(item.to_token_stream().to_string());
    }
    fn visit_expr_async(&mut self, item: &'ast syn::ExprAsync) {
        self.items.push(item.block.to_token_stream().to_string());
    }
    fn visit_macro(&mut self, item: &'ast syn::Macro) {
        if !expression_macro(&item.path, self.allow_bare)
            || !safe_macro_arguments(item, self.allow_bare)
        {
            self.items.push(item.to_token_stream().to_string());
        }
    }
}

fn opaque_attribute(attr: &syn::Attribute) -> bool {
    // Only the builtin that prevents inlining is known to leave this body's
    // patch boundary intact. Inline hints/always remain conservative.
    !attr.path().is_ident("doc")
        && !(attr.path().is_ident("inline")
            && attr
                .parse_args::<syn::Ident>()
                .is_ok_and(|arg| arg == "never"))
}

struct Outline<'a, const FAST: bool> {
    allow_bare_macros: bool,
    path: Vec<String>,
    result: &'a mut Analysis,
    module_slots: &'a mut Vec<usize>,
}
impl<const FAST: bool> Outline<'_, FAST> {
    fn visit_items(&mut self, items: &mut Vec<syn::Item>) {
        items.retain_mut(|item| {
            if let syn::Item::Fn(function) = item {
                self.record_function(function);
                false
            } else {
                self.visit_item_mut(item);
                true
            }
        });
    }

    fn record_function(&mut self, item: &mut syn::ItemFn) {
        let mut hazards = Hazards {
            items: Vec::new(),
            allow_bare: self.allow_bare_macros,
        };
        hazards.visit_block(&item.block);
        let key = format!("{}::{}", self.path.join("::"), item.sig.ident);
        let body = item.block.to_token_stream().to_string();
        let runtime_attrs = item.attrs.iter().any(opaque_attribute);
        let simple = !runtime_attrs
            && item.sig.unsafety.is_none()
            && item.sig.asyncness.is_none()
            && item.sig.abi.is_none()
            && item.sig.generics.params.is_empty();
        let empty = if FAST {
            syn::Block {
                brace_token: Default::default(),
                stmts: Vec::new(),
            }
        } else {
            syn::parse_quote!({})
        };
        let original = std::mem::replace(&mut item.block, Box::new(empty));
        let header = item.to_token_stream().to_string();
        item.block = original;
        self.result.functions.insert(
            key,
            Function {
                header,
                async_shape: if item.sig.asyncness.is_some() {
                    body.clone()
                } else {
                    String::new()
                },
                hazards: hazards.items,
                simple,
                body: blake3::hash(body.as_bytes()),
                attributed_body: if runtime_attrs || item.sig.ident == "main" {
                    body
                } else {
                    String::new()
                },
            },
        );
    }
}

impl<const FAST: bool> VisitMut for Outline<'_, FAST> {
    fn visit_file_mut(&mut self, file: &mut syn::File) {
        if FAST {
            self.visit_items(&mut file.items);
        } else {
            visit_mut::visit_file_mut(self, file);
        }
    }

    fn visit_item_mod_mut(&mut self, item: &mut syn::ItemMod) {
        if self.path.is_empty() && item.content.is_some() {
            self.module_slots.push(self.result.hazards.len());
        }
        self.path.push(item.ident.to_string());
        if FAST {
            if let Some((_, items)) = &mut item.content {
                self.visit_items(items);
            }
        } else {
            visit_mut::visit_item_mod_mut(self, item);
        }
        self.path.pop();
    }
    fn visit_item_fn_mut(&mut self, item: &mut syn::ItemFn) {
        self.record_function(item);
        // Nested free functions outside file/module lists retain the prototype's
        // placeholder in structure and still receive per-function analysis.
        *item = syn::parse_quote!(
            fn __refresh_function_placeholder() {}
        );
    }
    fn visit_impl_item_fn_mut(&mut self, item: &mut syn::ImplItemFn) {
        let mut hazards = Hazards {
            items: Vec::new(),
            allow_bare: self.allow_bare_macros,
        };
        hazards.visit_block(&item.block);
        self.result.hazards.extend(hazards.items);
        if item.sig.asyncness.is_some() {
            self.result
                .hazards
                .push(item.block.to_token_stream().to_string());
        }
        if item.attrs.iter().any(opaque_attribute) {
            self.result
                .hazards
                .push(item.block.to_token_stream().to_string());
        }
        item.block = if FAST {
            syn::Block {
                brace_token: Default::default(),
                stmts: Vec::new(),
            }
        } else {
            syn::parse_quote!({})
        };
    }
}

struct Remove;
impl VisitMut for Remove {
    fn visit_file_mut(&mut self, file: &mut syn::File) {
        file.items.retain(|item| !matches!(item, syn::Item::Fn(_)));
        visit_mut::visit_file_mut(self, file);
    }
    fn visit_item_mod_mut(&mut self, module: &mut syn::ItemMod) {
        if let Some((_, items)) = &mut module.content {
            items.retain(|item| !matches!(item, syn::Item::Fn(_)));
        }
        visit_mut::visit_item_mod_mut(self, module);
    }
}

#[cfg(test)]
mod tests {
    use super::Analysis;
    use std::{fs, path::Path};

    fn compare(source: &str, label: &str) {
        let serial = Analysis::with_outline::<false>(source, &mut Vec::new());
        let optimized = Analysis::parse(source.as_bytes());
        match (serial, optimized) {
            (Ok(a), Ok(b)) => assert_eq!(a, b, "{label}"),
            (Err(_), Err(_)) => (),
            (a, b) => panic!("{label}: serial={a:?}, optimized={b:?}"),
        }
    }

    #[test]
    fn parallel_analysis_matches_serial_on_entire_frozen_source_pool() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
        for file in fs::read_dir(root.join("sources")).unwrap() {
            let path = file.unwrap().path();
            compare(&fs::read_to_string(&path).unwrap(), path.to_str().unwrap());
        }
        compare(
            &fs::read_to_string(root.join("web-handlers-1733.rs")).unwrap(),
            "1733-line file",
        );
    }

    #[test]
    fn parallel_analysis_preserves_module_context_and_hazard_order() {
        let padding = format!("// {}\n", "padding".repeat(1500));
        for source in [
            "mod a { fn x() {} } mod b { async fn y() { \"before\"; } }",
            "//! crate docs\n#[cfg(any())] pub mod a { //! inner docs\nfn x() { log!(a::b); } } mod b { fn y() {} }",
            "struct S; impl S { fn x() { first!(); } } mod a { impl S { fn x() { second!(); } } } impl S { fn z() { third!(); } } mod b { impl S { fn x() { fourth!(); } } }",
            "mod a { pub mod nested { fn x() { let f = || 1; } } } mod b { extern \"C\" { fn x(); } }",
            "mod a { macro_rules! x { ($x:tt) => { $x + 1 }; } } mod b { fn x<'a>(x: &'a str) -> &'a str { x } }",
            "mod a {} mod b {} fn mod pretend {}",
            "mod a { fn broken( {} } mod b {}",
            "mod a { fn x() {} } mod b {} fn x( {",
            "mod a; mod b {} mod c {}",
            "mod a { #[doc = r#\"raw docs\"#] fn x() {} } mod b { fn x() { let t = '\\''; } }",
            "mod a { fn x() { let y = (1..=3).map(|v| v << 2); } } mod b { fn x() { path::<u8>(); } }",
            "mod a { fn x() { let s = r###\"} mod pretend {} {\"###; } } mod b {}",
            "mod a { /* nested { /* } */ comments } */ fn x() {} } mod b {}",
            "mod a { fn x() { let s = c\"} mod pretend {} {\"; } } mod b {}",
            "mod a { fn x() { let s = cr#\"} \" mod pretend {} {\"#; } } mod b {}",
            "mod /* header comment */ a { fn x() {} } pub(crate) mod b {}",
            "mod r#type { fn r#match() {} } mod b {}",
            "const X: () = { mod inner {} }; mod a {} mod b {}",
            "trait T { fn f() { mod inner {} } } mod a {} mod b {}",
        ] {
            compare(&(padding.clone() + source), source);
        }
        // Shebang parsing belongs to syn::parse_file; tokenization falls back.
        compare(
            &("#!/usr/bin/env rust-script\n".to_owned() + &padding + "mod a {} mod b {}"),
            "shebang",
        );
    }

    #[test]
    fn deterministic_buffer_mutations_match_serial_parse_and_analysis() {
        let padding = format!("// {}\n", "padding".repeat(1500));
        let code = "#[allow(dead_code)] mod a { fn f<'a>(x: &'a str) -> &'a str { let s = r#\"} mod pretend {} {\"#; x } } mod b { struct S; impl S { async fn g() { log!(\"text\"); } } }";
        let source = padding.clone() + code;
        assert!(super::partition_modules(&source).is_some());
        let replacements = [
            "{",
            "}",
            "\"",
            "'",
            "#",
            "/",
            "*",
            "(",
            "]",
            ";",
            " ",
            "mod other {}",
            "/*",
            "//\n",
        ];
        let mut seed = 47_u64;
        for index in 0..512 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let position = (seed as usize) % code.len();
            let mut changed = code.to_owned();
            match index % 3 {
                0 => {
                    changed.remove(position);
                }
                1 => changed.insert_str(
                    position,
                    replacements[(seed >> 32) as usize % replacements.len()],
                ),
                _ => changed.replace_range(
                    position..position + 1,
                    replacements[(seed >> 32) as usize % replacements.len()],
                ),
            }
            compare(
                &(padding.clone() + &changed),
                &format!("buffer mutation {index}"),
            );
        }
    }
}
