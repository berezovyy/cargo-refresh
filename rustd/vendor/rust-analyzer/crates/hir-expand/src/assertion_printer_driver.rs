// Exact rustc 1.99 parser/printer, isolated in a pipe worker. Compiler-private
// Rust linkage and TLS never enter the daemon. The worker exits on pipe EOF.
// https://github.com/rust-lang/rust/blob/1.99.0/compiler/rustc_builtin_macros/src/assert.rs
#![feature(rustc_private)]
extern crate rustc_ast;
extern crate rustc_ast_pretty;
extern crate rustc_driver as _;
extern crate rustc_parse;
extern crate rustc_session;
extern crate rustc_span;

fn main() {
    use std::io::{BufRead as _, Read as _, Write as _};
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    if writeln!(output, "rustd-assert:1").and_then(|_| output.flush()).is_err() {
        return;
    }
    loop {
        let mut header = String::new();
        if input.read_line(&mut header).unwrap_or(0) == 0 {
            break;
        }
        let mut parts = header.split_whitespace();
        let Some(edition) = parts.next().and_then(|p| p.parse::<u8>().ok()) else { break };
        let Some(len) = parts.next().and_then(|p| p.parse::<usize>().ok()) else { break };
        if len > 16 * 1024 * 1024 {
            break;
        }
        let mut source = vec![0; len];
        if input.read_exact(&mut source).is_err() {
            break;
        }
        let message = std::panic::catch_unwind(|| expression(&source, edition)).ok().flatten();
        match message {
            Some(message) if message.len() <= 64 * 1024 * 1024 => {
                if writeln!(output, "{}", message.len())
                    .and_then(|_| output.write_all(message.as_bytes()))
                    .is_err()
                {
                    break;
                }
            }
            _ => {
                if writeln!(output, "-1").is_err() {
                    break;
                }
            }
        }
        if output.flush().is_err() {
            break;
        }
    }
}

fn expression(packet: &[u8], edition: u8) -> Option<String> {
    use std::io::Read as _;
    fn number(input: &mut &[u8]) -> Option<usize> {
        let mut n = [0; 4];
        input.read_exact(&mut n).ok()?;
        Some(u32::from_le_bytes(n) as usize)
    }
    fn text(input: &mut &[u8]) -> Option<String> {
        let n = number(input)?;
        let s = std::str::from_utf8(input.get(..n)?).ok()?.to_owned();
        *input = &input[n..];
        Some(s)
    }
    let mut input = packet;
    let prefix = text(&mut input)?;
    let n = number(&mut input)?;
    let mut records = Vec::new();
    for _ in 0..n {
        let kind = *input.first()?;
        input = &input[1..];
        let source = text(&mut input)?;
        records.push((kind, source));
    }
    if !input.is_empty() {
        return None;
    }

    let edition = match edition {
        0 => rustc_span::edition::Edition::Edition2015,
        1 => rustc_span::edition::Edition::Edition2018,
        2 => rustc_span::edition::Edition::Edition2021,
        3 => rustc_span::edition::Edition::Edition2024,
        _ => return None,
    };
    rustc_span::create_session_globals_then(edition, &[], None, || {
        let psess = rustc_session::parse::ParseSess::new();
        psess.dcx().make_silent();
        use rustc_ast::{
            token::{self, Delimiter, InvisibleOrigin, MetaVarKind, NtExprKind},
            tokenstream::{DelimSpacing, DelimSpan, Spacing, TokenStream, TokenTree},
        };
        fn substitute(
            stream: TokenStream,
            prefix: &str,
            fragments: &[(u8, TokenStream)],
        ) -> Option<TokenStream> {
            let mut result = Vec::new();
            for tree in stream.iter().cloned() {
                let tree = match tree {
                    TokenTree::Token(tok, spacing)
                        if tok
                            .ident()
                            .is_some_and(|(id, _)| id.name.as_str().starts_with(prefix)) =>
                    {
                        let (id, _) = tok.ident()?;
                        let i = id.name.as_str().strip_prefix(prefix)?.parse::<usize>().ok()?;
                        let (kind, content) = fragments.get(i)?;
                        if *kind == 4 {
                            TokenTree::Token(
                                token::Token::new(
                                    token::Ident(
                                        rustc_span::Symbol::intern("$crate"),
                                        token::IdentIsRaw::No,
                                    ),
                                    tok.span,
                                ),
                                spacing,
                            )
                        } else {
                            let kind = match kind {
                                0 => MetaVarKind::Expr {
                                    kind: NtExprKind::Expr,
                                    can_begin_literal_maybe_minus: false,
                                    can_begin_string_literal: false,
                                },
                                1 => MetaVarKind::Path,
                                _ => return None,
                            };
                            TokenTree::Delimited(
                                DelimSpan::from_single(tok.span),
                                DelimSpacing::new(Spacing::Alone, spacing),
                                Delimiter::Invisible(InvisibleOrigin::MetaVar(kind)),
                                content.clone(),
                            )
                        }
                    }
                    TokenTree::Delimited(span, spacing, delimiter, content) => {
                        TokenTree::Delimited(
                            span,
                            spacing,
                            delimiter,
                            substitute(content, prefix, fragments)?,
                        )
                    }
                    tree => tree,
                };
                result.push(tree);
            }
            Some(TokenStream::new(result))
        }
        let mut fragments = Vec::new();
        for (kind, source) in records {
            let mut parser = match rustc_parse::new_parser_from_source_str(
                &psess,
                rustc_span::FileName::Custom(format!("assertion-fragment-{}", fragments.len())),
                source,
                rustc_parse::lexer::StripTokens::Nothing,
            ) {
                Ok(parser) => parser,
                Err(errors) => {
                    for error in errors {
                        error.cancel();
                    }
                    return None;
                }
            };
            let tokens = parser.parse_tokens();
            fragments.push((kind, substitute(tokens, &prefix, &fragments)?));
        }
        let (kind, root) = fragments.pop()?;
        if kind != 255 {
            return None;
        }
        let mut parser = rustc_parse::parser::Parser::new(&psess, root, None);
        let expr = match parser.parse_expr() {
            Ok(expr) => expr,
            Err(error) => {
                error.cancel();
                return None;
            }
        };
        if parser.token.kind != rustc_ast::token::Eof || psess.dcx().has_errors().is_some() {
            return None;
        }
        Some(rustc_ast_pretty::pprust::expr_to_string(&expr))
    })
}
