//! Syntax admission and conservative live-patch classification.
//!
//! Supply the same changed-file scope on both sides, with additions only in
//! `after` and removals only in `before`. Paths are lexical, relative to the same
//! project root. This library never reads the filesystem. Unchanged files need
//! not be supplied. Admission is a per-file EXP-119 heuristic, not a type checker.
//!
//! [`Guard`] shares bounded analysis results between admission and classification.
//! Free functions use a thread-local default guard. See the README for prototype
//! compatibility details, especially impl methods and the strict async policy.

mod admission;
mod analysis;
mod async_methods;
mod build_script;
pub use build_script::empty_build_script;

use analysis::Analysis;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
};

/// Bytes for one project-relative source/configuration path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileSnapshot {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

impl FileSnapshot {
    pub fn new(path: impl Into<PathBuf>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            bytes: bytes.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionRule {
    Parse,
    IncompleteExpression,
    MissingReturnTail,
    BareFunctionReference,
    UnknownCall,
    RemovedFunction,
    InternalError,
}

impl AdmissionRule {
    pub fn code(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::IncompleteExpression => "incomplete-expression",
            Self::MissingReturnTail => "missing-return-tail",
            Self::BareFunctionReference => "bare-function-reference",
            Self::UnknownCall => "unknown-call",
            Self::RemovedFunction => "removed-function",
            Self::InternalError => "internal-error",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Admission {
    Admit,
    Reject { rule: AdmissionRule, detail: String },
}

/// A changed or newly added free function, qualified by inline module path.
/// Ordinary impl methods are erased by the prototype outline and are not listed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangedFn {
    pub path: PathBuf,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reason {
    InternalError {
        detail: String,
    },
    Manifest {
        path: PathBuf,
    },
    FileSet {
        added: Vec<PathBuf>,
        removed: Vec<PathBuf>,
    },
    Layout {
        path: PathBuf,
    },
    Async {
        path: PathBuf,
        function: String,
    },
    MacroBody {
        path: PathBuf,
        function: String,
    },
    NewComplexFn {
        path: PathBuf,
        function: String,
    },
    /// Classification without successful admission fails closed to stock.
    Parse {
        path: PathBuf,
        detail: String,
    },
    InvalidSnapshots {
        detail: String,
    },
}

impl Reason {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InternalError { .. } => "internal-error",
            Self::Manifest { .. } => "manifest",
            Self::FileSet { .. } => "file-set",
            Self::Layout { .. } => "layout",
            Self::Async { .. } => "async",
            Self::MacroBody { .. } => "macro-body",
            Self::NewComplexFn { .. } => "new-complex-fn",
            Self::Parse { .. } => "parse",
            Self::InvalidSnapshots { .. } => "invalid-snapshots",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Class {
    LiveEligible { changed_fns: Vec<ChangedFn> },
    Restart { reason: Reason },
    Stock { reason: Reason },
}

/// Hits still hash the supplied bytes, but do not parse or walk syntax.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
    pub retained_bytes: usize,
}

type Key = (PathBuf, blake3::Hash);
type Parsed = Result<Arc<Analysis>, String>;

/// Session-owned syntax cache. Only derived strings/sets are retained, no ASTs.
/// LRU eviction bounds historical versions by both entry count and byte weight.
pub struct Guard {
    cache: HashMap<Key, (Parsed, usize)>,
    order: VecDeque<Key>,
    stats: CacheStats,
    max_entries: usize,
    max_bytes: usize,
}

impl Default for Guard {
    fn default() -> Self {
        Self::with_cache_limits(128, 16 * 1024 * 1024)
    }
}

impl Guard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Zero for either limit disables retention (useful for cold benchmarks).
    pub fn with_cache_limits(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            cache: HashMap::new(),
            order: VecDeque::new(),
            stats: CacheStats::default(),
            max_entries,
            max_bytes,
        }
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.stats
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.order.clear();
        self.stats.entries = 0;
        self.stats.retained_bytes = 0;
    }

    fn analyze(&mut self, snapshot: &FileSnapshot) -> Parsed {
        let key = (snapshot.path.clone(), blake3::hash(&snapshot.bytes));
        if let Some((result, _)) = self.cache.get(&key) {
            let result = result.clone();
            self.stats.hits += 1;
            // Keep frequently reused baseline files through long edit sessions.
            if let Some(position) = self.order.iter().position(|entry| entry == &key) {
                self.order.remove(position);
            }
            self.order.push_back(key);
            return result;
        }
        self.stats.misses += 1;
        let result = Analysis::parse(&snapshot.bytes).map(Arc::new);
        let weight = match &result {
            Ok(value) => value.weight(),
            Err(error) => error.capacity() + 128,
        } + 2 * snapshot.path.as_os_str().len()
            + 256;
        if self.max_entries != 0 && weight <= self.max_bytes {
            while self.cache.len() >= self.max_entries
                || self.stats.retained_bytes + weight > self.max_bytes
            {
                let Some(oldest) = self.order.pop_front() else {
                    break;
                };
                if let Some((_, bytes)) = self.cache.remove(&oldest) {
                    self.stats.retained_bytes -= bytes;
                }
            }
            self.cache.insert(key.clone(), (result.clone(), weight));
            self.order.push_back(key);
            self.stats.retained_bytes += weight;
            self.stats.entries = self.cache.len();
        }
        result
    }

    pub fn admit(&mut self, before: &[FileSnapshot], after: &[FileSnapshot]) -> Admission {
        self.contain_panic(|guard| admission::admit(guard, before, after))
            .unwrap_or_else(|detail| Admission::Reject {
                rule: AdmissionRule::InternalError,
                detail,
            })
    }

    // A parser/visitor bug must never unwind out of the controller worker.
    // Invalidate partial cache work and retain the payload in the fallback log.
    fn contain_panic<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> Result<T, String> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))) {
            Ok(value) => Ok(value),
            Err(payload) => {
                self.clear_cache();
                let detail = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("non-string panic payload")
                    .to_owned();
                eprintln!("refresh-guard internal error: {detail}; restarting with stock Cargo");
                Err(detail)
            }
        }
    }

    /// Reason priority is global across the change set, deterministic in lexical
    /// path/function order. Configuration changes outrank everything else.
    pub fn classify(&mut self, before: &[FileSnapshot], after: &[FileSnapshot]) -> Class {
        self.contain_panic(|guard| guard.classify_inner(before, after, false, &mut Vec::new()))
            .unwrap_or_else(|detail| Class::Restart {
                reason: Reason::InternalError { detail },
            })
    }

    /// Defer same-header async body decisions to compiler proofs. Every returned
    /// owner must be covered before the controller may choose the live rung.
    /// File sets, layouts, attributes and non-async policy remain strict.
    pub fn classify_with_async_certificates(
        &mut self,
        before: &[FileSnapshot],
        after: &[FileSnapshot],
    ) -> (Class, Vec<ChangedFn>) {
        self.contain_panic(|guard| guard.classify_certificates_inner(before, after))
            .unwrap_or_else(|detail| {
                (
                    Class::Restart {
                        reason: Reason::InternalError { detail },
                    },
                    Vec::new(),
                )
            })
    }

    fn classify_certificates_inner(
        &mut self,
        before: &[FileSnapshot],
        after: &[FileSnapshot],
    ) -> (Class, Vec<ChangedFn>) {
        let (before, _) = async_methods::view(before);
        let (after, methods) = async_methods::view(after);
        let mut owners = Vec::new();
        let class = self.classify_inner(&before, &after, true, &mut owners);
        for owner in &mut owners {
            if let Some(&(first, last)) = methods.get(&(owner.path.clone(), owner.name.clone())) {
                let line = if first == last {
                    first.to_string()
                } else {
                    format!("{first}-{last}")
                };
                owner.name = format!("@line:{line}{}", owner.name);
            }
        }
        (class, owners)
    }

    fn classify_inner(
        &mut self,
        before: &[FileSnapshot],
        after: &[FileSnapshot],
        certificates: bool,
        owners: &mut Vec<ChangedFn>,
    ) -> Class {
        let (old, new) = match snapshots(before, after) {
            Ok(pair) => pair,
            Err(detail) => {
                return Class::Stock {
                    reason: Reason::InvalidSnapshots { detail },
                };
            }
        };
        for path in old
            .keys()
            .chain(new.keys())
            .copied()
            .collect::<BTreeSet<_>>()
        {
            if !is_rust(path) && old.get(path).map(|f| &f.bytes) != new.get(path).map(|f| &f.bytes)
            {
                return Class::Stock {
                    reason: Reason::Manifest {
                        path: path.to_path_buf(),
                    },
                };
            }
        }
        let added: Vec<_> = new
            .keys()
            .filter(|p| is_rust(p) && !old.contains_key(*p))
            .map(|p| p.to_path_buf())
            .collect();
        let removed: Vec<_> = old
            .keys()
            .filter(|p| is_rust(p) && !new.contains_key(*p))
            .map(|p| p.to_path_buf())
            .collect();
        if !added.is_empty() || !removed.is_empty() {
            return Class::Restart {
                reason: Reason::FileSet { added, removed },
            };
        }
        let mut pairs = Vec::new();
        for (path, snapshot) in &old {
            if !is_rust(path) {
                continue;
            }
            // Parse even unchanged supplied files when admission was bypassed.
            let a = match self.analyze(snapshot) {
                Ok(value) => value,
                Err(detail) => {
                    return Class::Stock {
                        reason: Reason::Parse {
                            path: path.to_path_buf(),
                            detail,
                        },
                    };
                }
            };
            let b = match self.analyze(new[path]) {
                Ok(value) => value,
                Err(detail) => {
                    return Class::Stock {
                        reason: Reason::Parse {
                            path: path.to_path_buf(),
                            detail,
                        },
                    };
                }
            };
            pairs.push((path.to_path_buf(), a, b));
        }
        for (path, a, b) in &pairs {
            if (a.structure != b.structure && !b.only_body_imports_added(a))
                || a.hazards != b.hazards
            {
                return Class::Restart {
                    reason: Reason::Layout { path: path.clone() },
                };
            }
        }
        for (path, a, b) in &pairs {
            for (name, function) in &a.functions {
                let other = b.functions.get(name);
                if (!function.async_shape.is_empty()
                    || other.is_some_and(|f| !f.async_shape.is_empty()))
                    && other.map(|f| &f.async_shape) != Some(&function.async_shape)
                {
                    if certificates
                        && other.is_some_and(|other| {
                            function.header == other.header
                                && !function.async_shape.is_empty()
                                && !other.async_shape.is_empty()
                        })
                    {
                        owners.push(ChangedFn {
                            path: path.clone(),
                            name: name.clone(),
                        });
                        continue;
                    }
                    return Class::Restart {
                        reason: Reason::Async {
                            path: path.clone(),
                            function: name.clone(),
                        },
                    };
                }
            }
        }
        for (path, a, b) in &pairs {
            for (name, function) in &a.functions {
                if certificates
                    && owners
                        .iter()
                        .any(|owner| &owner.path == path && &owner.name == name)
                {
                    continue;
                }
                if b.functions
                    .get(name)
                    .is_none_or(|other| !function.same_policy(other))
                {
                    return Class::Restart {
                        reason: Reason::MacroBody {
                            path: path.clone(),
                            function: name.clone(),
                        },
                    };
                }
            }
        }
        for (path, a, b) in &pairs {
            for (name, function) in &b.functions {
                if !a.functions.contains_key(name)
                    && (!function.simple || !function.hazards.is_empty())
                {
                    return Class::Restart {
                        reason: Reason::NewComplexFn {
                            path: path.clone(),
                            function: name.clone(),
                        },
                    };
                }
            }
        }
        let mut changed_fns = Vec::new();
        for (path, a, b) in &pairs {
            for (name, function) in &b.functions {
                if a.functions
                    .get(name)
                    .is_none_or(|old| old.body != function.body || old.header != function.header)
                {
                    changed_fns.push(ChangedFn {
                        path: path.clone(),
                        name: name.clone(),
                    });
                }
            }
        }
        Class::LiveEligible { changed_fns }
    }
}

