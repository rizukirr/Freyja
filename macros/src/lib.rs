use proc_macro::TokenStream;

mod tools;

#[proc_macro_attribute]
pub fn tool(attributes: TokenStream, input: TokenStream) -> TokenStream {
    tools::expand(attributes.into(), input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
