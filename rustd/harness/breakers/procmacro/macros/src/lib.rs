//! Macros that give tokens only a proc macro can make.

use proc_macro::{Delimiter, Group, Literal, Punct, Spacing, TokenStream, TokenTree};

/// Three negative numbers, each one literal token: `(-1, -20i16, -1.5,)`.
#[proc_macro]
pub fn negative_numbers(_: TokenStream) -> TokenStream {
    let parsed: Literal = "-20i16".parse().expect("a negative number is a literal");
    let numbers = [Literal::i8_unsuffixed(-1), parsed, Literal::f32_unsuffixed(-1.5)];
    let mut tuple = TokenStream::new();
    for number in numbers {
        tuple.extend([TokenTree::from(number), TokenTree::from(Punct::new(',', Spacing::Alone))]);
    }
    TokenTree::from(Group::new(Delimiter::Parenthesis, tuple)).into()
}

/// Synthesized tokens must report the attribute, while forwarded body tokens
/// retain their written locations. Exercise the same call-site span as instrument.
#[proc_macro_attribute]
pub fn locations(_: TokenStream, input: TokenStream) -> TokenStream {
    input.into_iter().map(|token| match token {
        TokenTree::Group(group) if group.delimiter() == Delimiter::Brace => {
            let mut body: TokenStream = "let generated_line = line!(); let generated_location = std::panic::Location::caller().line(); let _ = std::panic::catch_unwind(|| panic!(\"generated\"));".parse().unwrap();
            body.extend(group.stream());
            let mut replacement = Group::new(Delimiter::Brace, body);
            replacement.set_span(group.span());
            TokenTree::Group(replacement)
        }
        token => token,
    }).collect()
}
