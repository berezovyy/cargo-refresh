//! Exact-toolchain assertion AST printing, isolated from RA's Rust ABI/TLS.
//! rustc prints the condition before expanding macros inside it:
//! https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_builtin_macros/src/assert.rs
use span::Edition;
use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    os::unix::fs::PermissionsExt as _,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{LazyLock, Mutex},
};

struct Printer {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Drop for Printer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
static PRINTER: LazyLock<Mutex<Option<Printer>>> = LazyLock::new(|| {
    Mutex::new(load().map_err(|e| eprintln!("rustd: assertion printer unavailable: {e}")).ok())
});

fn load() -> Result<Printer, Box<dyn std::error::Error>> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let version = Command::new(&rustc).arg("-Vv").output()?;
    if !version.status.success()
        || !String::from_utf8_lossy(&version.stdout)
            .lines()
            .any(|line| line == concat!("commit-hash: ", env!("RUSTD_ASSERT_RUSTC_COMMIT")))
    {
        return Err("assertion printer toolchain mismatch".into());
    }
    let sysroot = Command::new(rustc).args(["--print", "sysroot"]).output()?;
    if !sysroot.status.success() {
        return Err("assertion printer sysroot unavailable".into());
    }
    let sysroot = std::path::PathBuf::from(String::from_utf8(sysroot.stdout)?.trim());
    let directory = std::env::var_os("RUSTD_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".cache/cargo-refresh/compiler-helpers"))
        })
        .ok_or("assertion printer needs a private on-disk scratch directory")?;
    std::fs::create_dir_all(&directory)?;
    let mut file = tempfile::Builder::new().prefix("rustd-assert-").tempfile_in(directory)?;
    file.write_all(include_bytes!(concat!(env!("OUT_DIR"), "/assertion-printer")))?;
    file.flush()?;
    file.as_file().set_permissions(std::fs::Permissions::from_mode(0o700))?;
    let path = file.into_temp_path(); // close the writing handle before exec
    let mut child = Command::new(&path)
        .env("LD_LIBRARY_PATH", sysroot.join("lib"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    drop(path); // Linux retains the running inode; no scratch file persists.
    let input = child.stdin.take().ok_or("assertion printer stdin unavailable")?;
    let mut output =
        BufReader::new(child.stdout.take().ok_or("assertion printer stdout unavailable")?);
    let mut ready = String::new();
    if output.read_line(&mut ready).is_err() || ready != "rustd-assert:1\n" {
        let _ = child.kill();
        let _ = child.wait();
        return Err("assertion printer did not start with the selected toolchain libraries".into());
    }
    Ok(Printer { child, input, output })
}

// Keep interpolated AST fragments opaque while lexing. In particular, RA's
// synthetic parentheses preserve grouping but must not become explicit Paren
// nodes: rustc's pretty-printer inserts only the parentheses precedence needs.
fn packet(
    tokens: tt::TokenTreesView<'_>,
    span: span::Span,
    stringify: &dyn Fn(tt::TokenTreesView<'_>) -> String,
) -> Vec<u8> {
    let mut prefix = "__rustd_assert_fragment_".to_owned();
    while tokens.iter_flat_tokens().any(|t| matches!(t, tt::TokenTree::Leaf(tt::Leaf::Ident(i)) if i.sym.as_str().starts_with(&prefix))) { prefix.push('_'); }
    fn encode(
        tokens: tt::TokenTreesView<'_>,
        span: span::Span,
        prefix: &str,
        records: &mut Vec<(u8, String)>,
        stringify: &dyn Fn(tt::TokenTreesView<'_>) -> String,
    ) -> String {
        let mut builder = tt::TopSubtreeBuilder::new(tt::Delimiter::invisible_spanned(span));
        fn append(
            iter: tt::TtIter<'_>,
            span: span::Span,
            prefix: &str,
            records: &mut Vec<(u8, String)>,
            stringify: &dyn Fn(tt::TokenTreesView<'_>) -> String,
            builder: &mut tt::TopSubtreeBuilder,
        ) {
            for item in iter {
                let fragment = match item.clone() {
                    tt::TtElement::Subtree(st, content) if st.delimiter.kind == tt::DelimiterKind::Parenthesis && st.delimiter.open == st.delimiter.close => {
                        let text = encode(content.remaining(), span, prefix, records, stringify);
                        Some((0, text))
                    }
                    tt::TtElement::Subtree(st, content) if st.delimiter.kind == tt::DelimiterKind::Invisible
                        && content.clone().any(|t| matches!(t, tt::TtElement::Leaf(tt::Leaf::Punct(p)) if p.spacing == tt::Spacing::Synthetic)) => {
                        let text = encode(content.remaining(), span, prefix, records, stringify);
                        Some((1, text))
                    }
                    tt::TtElement::Leaf(tt::Leaf::Ident(ref ident)) if ident.sym.as_str() == "$crate" => Some((4, String::new())),
                    _ => None,
                };
                if let Some(fragment) = fragment {
                    let index = records.len();
                    records.push(fragment);
                    let mut ident = tt::Ident::new(&format!("{prefix}{index}"), span);
                    ident.origin = tt::IdentOrigin::Metavar;
                    builder.push(tt::Leaf::Ident(ident));
                } else {
                    match item {
                        tt::TtElement::Leaf(leaf) => builder.push(leaf),
                        tt::TtElement::Subtree(st, content) => {
                            builder.open(st.delimiter.kind, st.delimiter.open);
                            append(content, span, prefix, records, stringify, builder);
                            builder.close(st.delimiter.close);
                        }
                    }
                }
            }
        }
        append(tokens.iter(), span, prefix, records, stringify, &mut builder);
        stringify(builder.build().token_trees())
    }
    let mut records = Vec::new();
    let root = encode(tokens, span, &prefix, &mut records, stringify);
    records.push((255, root));
    let mut result = Vec::new();
    result.extend_from_slice(&(prefix.len() as u32).to_le_bytes());
    result.extend_from_slice(prefix.as_bytes());
    result.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for (kind, text) in records {
        result.push(kind);
        result.extend_from_slice(&(text.len() as u32).to_le_bytes());
        result.extend_from_slice(text.as_bytes());
    }
    result
}

pub(crate) fn expression(
    tokens: tt::TokenTreesView<'_>,
    span: span::Span,
    edition: Edition,
    stringify: &dyn Fn(tt::TokenTreesView<'_>) -> String,
) -> Option<String> {
    let source = packet(tokens, span, stringify);
    if source.len() > 16 * 1024 * 1024 {
        return None;
    }
    let mut lock = PRINTER.lock().unwrap_or_else(|e| e.into_inner());
    let printer = lock.as_mut()?;
    let printed = (|| -> std::io::Result<Option<String>> {
        writeln!(printer.input, "{} {}", edition as u8, source.len())?;
        printer.input.write_all(&source)?;
        printer.input.flush()?;
        let mut header = String::new();
        printer.output.read_line(&mut header)?;
        if header.trim() == "-1" {
            return Ok(None);
        }
        let len = header
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|n| *n <= 64 * 1024 * 1024)
            .ok_or_else(|| std::io::Error::other("invalid assertion printer response"))?;
        let mut bytes = vec![0; len];
        printer.output.read_exact(&mut bytes)?;
        Ok(String::from_utf8(bytes).ok())
    })();
    match printed {
        Ok(message) => message,
        Err(_) => {
            *lock = None;
            None
        }
    }
}

pub(crate) fn available() -> bool {
    PRINTER.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}
