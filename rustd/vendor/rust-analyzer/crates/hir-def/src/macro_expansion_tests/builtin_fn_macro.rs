//! Tests for `builtin_fn_macro.rs` from `hir_expand`.

use expect_test::expect;

use crate::macro_expansion_tests::check;

#[test]
fn test_column_expand() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! column {() => {}}

fn main() { column!(); }
"#,
        expect![[r#"
#[rustc_builtin_macro]
macro_rules! column {() => {}}

fn main() { 0u32; }
"#]],
    );
}

#[test]
fn test_asm_expand() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! asm {() => {}}
#[rustc_builtin_macro]
macro_rules! global_asm {() => {}}
#[rustc_builtin_macro]
macro_rules! naked_asm {() => {}}

global_asm! {
    ""
}

#[unsafe(naked)]
extern "C" fn foo() {
    naked_asm!("");
}

fn main() {
    let i: u64 = 3;
    let o: u64;
    unsafe {
        asm!(
            "mov {0}, {1}",
            "add {0}, 5",
            out(reg) o,
            in(reg) i,
        );
    }
}
"#,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! asm {() => {}}
#[rustc_builtin_macro]
macro_rules! global_asm {() => {}}
#[rustc_builtin_macro]
macro_rules! naked_asm {() => {}}

builtin #global_asm ("")

#[unsafe(naked)]
extern "C" fn foo() {
    builtin #naked_asm ("");
}

fn main() {
    let i: u64 = 3;
    let o: u64;
    unsafe {
        builtin #asm ("mov {0}, {1}", "add {0}, 5", out (reg)o, in (reg)i, );
    }
}
"##]],
    );
}

#[test]
fn test_line_expand() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! line {() => {}}

fn main() { line!() }
"#,
        expect![[r#"
#[rustc_builtin_macro]
macro_rules! line {() => {}}

fn main() { 0u32 }
"#]],
    );
}

#[test]
fn test_stringify_expand() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! stringify {() => {}}

fn main() {
    stringify!(
        a
        b
        c
    );
}
"#,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! stringify {() => {}}

fn main() {
    "a b c";
}
"##]],
    );
}

#[test]
fn test_stringify_spacing() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! stringify {() => {}}

fn main() {
    stringify!(0 as int! {...} != 0);
    stringify!(1 + 'a: loop { break 'a 1 } == 1);
    stringify!(E::U::<>>E::U::<u8>);
    stringify!(a +-b; x<<=y; &&z; {} { } {a} { a } {a } { a}; f::<'a>(); |&'a x| 1);
}
"#,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! stringify {() => {}}

fn main() {
    "0 as int! {...} != 0";
    "1 + \'a: loop { break \'a 1 } == 1";
    "E::U::<>>E::U::<u8>";
    "a +-b; x<<=y; &&z; {} {} {a} { a } {a} { a }; f::<\'a>(); |&\'a x| 1";
}
"##]],
    );
}

#[test]
fn test_env_expand() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! env {() => {}}

fn main() {
    env!("TEST_ENV_VAR");
    env!("TEST_ENV_VAR",);
    env!("TEST_ENV_VAR", "error");
    env!("TEST_ENV_VAR", "error",);
}
"#,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! env {() => {}}

fn main() {
    /* error: environment variable `TEST_ENV_VAR` not defined at compile time */"UNRESOLVED_ENV_VAR";
    /* error: environment variable `TEST_ENV_VAR` not defined at compile time */"UNRESOLVED_ENV_VAR";
    /* error: environment variable `TEST_ENV_VAR` not defined at compile time */"UNRESOLVED_ENV_VAR";
    /* error: environment variable `TEST_ENV_VAR` not defined at compile time */"UNRESOLVED_ENV_VAR";
}
"##]],
    );
}

#[test]
fn test_option_env_expand() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! option_env {() => {}}

fn main() {
    option_env!("TEST_ENV_VAR");
    option_env!("TEST_ENV_VAR",);
    option_env!("TEST_ENV_VAR", "invalid");
}
"#,
        expect![[r#"
#[rustc_builtin_macro]
macro_rules! option_env {() => {}}

fn main() {
    $crate::option::Option::None:: < &str>;
    $crate::option::Option::None:: < &str>;
    /* error: unexpected input */;
}
"#]],
    );
}

#[test]
fn test_file_expand() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! file {() => {}}

fn main() { file!(); }
"#,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! file {() => {}}

fn main() { "file"; }
"##]],
    );
}

#[test]
fn test_assert_expand() {
    check(
        r#"
//- minicore: assert
fn main() {
    assert!(true, "{} {:?}", arg1(a, b, c), arg2);
}
"#,
        expect![[r#"
fn main() {
     {
        if !(true ) {
            $crate::panic::panic_2021!("{} {:?}", arg1(a, b, c), arg2);
        }
    };
}
"#]],
    );
}

#[test]
fn test_assert_expand_2015() {
    check(
        r#"
//- minicore: assert
//- /main.rs edition:2015
fn main() {
    assert!(true, "{} {:?}", arg1(a, b, c), arg2);
}
"#,
        expect![[r#"
fn main() {
     {
        if !(true ) {
            $crate::panic!("{} {:?}", arg1(a, b, c), arg2);
        }
    };
}
"#]],
    );
}

#[test]
fn test_compile_error_expand() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! compile_error {
    ($msg:expr) => ({ /* compiler built-in */ });
    ($msg:expr,) => ({ /* compiler built-in */ })
}

// This expands to nothing (since it's in item position), but emits an error.
compile_error!("error, with an escaped quote: \"");
compile_error!(r"this is a raw string");
"#,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! compile_error {
    ($msg:expr) => ({ /* compiler built-in */ });
    ($msg:expr,) => ({ /* compiler built-in */ })
}

/* error: error, with an escaped quote: " */
/* error: this is a raw string */
"##]],
    );
}

#[test]
fn test_format_args_expand() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! format_args {
    ($fmt:expr) => ({ /* compiler built-in */ });
    ($fmt:expr, $($args:tt)*) => ({ /* compiler built-in */ })
}

fn main() {
    format_args!("{} {:?}", arg1(a, b, c), arg2);
}
"#,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! format_args {
    ($fmt:expr) => ({ /* compiler built-in */ });
    ($fmt:expr, $($args:tt)*) => ({ /* compiler built-in */ })
}

