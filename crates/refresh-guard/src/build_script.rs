//! Startup-only proof for excluded-target saves in packages with build scripts.
//! Unknown scripts may read arbitrary package files without declaring inputs.

/// Prove a build script has an empty main after simple Cargo feature cfgs.
/// Everything beyond this tiny language stays opaque and requires Cargo.
pub fn empty_build_script(text: &str, features: &[String]) -> bool {
    fn disabled(attrs: &[syn::Attribute], features: &[String]) -> bool {
        attrs.iter().any(|attr| {
            if !attr.path().is_ident("cfg") {
                return false;
            }
            let Ok(syn::Meta::NameValue(value)) = attr.parse_args::<syn::Meta>() else {
                return false;
            };
            if !value.path.is_ident("feature") {
                return false;
            }
            let syn::Expr::Lit(literal) = value.value else {
                return false;
            };
            let syn::Lit::Str(name) = literal.lit else {
                return false;
            };
            !features.contains(&name.value())
        })
    }
    let Ok(file) = syn::parse_file(text) else {
        return false;
    };
    if !file.attrs.is_empty() {
        return false;
    }
    let mut main = false;
    for item in file.items {
        match item {
            syn::Item::Mod(module) if disabled(&module.attrs, features) => {}
            syn::Item::Fn(function) if disabled(&function.attrs, features) => {}
            syn::Item::Fn(function)
                if function.sig.ident == "main" && function.attrs.is_empty() =>
            {
                if main || function.sig.asyncness.is_some() || function.sig.unsafety.is_some() {
                    return false;
                }
                main = true;
                for statement in &function.block.stmts {
                    let attrs = match statement {
                        syn::Stmt::Expr(syn::Expr::Call(call), _) => &call.attrs,
                        syn::Stmt::Expr(syn::Expr::MethodCall(call), _) => &call.attrs,
                        _ => return false,
                    };
                    if !disabled(attrs, features) {
                        return false;
                    }
                }
            }
            _ => return false,
        }
    }
    main
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_empty_or_disabled_build_work_is_proven() {
        assert!(empty_build_script("fn main() {}", &[]));
        let conditional = "fn main() { #[cfg(feature = \"dashboard\")] dashboard::build().unwrap(); } #[cfg(feature = \"dashboard\")] mod dashboard { pub fn build() {} }";
        assert!(empty_build_script(conditional, &[]));
        assert!(!empty_build_script(conditional, &["dashboard".into()]));
        for text in [
            "fn main() { std::fs::read(\"tests/input.rs\").unwrap(); }",
            "fn main() { #[cfg(unix)] read(); }",
            "#[ctor] fn before() {} fn main() {}",
            "static DATA: &str = include_str!(\"tests/input.rs\"); fn main() {}",
            "invalid",
        ] {
            assert!(!empty_build_script(text, &[]), "{text}");
        }
    }
}
