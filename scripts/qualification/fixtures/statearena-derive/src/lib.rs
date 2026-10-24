use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, Data, Fields};
#[proc_macro_derive(Persist)]
pub fn derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    if !input.generics.params.is_empty() { return syn::Error::new_spanned(name, "Persist prototype supports concrete structs").to_compile_error().into(); }
    let Data::Struct(data) = input.data else { return syn::Error::new_spanned(name, "Persist needs a struct").to_compile_error().into(); };
    let Fields::Named(fields) = data.fields else { return syn::Error::new_spanned(name, "Persist needs named fields").to_compile_error().into(); };
    let names: Vec<_> = fields.named.iter().map(|f| f.ident.as_ref().unwrap()).collect();
    let types: Vec<_> = fields.named.iter().map(|f| &f.ty).collect();
    quote! {
        impl ::statearena::Persist for #name {
            fn schema() -> ::statearena::Schema {
                ::statearena::Schema::record(stringify!(#name), ::std::mem::size_of::<Self>(), vec![
                    #(::statearena::Field { name: stringify!(#names).into(), offset: ::std::mem::offset_of!(Self, #names), schema: <#types as ::statearena::Persist>::schema() }),*
                ])
            }
            fn encode(&self) -> ::statearena::Value {
                ::statearena::Value::Record(vec![#((stringify!(#names).into(), ::statearena::Persist::encode(&self.#names))),*])
            }
            fn decode(value: &::statearena::Value, old: &::statearena::Schema, log: &mut Vec<String>) -> ::statearena::Result<Self> {
                static NEW_SCHEMA: ::std::sync::OnceLock<::statearena::Schema> = ::std::sync::OnceLock::new();
                let schema = NEW_SCHEMA.get_or_init(Self::schema);
                let fields = ::statearena::record_fields(value, old, stringify!(#name), schema, log)?;
                Ok(Self { #(#names: ::statearena::field::<#types>(stringify!(#names), fields, old, log)?),* })
            }
        }
    }.into()
}