fn main() {
    builtin #format_args ("{} {:?}", arg1(a, b, c), arg2);
}
"##]],
    );
}

#[test]
fn regression_15002() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! format_args {
    ($fmt:expr) => ({ /* compiler built-in */ });
    ($fmt:expr, $($args:tt)*) => ({ /* compiler built-in */ })
}

fn main() {
    format_args!(x = 2);
    format_args!/*+errors*/(x =);
    format_args!/*+errors*/(x =, x = 2);
    format_args!/*+errors*/("{}", x =);
    format_args!/*+errors*/(=, "{}", x =);
    format_args!(x = 2, "{}", 5);
}
"#,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! format_args {
    ($fmt:expr) => ({ /* compiler built-in */ });
    ($fmt:expr, $($args:tt)*) => ({ /* compiler built-in */ })
}

fn main() {
    builtin #format_args (x = 2);
    /* parse error: expected expression */
builtin #format_args (x = );
    /* parse error: expected expression */
builtin #format_args (x = , x = 2);
    /* parse error: expected expression */
builtin #format_args ("{}", x = );
    /* parse error: expected expression */
/* parse error: expected expression */
builtin #format_args ( = , "{}", x = );
    builtin #format_args (x = 2, "{}", 5);
}
"##]],
    );
}

#[test]
fn test_format_args_expand_with_comma_exprs() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! format_args {
    ($fmt:expr) => ({ /* compiler built-in */ });
    ($fmt:expr, $($args:tt)*) => ({ /* compiler built-in */ })
}

fn main() {
    format_args!("{} {:?}", a::<A,B>(), b);
}
"#,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! format_args {
    ($fmt:expr) => ({ /* compiler built-in */ });
    ($fmt:expr, $($args:tt)*) => ({ /* compiler built-in */ })
}

fn main() {
    builtin #format_args ("{} {:?}", a::<A, B>(), b);
}
"##]],
    );
}

#[test]
fn test_format_args_expand_with_raw_strings() {
    check(
        r##"
#[rustc_builtin_macro]
macro_rules! format_args {
    ($fmt:expr) => ({ /* compiler built-in */ });
    ($fmt:expr, $($args:tt)*) => ({ /* compiler built-in */ })
}

fn main() {
    format_args!(
        r#"{},mismatch,"{}","{}""#,
        location_csv_pat(db, &analysis, vfs, &sm, pat_id),
        mismatch.expected.display(db),
        mismatch.actual.display(db)
    );
}
"##,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! format_args {
    ($fmt:expr) => ({ /* compiler built-in */ });
    ($fmt:expr, $($args:tt)*) => ({ /* compiler built-in */ })
}

fn main() {
    builtin #format_args (r#"{},mismatch,"{}","{}""#, location_csv_pat(db, &analysis, vfs, &sm, pat_id), mismatch.expected.display(db), mismatch.actual.display(db));
}
"##]],
    );
}

#[test]
fn test_format_args_expand_eager() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! concat {}

#[rustc_builtin_macro]
macro_rules! format_args {
    ($fmt:expr) => ({ /* compiler built-in */ });
    ($fmt:expr, $($args:tt)*) => ({ /* compiler built-in */ })
}

fn main() {
    format_args!(concat!("xxx{}y", "{:?}zzz"), 2, b);
}
"#,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! concat {}

#[rustc_builtin_macro]
macro_rules! format_args {
    ($fmt:expr) => ({ /* compiler built-in */ });
    ($fmt:expr, $($args:tt)*) => ({ /* compiler built-in */ })
}

fn main() {
    builtin #format_args (concat!("xxx{}y", "{:?}zzz"), 2, b);
}
"##]],
    );
}

#[test]
fn test_format_args_expand_with_broken_member_access() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! format_args {
    ($fmt:expr) => ({ /* compiler built-in */ });
    ($fmt:expr, $($args:tt)*) => ({ /* compiler built-in */ })
}

fn main() {
    let _ =
        format_args!/*+errors*/("{} {:?}", a.);
}
"#,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! format_args {
    ($fmt:expr) => ({ /* compiler built-in */ });
    ($fmt:expr, $($args:tt)*) => ({ /* compiler built-in */ })
}

fn main() {
    let _ =
        /* parse error: expected field name or number */
builtin #format_args ("{} {:?}", a.);
}
"##]],
    );
}