type Snapshots<'a> = BTreeMap<&'a Path, &'a FileSnapshot>;

fn snapshots<'a>(
    before: &'a [FileSnapshot],
    after: &'a [FileSnapshot],
) -> Result<(Snapshots<'a>, Snapshots<'a>), String> {
    fn index(files: &[FileSnapshot]) -> Result<Snapshots<'_>, String> {
        let mut result = BTreeMap::new();
        for file in files {
            if result.insert(file.path.as_path(), file).is_some() {
                return Err(format!("duplicate snapshot path: {}", file.path.display()));
            }
        }
        Ok(result)
    }
    Ok((index(before)?, index(after)?))
}

fn is_rust(path: &Path) -> bool {
    // Build scripts are configuration inputs even though they have a .rs suffix.
    path.extension().is_some_and(|ext| ext == "rs")
        && path.file_name().is_none_or(|name| name != "build.rs")
}

thread_local! { static DEFAULT_GUARD: RefCell<Guard> = RefCell::new(Guard::default()); }

pub fn admit(before: &[FileSnapshot], after: &[FileSnapshot]) -> Admission {
    DEFAULT_GUARD.with_borrow_mut(|guard| guard.admit(before, after))
}

pub fn classify(before: &[FileSnapshot], after: &[FileSnapshot]) -> Class {
    DEFAULT_GUARD.with_borrow_mut(|guard| guard.classify(before, after))
}

