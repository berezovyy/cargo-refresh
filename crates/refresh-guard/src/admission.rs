use crate::{Admission, AdmissionRule, FileSnapshot, Guard, is_rust, snapshots};
use std::collections::BTreeSet;
use syn::visit::{self, Visit};

/// EXP-119's sets intentionally use unqualified owner names, not resolution.
#[derive(Default, Debug, Eq, PartialEq)]
pub(crate) struct Names {
    functions: BTreeSet<String>,
    calls: BTreeSet<String>,
    identifiers: BTreeSet<String>,
    references: BTreeSet<String>,
    bad_returns: BTreeSet<String>,
    expression_sites: BTreeSet<(String, String)>,
    callee: bool,
    owner: String,
}

impl Names {
    pub(crate) fn merge(&mut self, other: Self) {
        self.functions.extend(other.functions);
        self.calls.extend(other.calls);
        self.identifiers.extend(other.identifiers);
        self.references.extend(other.references);
        self.bad_returns.extend(other.bad_returns);
        self.expression_sites.extend(other.expression_sites);
    }

    fn check_body(&mut self, name: String, output: &syn::ReturnType, block: &syn::Block) {
        let nonunit = match output {
            syn::ReturnType::Default => false,
            syn::ReturnType::Type(_, ty) => {
                !matches!(&**ty, syn::Type::Tuple(t) if t.elems.is_empty())
            }
        };
        let tail = matches!(block.stmts.last(), Some(syn::Stmt::Expr(_, None)));
        let explicit_return = matches!(
            block.stmts.last(),
            Some(syn::Stmt::Expr(syn::Expr::Return(_), _))
        );
        if nonunit && !tail && !explicit_return {
            self.bad_returns.insert(name);
        }
    }

    pub(crate) fn weight(&self) -> usize {
        self.functions
            .iter()
            .chain(&self.calls)
            .chain(&self.identifiers)
            .chain(&self.references)
            .chain(&self.bad_returns)
            .map(|s| s.capacity() + 128)
            .sum::<usize>()
            + self
                .expression_sites
                .iter()
                .map(|(a, b)| a.capacity() + b.capacity() + 192)
                .sum::<usize>()
    }

    fn rejection(&self, old: &Self, rule: AdmissionRule) -> Vec<String> {
        match rule {
            AdmissionRule::IncompleteExpression => {
                let known: BTreeSet<_> = old.identifiers.union(&self.functions).collect();
                self.expression_sites
                    .difference(&old.expression_sites)
                    .filter(|(_, name)| {
                        !self.functions.contains(name)
                            && known
                                .iter()
                                .any(|known| **known != *name && known.starts_with(name))
                    })
                    .map(|(_, name)| name.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            }
            AdmissionRule::MissingReturnTail => self
                .bad_returns
                .difference(&old.bad_returns)
                .cloned()
                .collect(),
            AdmissionRule::BareFunctionReference => self
                .references
                .difference(&old.references)
                .filter(|name| self.functions.contains(*name))
                .cloned()
                .collect(),
            AdmissionRule::UnknownCall => self
                .calls
                .difference(&old.calls)
                .filter(|name| !self.functions.contains(*name))
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }
}

impl<'ast> Visit<'ast> for Names {
    fn visit_ident(&mut self, id: &'ast syn::Ident) {
        self.identifiers.insert(id.to_string());
    }
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        self.functions.insert(item.sig.ident.to_string());
        self.check_body(item.sig.ident.to_string(), &item.sig.output, &item.block);
        let previous = std::mem::replace(&mut self.owner, item.sig.ident.to_string());
        visit::visit_item_fn(self, item);
        self.owner = previous;
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        self.check_body(item.sig.ident.to_string(), &item.sig.output, &item.block);
        let previous = std::mem::replace(&mut self.owner, item.sig.ident.to_string());
        visit::visit_impl_item_fn(self, item);
        self.owner = previous;
    }
    fn visit_expr_path(&mut self, item: &'ast syn::ExprPath) {
        if item.qself.is_none() && item.path.segments.len() == 1 {
            let name = item.path.segments[0].ident.to_string();
            self.expression_sites
                .insert((self.owner.clone(), name.clone()));
            if !self.callee {
                self.references.insert(name);
            }
        }
        visit::visit_expr_path(self, item);
    }
    fn visit_expr_call(&mut self, item: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = &*item.func
            && path.qself.is_none()
            && path.path.segments.len() == 1
        {
            self.calls.insert(path.path.segments[0].ident.to_string());
        }
        let previous = self.callee;
        self.callee = true;
        self.visit_expr(&item.func);
        self.callee = false;
        for arg in &item.args {
            self.visit_expr(arg);
        }
        self.callee = previous;
    }
}

pub(crate) fn admit(
    guard: &mut Guard,
    before: &[FileSnapshot],
    after: &[FileSnapshot],
) -> Admission {
    let (old, new) = match snapshots(before, after) {
        Ok(pair) => pair,
        Err(detail) => {
            return Admission::Reject {
                rule: AdmissionRule::Parse,
                detail,
            };
        }
    };
    let mut pairs = Vec::new();
    // Parse the whole supplied Rust scope before running any heuristic rule.
    for path in old
        .keys()
        .chain(new.keys())
        .copied()
        .collect::<BTreeSet<_>>()
    {
        if !is_rust(path) {
            continue;
        }
        let mut parse = |file: Option<&&FileSnapshot>| match file {
            Some(file) => guard.analyze(file),
            None => guard.analyze(&FileSnapshot::new(path, Vec::new())),
        };
        let a = match parse(old.get(path)) {
            Ok(value) => value,
            Err(error) => {
                return Admission::Reject {
                    rule: AdmissionRule::Parse,
                    detail: format!("{} (before): {error}", path.display()),
                };
            }
        };
        let b = match parse(new.get(path)) {
            Ok(value) => value,
            Err(error) => {
                return Admission::Reject {
                    rule: AdmissionRule::Parse,
                    detail: format!("{} (after): {error}", path.display()),
                };
            }
        };
        pairs.push((path, a, b));
    }
    for rule in [
        AdmissionRule::IncompleteExpression,
        AdmissionRule::MissingReturnTail,
        AdmissionRule::BareFunctionReference,
        AdmissionRule::UnknownCall,
        AdmissionRule::RemovedFunction,
    ] {
        for (path, a, b) in &pairs {
            let names: Vec<_> = if rule == AdmissionRule::RemovedFunction {
                // Rich outline keys include inline modules; impl methods excluded.
                a.functions
                    .keys()
                    .filter(|name| !b.functions.contains_key(*name))
                    .cloned()
                    .collect()
            } else {
                b.names.rejection(&a.names, rule)
            };
            if !names.is_empty() {
                return Admission::Reject {
                    rule,
                    detail: format!("{}: {}", path.display(), names.join(", ")),
                };
            }
        }
    }
    Admission::Admit
}