#[test]
fn test_include_bytes_expand() {
    check(
        r#"
#[rustc_builtin_macro]
macro_rules! include_bytes {
    ($file:expr) => {{ /* compiler built-in */ }};
    ($file:expr,) => {{ /* compiler built-in */ }};
}

fn main() { include_bytes("foo");include_bytes(r"foo"); }
"#,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! include_bytes {
    ($file:expr) => {{ /* compiler built-in */ }};
    ($file:expr,) => {{ /* compiler built-in */ }};
}

fn main() { include_bytes("foo");include_bytes(r"foo"); }
"##]],
    );
}

#[test]
fn test_concat_expand() {
    check(
        r##"
#[rustc_builtin_macro]
macro_rules! concat {}

fn main() { concat!("fo", "o", 0, r#""bar""#, "\n", false, '"', -4, - 4, '\0'); }
"##,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! concat {}

fn main() { "foo0\"bar\"\nfalse\"-4-4\u{0}"; }
"##]],
    );
}

#[test]
fn test_concat_bytes_expand() {
    check(
        r##"
#[rustc_builtin_macro]
macro_rules! concat_bytes {}

fn main() { concat_bytes!(b'A', b"BC\"", [68, b'E', 70], br#"G""#,b'\0'); }
"##,
        expect![[r#"
#[rustc_builtin_macro]
macro_rules! concat_bytes {}

fn main() { b"ABC\"DEFG\"\x00"; }
"#]],
    );
}

#[test]
fn test_concat_with_captured_expr() {
    check(
        r##"
#[rustc_builtin_macro]
macro_rules! concat {}

macro_rules! surprise {
    () => { "s" };
}

fn main() { concat!(surprise!()); }
"##,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! concat {}

macro_rules! surprise {
    () => { "s" };
}

fn main() { "s"; }
"##]],
    );
}

#[test]
fn test_quote_string() {
    check(
        r##"
#[rustc_builtin_macro]
macro_rules! stringify {}

fn main() { stringify!("hello"); }
"##,
        expect![[r##"
#[rustc_builtin_macro]
macro_rules! stringify {}

fn main() { "\"hello\""; }
"##]],
    );
}

#[test]
fn cfg_select() {
    check(
        r#"
#[rustc_builtin_macro]
pub macro cfg_select($($tt:tt)*) {}

cfg_select! {
    false => { fn false_1() {} }
    any(false, true) => { fn true_1() {} }
}

cfg_select! {
    false => { fn false_2() {} }
    _ => { fn true_2() {} }
}

const _: ((),) = cfg_select! { _ => ((), ) };
const _: i32 = cfg_select! { true => 2 + 3, _ => 3 + 4 };
const _: i32 = cfg_select! { false => 2 + 3, _ => 3 + 4 };
const _: bool = cfg_select! { _ => 2 < 3 };
const _: bool = cfg_select! { true => foo::<(), fn() -> Foo<i32, i64>>(1,), _ => false };

cfg_select! {
    false => { fn false_3() {} }
}

cfg_select! {
    false
}

cfg_select! {
    false =>
}

    "#,
        expect![[r#"
#[rustc_builtin_macro]
pub macro cfg_select($($tt:tt)*) {}

fn true_1() {}

fn true_2() {}

const _: ((),) = ((), );
const _: i32 = 2+3;
const _: i32 = 3+4;
const _: bool = 2<3;
const _: bool = foo::<(), fn() -> Foo<i32, i64>>(1, );

/* error: none of the predicates in this `cfg_select` evaluated to true */

/* error: expected `=>` after cfg expression */

/* error: expected a token tree after `=>` */

    "#]],
    );
}

#[test]
fn eager_recursion_limit() {
    check(
        r#"
//- minicore: concat

macro_rules! concat_separator {
    () => {
        concat!("", concat_separator!())
    };
}

fn main() {
    concat_separator!()
}
    "#,
        expect![[r#"

macro_rules! concat_separator {
    () => {
        concat!("", concat_separator!())
    };
}

fn main() {
    concat!("", concat_separator!())
}
    "#]],
    );
}

// Resolve through nested macro_rules expansions using the same body source map
// used by the normal macro expansion fixtures.
fn stringified_value(
    db: &crate::test_db::TestDB,
    def_map: &crate::nameres::DefMap,
    id: hir_expand::MacroCallId,
) -> String {
    use hir_expand::InFile;
    use syntax::{AstNode, AstPtr, AstToken, ast};
    let expansion = id.parse_macro_expansion(db);
    assert!(expansion.err.is_none(), "{:?}", expansion.err);
    let node = expansion.value.0.syntax_node();
    if let Some(call) = node.descendants().find_map(ast::MacroCall::cast) {
        let file = hir_expand::HirFileId::from(id);
        let ast_id = file.ast_id_map(db).ast_id(&call);
        let id = super::resolve_macro_call_id(
            db,
            def_map,
            InFile::new(file, ast_id),
            InFile::new(file, AstPtr::new(&call)),
        )
        .unwrap();
        return stringified_value(db, def_map, id);
    }
    node.descendants_with_tokens()
        .filter_map(|it| it.into_token())
        .find_map(ast::String::cast)
        .unwrap()
        .value()
        .unwrap()
        .into_owned()
}

/// Differential oracle for the printer contract of the qualified rustc version.
/// Run explicitly with CARGO_REFRESH_ORACLE_DIR on disk (never /tmp), using 1.99.
#[test]
#[ignore = "compiles a stock rustc 1.99 oracle; requires CARGO_REFRESH_ORACLE_DIR"]
fn stringify_rustc_199_equivalence_table() {
    use super::resolve_macro_call_id;
    use crate::{
        nameres::{ModuleSource, crate_def_map},
        test_db::TestDB,
    };
    use hir_expand::InFile;
    use std::{fmt::Write as _, fs, path::PathBuf, process::Command};
    use syntax::{AstNode, AstPtr, ast};
    use test_fixture::WithFixture;

    let dir = PathBuf::from(
        std::env::var_os("CARGO_REFRESH_ORACLE_DIR").expect("on-disk oracle directory"),
    );
    fs::create_dir_all(&dir).unwrap();
    let mut cases = Vec::<(String, String)>::new();
    for n in [1, 2, 3, 5, 8, 13, 21, 34, 55, 89, 120, 200] {
        let list = (0..n).map(|i| format!("token_{i:03}")).collect::<Vec<_>>().join(" + ");
        let commas = (0..n).map(|i| format!("arg_{i:03}")).collect::<Vec<_>>().join(", ");
        let templates = [
            list.clone(),
            format!("({list})"),
            format!("[{list}]"),
            format!("{{ {list} }}"),
            format!("{{{list}}}"),
            format!("f({commas})"),
            format!("foo::bar::<u8>({commas});"),
            format!("if true {{ {list} }} else {{ {list} }}"),
            format!("{{ {{ {{ {list} }} }} }}"),
            format!("fn function<'a>(x: &'a str) -> &'a str {{ {list}; x }}"),
            format!("move |x: i32, y: i32| {{ {list}; x + y }}"),
            format!("#[allow(dead_code)] struct S {{ {commas} }}"),
            format!("r#type r#match r#fn; {list}"),
            format!("\"escaped\\n\\\"text\" r###\"raw \" text\"### b\"bytes\" c\"cstr\"; {list}"),
            format!("'a 'static r#ident 'z: loop {{ break 'z; {list} }}"),
            format!("a..b a .. b a...b a ... b a..=b a ..= b; {list}"),
            format!("x<<=1; x >>= 2; a::b -> c => d && e || f; {list}"),
            format!("{{ /// line documentation\n{list} }}"),
            format!("{{ /** block documentation */ {list} }}"),
            format!("{{ //! inner documentation\n{list} }}"),
            format!("{{ /*! inner block documentation */ {list} }}"),
            format!("#[doc = \"ordinary doc attribute\"] {list}"),
            format!("{{ {list}; {{ {list} }} }}"),
            format!("[({list}), {{{list}}}, [{{ {list} }}]]"),
        ];
        for (kind, text) in templates.into_iter().enumerate() {
            cases.push((format!("kind-{kind:02}-n-{n:03}"), text));
        }
    }
    // Punctuation and source-adjacency boundaries, including complete rustc
    // operator tokens that are several punct leaves in rust-analyzer.
    for op in [
        "::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "..", "...", "..=", "<<", ">>",
        "<<=", ">>=", "+=", "-=", "*=", "/=", "%=", "^=", "&=", "|=", "<-", ".", ",", ";", "!",
        "#", "$", ":", "'a",
    ] {
        for spaces in [false, true] {
            let text = if spaces {
                format!("before {op} after")
            } else if op == "#" || op.starts_with('\'') {
                format!("before {op}after")
            } else {
                format!("before{op}after")
            };
            cases.push((format!("operator-{op}-{spaces}"), text));
        }
    }
    for name in [
        "ident", "r#type", "fn", "pub", "Self", "self", "if", "let", "const", "async", "await",
        "union", "true", "macro", "gen", "dyn", "try", "_",
    ] {
        for suffix in [" ! ()", " (a, b)", " ! = a"] {
            cases.push((format!("keyword-{name}-{suffix}"), format!("{name}{suffix}")));
        }
    }
    for (_, tokens) in &mut cases {
        *tokens = format!("stringify!({tokens})");
    }
    let wrappers = r#"
macro_rules! via_tt { ($($t:tt)*) => { stringify!($($t)*) }; }
macro_rules! via_expr { ($e:expr) => { stringify!($e) }; }
macro_rules! via_ident { ($i:ident) => { stringify!($i+1 $i::suffix $i.foo $i!() $i()) }; }
macro_rules! forwarded_ident { ($i:ident) => { via_tt!($i+1 $i::suffix $i.foo) }; }
macro_rules! via_lifetime { ($l:lifetime) => { stringify!($l: loop { break $l; }) }; }
macro_rules! via_path { ($p:path) => { stringify!($p) }; }
macro_rules! via_ty { ($t:ty) => { stringify!($t) }; }
macro_rules! via_stmt { ($($s:stmt)*) => { stringify!($($s |)*) }; }
macro_rules! via_literal { ($l:literal) => { stringify!($l+1) }; }
macro_rules! via_block { ($b:block) => { stringify!($b+1) }; }
macro_rules! via_pat { ($p:pat) => { stringify!($p=>suffix) }; }
macro_rules! via_meta { ($m:meta) => { stringify!($m+1) }; }
macro_rules! via_vis { ($v:vis ,) => { stringify!($v fn f()) }; }
macro_rules! via_item { ($i:item) => { stringify!($i trailing) }; }
macro_rules! via_dollar { ($d:tt $i:ident) => { stringify!($d $i) }; }
macro_rules! generated_crate { () => { stringify!($crate ! () $crate(x)) }; }
macro_rules! generated_doc { () => { stringify!(/// docs
x) }; }
macro_rules! generated { () => { stringify!(hello+world (1+2) [x+y] {a+b}) }; }
"#;
    for n in [1, 2, 3, 8, 13, 21, 34, 55, 89, 120] {
        for sep in ["+", " + ", " +", "+ "] {
            let text = (0..n).map(|i| format!("value{i}")).collect::<Vec<_>>().join(sep);
            cases.push((format!("tt-{n}-{sep:?}"), format!("via_tt!({text})")));
            cases.push((format!("expr-{n}-{sep:?}"), format!("via_expr!({text})")));
        }
        cases.push((format!("ident-{n}"), format!("via_ident!(identifier{n})")));
        cases.push((format!("lifetime-{n}"), format!("via_lifetime!('lifetime{n})")));
    }
    cases.push(("generated".into(), "generated!()".into()));
    for expression in [
        "generated_doc!()",
        "generated_crate!()",
        "via_literal!(1)",
        "via_literal!(true)",
        "via_literal!(-1)",
        "via_literal!(1.25f32)",
        "via_literal!(r#\"text\"#)",
        "via_block!({x+y})",
        "via_pat!(Some(x))",
        "via_meta!(allow(dead_code))",
        "via_vis!(pub(crate),)",
        "via_vis!(,)",
        "via_item!(fn f() {})",
        "via_dollar!($ identifier)",
    ] {
        cases.push((format!("fragment-{expression}"), expression.into()));
    }

    for ident in ["alpha", "r#type", "r#match", "fn", "self", "Self", "pub", "async"] {
        for mac in ["via_ident", "forwarded_ident"] {
            cases.push((format!("{mac}-{ident}"), format!("{mac}!({ident})")));
        }
    }
    for tokens in [
        "",
        "{}",
        "()",
        "[]",
        "a /* ordinary */ + b",
        "/// docs\nx",
        "/** docs */ x",
        "/** docs */x",
        "//! docs\nx",
        "/*! docs */x",
        "{/// docs\nx}",
        "{/** docs */ x}",
        "'a+'b",
        "r#ident::name",
        "1_000u64 0xff_u8 1.0e+12f64",
        "'\\n' b'\\x7f'",
        "{ a+b;c*d; }",
        "#[attr(key = \"value\")] fn f() {}",
    ] {
        cases.push((format!("edge-direct-{tokens:?}"), format!("stringify!({tokens})")));
        cases.push((format!("edge-tt-{tokens:?}"), format!("via_tt!({tokens})")));
    }

    for expression in [
        "via_path!(Vec<u8>)",
        "via_path!(std::vec::Vec<u8>)",
        "via_ty!(Vec<u8>)",
        "via_ty!(&'a mut [u8])",
        "via_stmt!(let x=1; x+2;)",
        "via_tt!(/// quote \" text #\nx)",
        "via_tt!(/** quote \"# text */ x)",
    ] {
        cases.push((format!("boundary-{expression:?}"), expression.into()));
    }
    for n in 0..40 {
        cases.push((format!("concat-numeric-{n}"), format!("concat!({n}_000u64, \"|\", 0x{n:x}u128, \"|\", 0o{n:o}, \"|\", 0b{n:b}, \"|\", -0x{n:x}i128, \"|\", {n}_000.5_0e+02f64)")));
    }
    let root_dir = dir;
    for edition in [2015, 2018, 2021, 2024] {
        let dir = root_dir.join(edition.to_string());
        fs::create_dir_all(&dir).unwrap();
        let mut oracle = String::from(
            "fn emit(s: &str) { for b in s.bytes() { print!(\"{b:02x}\"); } println!(); }\nfn main() {\n",
        );
        let mut fixture = String::from(
            "#[rustc_builtin_macro]\nmacro_rules! stringify { () => {} }\n#[rustc_builtin_macro]\nmacro_rules! concat { () => {} }\nfn main() {\n",
        );
        oracle.insert_str(0, wrappers);
        fixture.insert_str(0, wrappers);
        fixture.insert_str(0, &format!("//- /main.rs crate:test edition:{edition}\n"));
        for (_, expression) in &cases {
            writeln!(oracle, "emit({expression});").unwrap();
            writeln!(fixture, "{expression};").unwrap();
        }
        oracle.push_str("}\n");
        fixture.push_str("}\n");
        fs::write(dir.join("oracle.rs"), oracle).unwrap();
        let rustc = Command::new("rustc")
            .env("RUSTUP_TOOLCHAIN", "1.99.0")
            .arg(format!("--edition={edition}"))
            .args(["--crate-name", "stringify_oracle"])
            .arg(dir.join("oracle.rs"))
            .arg("-o")
            .arg(dir.join("oracle"))
            .output()
            .unwrap();
        assert!(rustc.status.success(), "{}", String::from_utf8_lossy(&rustc.stderr));
        let stock = Command::new(dir.join("oracle")).output().unwrap();
        assert!(stock.status.success());
        let stock = String::from_utf8(stock.stdout).unwrap();
        let stock = stock.lines().collect::<Vec<_>>();
        assert_eq!(stock.len(), cases.len());

        let db = TestDB::with_files(&fixture);
        let krate = db.fetch_test_crate();
        let def_map = crate_def_map(&db, krate);
        let source = def_map.module(&db, def_map.root).definition_source(&db);
        let ModuleSource::SourceFile(source_file) = source.value else { panic!() };
        let mut table = String::from("case\tinput_bytes\toutput_bytes\tequal\n");
        let mut differences = Vec::new();
        let calls =
            source_file.syntax().descendants().filter_map(ast::MacroCall::cast).collect::<Vec<_>>();
        assert_eq!(calls.len(), cases.len());
        for (index, call) in calls.into_iter().enumerate() {
            let ast_id = source.file_id.ast_id_map(&db).ast_id(&call);
            let id = resolve_macro_call_id(
                &db,
                def_map,
                InFile::new(source.file_id, ast_id),
                InFile::new(source.file_id, AstPtr::new(&call)),
            )
            .unwrap();
            let text = stringified_value(&db, def_map, id);
            let actual = text.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>();
            let equal = actual == stock[index];
            writeln!(
                table,
                "{}\t{}\t{}\t{equal}",
                cases[index].0,
                cases[index].1.len(),
                text.len()
            )
            .unwrap();
            if !equal {
                differences.push(format!(
                    "{}: input {:?}\nactual {:?}\nstock hex {}",
                    cases[index].0, cases[index].1, text, stock[index]
                ));
            }
        }
        fs::write(dir.join("equivalence.tsv"), table).unwrap();
        fs::write(dir.join("differences.txt"), differences.join("\n\n")).unwrap();
        assert!(
            differences.is_empty(),
            "{} / {} differences; first: {}",
            differences.len(),
            cases.len(),
            differences.first().unwrap_or(&String::new())
        );
        eprintln!("{} / {} byte-identical macro expansions", cases.len(), cases.len());
    }
}

/// Assert prints the parsed, unexpanded condition, unlike stringify's tokens.
/// Compare actual panic payload bytes from the qualified stock compiler.
#[test]
#[cfg(feature = "rustc-assert")]
#[ignore = "requires stock rustc 1.99 and on-disk CARGO_REFRESH_ORACLE_DIR"]
fn assertion_rustc_199_equivalence_table() {
    use super::resolve_macro_call_id;
    use crate::{
        nameres::{ModuleSource, crate_def_map},
        test_db::TestDB,
    };
    use hir_expand::{InFile, MacroCallId};
    use std::{fmt::Write as _, fs, path::PathBuf, process::Command};
    use syntax::{AstNode, AstPtr, AstToken, ast};
    use test_fixture::WithFixture;

    fn message(db: &TestDB, map: &crate::nameres::DefMap, id: MacroCallId) -> String {
        let expanded = id.parse_macro_expansion(db);
        assert!(expanded.err.is_none(), "{:?}", expanded.err);
        let node = expanded.value.0.syntax_node();
        if let Some(message) = node
            .descendants_with_tokens()
            .filter_map(|it| it.into_token())
            .filter_map(ast::String::cast)
            .filter_map(|it| it.value().ok().map(|s| s.into_owned()))
            .find(|it| it.starts_with("assertion failed: "))
        {
            return message;
        }
        let call = node.descendants().find_map(ast::MacroCall::cast).expect("wrapped assertion");
        let file = hir_expand::HirFileId::from(id);
        let ast_id = file.ast_id_map(db).ast_id(&call);
        let id = resolve_macro_call_id(
            db,
            map,
            InFile::new(file, ast_id),
            InFile::new(file, AstPtr::new(&call)),
        )
        .unwrap();
        message(db, map, id)
    }

    let directory = PathBuf::from(
        std::env::var_os("CARGO_REFRESH_ORACLE_DIR").expect("on-disk oracle directory"),
    );
    let support = r##"
#[derive(PartialEq)] struct S { value: i32 }
fn identity<T>(x: T) -> T { x }
macro_rules! flag { ($($t:tt)*) => { false }; }
macro_rules! via_expr { ($e:expr) => { assert!($e) }; }
macro_rules! via_tt { ($($t:tt)*) => { assert!($($t)*) }; }
macro_rules! via_path { ($p:path) => { assert!($p(0) != 0) }; }
macro_rules! via_ty { ($t:ty) => { assert!(core::mem::size_of::<$t>() == 0) }; }
macro_rules! via_block { ($b:block) => { assert!($b) }; }
macro_rules! via_ident { ($i:ident) => { assert!($i(0) != 0) }; }
macro_rules! via_arithmetic { ($e:expr) => { assert!($e*2==0) }; }
macro_rules! via_same { ($e:expr) => { assert!($e+3==0) }; }
macro_rules! via_macro { ($e:expr) => { assert!(flag!($e+1)) }; }
macro_rules! via_type_macro { ($t:ty) => { assert!(flag!($t)) }; }
macro_rules! via_path_macro { ($p:path) => { assert!(flag!($p)) }; }
macro_rules! forwarded { ($e:expr) => { via_arithmetic!($e) }; }

"##;
    let mut cases = Vec::<String>::new();
    for n in 1..=20 {
        for expression in [
            format!("{n}+2==0"),
            format!("({n} + 2)*3 == 0"),
            format!("{n}/* comment */ + /* comment */2==0"),
            format!("({n} < 0) && true || false"),
            format!("!({n} != 0)"),
            format!("({n} & 3 | 4 ^ 1) == 0"),
            format!("({n} << 2 >> 1) < 0"),
            format!("{n} as u64 == 0u64"),
            format!("const {{ {n} }} == 0"),
            format!("if let Some(x) = Some({n}) && x > 0 {{ false }} else {{ true }}"),
            format!("identity::<i32>({n}) == 0"),
            format!("Some({n}).map(|v| v+1).unwrap() == 0"),
            format!("(move |x: i32| {{ x+{n} }})(1) == 0"),
            format!("{{ let x={n}; x+1 }} == 0"),
            format!("unsafe {{ core::ptr::read(&{n}) }} == 0"),
            format!("{{ let _ = async {{ {n} }}; false }}"),
            format!("[1,2,{n}].iter().any(|x| *x < 0)"),
            format!("(0..{n}).contains(&-1)"),
            format!("(0..={n}).contains(&-1)"),
            format!("(S {{ value: {n} }}) == S {{ value: 0 }}"),
            format!("match Some({n}) {{ Some(x) => x==0, None => true }}"),
            format!("if {n} > 0 {{ false }} else {{ true }}"),
            format!("{{ let mut x={n}; x+=1; x==0 }}"),
            format!("{{ 'outer: loop {{ break 'outer {n} == 0; }} }}"),
            format!("{{ #[allow(unused_variables)] let x={n}; false }}"),
            format!("\"unicode λ🦀{n}\".is_empty()"),
            format!("r###\"raw \" quote {n}\"###.is_empty()"),
            format!("b\"bytes{n}\".is_empty()"),
            format!("br##\"raw{n}\"##.is_empty()"),
            format!("'λ' as u32 == {n}"),
            format!("b'Z' as u32 == {n}"),
            format!("flag!({n}+2; nested!(a,b); {{x+y}})"),
            format!("{{ let (a,b)=({n},2); a+b==0 }}"),
            format!("{{ let v=vec![{n};3]; v.len()==0 }}"),
            format!("({n},).0 == 0"),
            format!("[1,{n},3][1] == 0"),
            format!("(&{n}).to_string().is_empty()"),
            format!("{{ const C:i32={n}; C==0 }}"),
            format!("{{ let _ = stringify!({n}+2); false }}"),
            format!("{{ let r#type={n}; r#type==0 }}"),
            format!("{{ let _ = c\"abc{n}\"; false }}"),
            format!("{{ let _ = || -> i32 {{ {n} }}; false }}"),
        ] {
            cases.push(format!("assert!({expression})"));
        }
        cases.push(format!("via_expr!({n}+2==0)"));
        cases.push(format!("via_tt!({n}+2==0)"));
        cases.push(format!("via_block!({{ {n}==0 }})"));
        cases.push(format!("via_arithmetic!({n}+1)"));
        cases.push(format!("via_same!({n}+1)"));
        cases.push(format!("via_macro!({n}+1)"));
        cases.push(format!("forwarded!({n}+1)"));
    }
    for wrapper in [
        "via_path!(identity)",
        "via_ty!(u64)",
        "via_ident!(identity)",
        "via_type_macro!(Option<u8>)",
        "via_path_macro!(identity::<i32>)",
    ] {
        cases.push(wrapper.to_owned());
    }
    // Exercise the Oppen margin and nested macro token formatting.
    for n in [4, 8, 13, 21, 34, 55, 89, 120] {
        let sum = (1..=n).map(|i| i.to_string()).collect::<Vec<_>>().join("+");
        cases.push(format!("assert!({sum}==0)"));
        cases.push(format!("via_expr!({sum}==0)"));
        cases.push(format!("assert!(flag!({sum}))"));
    }
    for edition in [2015, 2018, 2021, 2024] {
        let cases = cases
            .iter()
            .filter(|s| edition != 2015 || !s.contains("async {"))
            .filter(|s| edition >= 2021 || !s.contains("c\"abc"))
            .filter(|s| edition == 2024 || !s.contains("if let Some"))
            .cloned()
            .collect::<Vec<_>>();
        let dir = directory.join(edition.to_string());
        fs::create_dir_all(&dir).unwrap();
        let mut stock = format!("#![allow(unused,unused_parens)]\n{support}\n");
        let mut fixture = format!(
            "//- /main.rs crate:test edition:{edition}\n#[rustc_builtin_macro] macro_rules! assert {{ () => {{}} }}\n{support}\n"
        );
        for (i, expression) in cases.iter().enumerate() {
            writeln!(stock, "fn case_{i}() {{ {expression}; }}").unwrap();
            writeln!(fixture, "fn case_{i}() {{ {expression}; }}").unwrap();
        }
        stock.push_str("fn main() { std::panic::set_hook(Box::new(|_| {}));\n");
        for i in 0..cases.len() {
            writeln!(stock,"let p=std::panic::catch_unwind(case_{i}).expect_err(\"must fail\"); let s=p.downcast_ref::<&str>().copied().or_else(|| p.downcast_ref::<String>().map(String::as_str)).unwrap(); for b in s.bytes() {{ print!(\"{{b:02x}}\"); }} println!();").unwrap();
        }
        stock.push_str("}\n");
        fs::write(dir.join("oracle.rs"), stock).unwrap();
        let output = Command::new("rustc")
            .env("RUSTUP_TOOLCHAIN", "1.99.0")
            .arg(format!("--edition={edition}"))
            .arg(dir.join("oracle.rs"))
            .arg("-o")
            .arg(dir.join("oracle"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let output = Command::new(dir.join("oracle")).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stderr.is_empty());
        let expected = String::from_utf8(output.stdout).unwrap();
        let expected = expected.lines().collect::<Vec<_>>();
        assert_eq!(expected.len(), cases.len());
        let db = TestDB::with_files(&fixture);
        let map = crate_def_map(&db, db.fetch_test_crate());
        let source = map.module(&db, map.root).definition_source(&db);
        let ModuleSource::SourceFile(file) = source.value else { panic!() };
        let calls = file
            .syntax()
            .children()
            .filter_map(ast::Fn::cast)
            .filter_map(|f| f.syntax().descendants().find_map(ast::MacroCall::cast))
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), cases.len());
        let mut table = String::new();
        let mut differences = Vec::new();
        for (i, call) in calls.into_iter().enumerate() {
            let ast_id = source.file_id.ast_id_map(&db).ast_id(&call);
            let id = resolve_macro_call_id(
                &db,
                map,
                InFile::new(source.file_id, ast_id),
                InFile::new(source.file_id, AstPtr::new(&call)),
            )
            .unwrap();
            let actual = message(&db, map, id);
            let hex = actual.bytes().map(|b| format!("{b:02x}")).collect::<String>();
            let equal = hex == expected[i];
            writeln!(table, "{i}\t{equal}\t{}\t{actual:?}", cases[i]).unwrap();
            if !equal {
                differences.push(format!(
                    "case {i} {}\nactual: {actual:?}\nexpected hex: {}",
                    cases[i], expected[i]
                ));
            }
        }
        fs::write(dir.join("table.tsv"), table).unwrap();
        fs::write(dir.join("differences.txt"), differences.join("\n\n")).unwrap();
        assert!(
            differences.is_empty(),
            "{edition}: {} differences; {}",
            differences.len(),
            differences.first().unwrap_or(&String::new())
        );
        eprintln!(
            "assertions edition {edition}: {}/{} byte-identical panic messages",
            cases.len(),
            cases.len()
        );
    }
}

#[test]
#[cfg(feature = "rustc-assert")]
fn unsupported_assertion_message_is_an_item_refusal_not_a_macro_error() {
    use super::resolve_macro_call_id;
    use crate::{
        nameres::{ModuleSource, crate_def_map},
        test_db::TestDB,
    };
    use hir_expand::InFile;
    use syntax::{AstNode, AstPtr, ast};
    use test_fixture::WithFixture;
    let db = TestDB::with_files(
        r#"
//- /main.rs crate:main deps:legacy edition:2024
fn main() { legacy::mixed_assert!({ let _ = async {}; true }); }
//- /legacy.rs crate:legacy edition:2015
#[macro_export]
#[rustc_builtin_macro]
macro_rules! assert { () => {} }
#[macro_export]
macro_rules! mixed_assert { ($e:expr) => {{ let async = false; $crate::assert!(async && $e); }}; }
"#,
    );
    let map = crate_def_map(&db, db.fetch_test_crate());
    let source = map.module(&db, map.root).definition_source(&db);
    let ModuleSource::SourceFile(file) = source.value else { panic!() };
    let call = file.syntax().descendants().find_map(ast::MacroCall::cast).unwrap();
    let ast_id = source.file_id.ast_id_map(&db).ast_id(&call);
    let wrapper = resolve_macro_call_id(
        &db,
        map,
        InFile::new(source.file_id, ast_id),
        InFile::new(source.file_id, AstPtr::new(&call)),
    )
    .unwrap();
    let expanded = wrapper.parse_macro_expansion(&db);
    assert!(expanded.err.is_none());
    let call = expanded.value.0.syntax_node().descendants().find_map(ast::MacroCall::cast).unwrap();
    let file = hir_expand::HirFileId::from(wrapper);
    let ast_id = file.ast_id_map(&db).ast_id(&call);
    let assertion = resolve_macro_call_id(
        &db,
        map,
        InFile::new(file, ast_id),
        InFile::new(file, AstPtr::new(&call)),
    )
    .unwrap();
    assert!(
        assertion.parse_macro_expansion(&db).err.is_none(),
        "unrelated items must remain analyzable"
    );
    assert!(assertion.assertion_message_unqualified(&db), "native codegen must refuse this item");
}

#[test]
#[cfg(feature = "rustc-assert")]
fn procedural_assertion_call_and_definition_origins_stay_native() {
    use crate::{ModuleDefId, expr_store::Body, nameres::crate_def_map, test_db::TestDB};
    use hir_expand::{MacroDefKind, builtin::BuiltinFnLikeExpander};
    use test_fixture::WithFixture;
    for source in [
        "#[proc_macros::identity] fn main() { assert!((false)); }",
        "#[proc_macros::identity] macro_rules! generated { () => { assert!((false)); }; } fn main() { generated!(); }",
    ] {
        let fixture = format!(
            r#"
//- proc_macros: identity
#[rustc_builtin_macro]
macro_rules! assert {{ () => {{}} }}
{source}
"#
        );
        let db = TestDB::with_files(&fixture);
        let map = crate_def_map(&db, db.fetch_test_crate());
        let function = map
            .module(&db, map.root)
            .scope(&db)
            .declarations()
            .find_map(|item| if let ModuleDefId::FunctionId(id) = item { Some(id) } else { None })
            .unwrap();
        let (_, source_map) = Body::with_source_map(&db, function.into());
        let assertions = source_map
            .macro_calls()
            .filter_map(|(_, call)| {
                matches!(
                    call.loc(&db).def.kind,
                    MacroDefKind::BuiltIn(_, BuiltinFnLikeExpander::Assert)
                )
                .then_some(call)
            })
            .collect::<Vec<_>>();
        assert_eq!(assertions.len(), 1, "{source}");
        assert!(assertions[0].parse_macro_expansion(&db).err.is_none());
        assert!(assertions[0].assertion_message_unqualified(&db), "{source}");
    }
}