/// Macro argument token streams can contain async bodies invisible to the
/// ordinary AST visitor. Their coroutine identities are not yet safe to reuse
/// when macro expansion changes the number of bodies (e.g. vec repeat/list).
/// Only use this fence for a changed opaque macro body, not outer async code.
pub fn has_async_macro_arguments(bytes: &[u8]) -> bool {
    use proc_macro2::{TokenStream, TokenTree};
    fn contains_async(tokens: TokenStream) -> bool {
        tokens.into_iter().any(|token| match token {
            TokenTree::Ident(ident) => ident == "async",
            TokenTree::Group(group) => contains_async(group.stream()),
            _ => false,
        })
    }
    fn scan(tokens: TokenStream) -> bool {
        let mut bang = false;
        for token in tokens {
            match token {
                TokenTree::Group(group) => {
                    if (bang && contains_async(group.stream())) || scan(group.stream()) {
                        return true;
                    }
                    bang = false;
                }
                TokenTree::Punct(punct) => bang = punct.as_char() == '!',
                _ => bang = false,
            }
        }
        false
    }
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<TokenStream>().ok())
        .is_some_and(scan)
}

/// Token-stringifying macros whose exact expansion remains unqualified.
/// `assert!` uses rustc's AST expression printer, whereas `stringify!` uses its
/// token printer. Until the AST printer is ported, even startup code containing
/// these macros must use stock rustc. Scan token trees, including macro_rules
/// bodies, so a wrapper cannot hide a use from the controller's baseline fence.
/// The controller caches this per file; this function never reads the filesystem.
pub fn requires_stock_stringification(bytes: &[u8]) -> bool {
    fn scan(tokens: proc_macro2::TokenStream) -> bool {
        let mut previous = false;
        for token in tokens {
            match token {
                proc_macro2::TokenTree::Group(group) => {
                    if scan(group.stream()) {
                        return true;
                    }
                    previous = false;
                }
                proc_macro2::TokenTree::Ident(ident) => {
                    let name = ident.to_string();
                    // An imported alias can hide the spelling at the call site.
                    // Conservatively fence `use ...::assert as alias` as well.
                    if previous && name == "as" {
                        return true;
                    }
                    previous = matches!(name.trim_start_matches("r#"), "assert" | "debug_assert");
                }
                proc_macro2::TokenTree::Punct(punct) => {
                    if previous && punct.as_char() == '!' {
                        return true;
                    }
                    previous = false;
                }
                _ => previous = false,
            }
        }
        false
    }
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<proc_macro2::TokenStream>().ok())
        .is_some_and(scan)
}

#[cfg(test)]
mod containment_tests {
    use super::*;

    #[test]
    fn internal_panic_clears_cache_and_guard_remains_usable() {
        let source = [FileSnapshot::new(
            "main.rs",
            b"fn f() -> i32 { 1 }".to_vec(),
        )];
        let mut guard = Guard::new();
        assert!(matches!(
            guard.classify(&source, &source),
            Class::LiveEligible { .. }
        ));
        assert!(guard.cache_stats().entries > 0);
        let result: Result<(), _> = guard.contain_panic(|_| panic!("injected visitor failure"));
        assert_eq!(result.unwrap_err(), "injected visitor failure");
        assert_eq!(guard.cache_stats().entries, 0);
        assert!(matches!(
            guard.classify(&source, &source),
            Class::LiveEligible { .. }
        ));
    }
}
