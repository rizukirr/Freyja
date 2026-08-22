use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Error, FnArg, Ident, ItemFn, LitBool, LitStr, Result, Token, Visibility, parse::Parse,
    parse::ParseStream,
};

pub(crate) struct ToolAttrs {
    pub(crate) description: LitStr,
    pub(crate) strict: bool,
}

impl Parse for ToolAttrs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut description = None;
        let mut strict = false;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;

            match key.to_string().as_str() {
                "description" => description = Some(input.parse::<LitStr>()?),
                "strict" => strict = input.parse::<LitBool>()?.value,
                _ => {
                    return Err(Error::new_spanned(
                        key,
                        "unknown tool attribute; expected `description` or `strict`",
                    ));
                }
            }

            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        let description = description.ok_or_else(|| {
            Error::new(
                proc_macro2::Span::call_site(),
                "missing description = \"...\"",
            )
        })?;

        Ok(Self {
            description,
            strict,
        })
    }
}

pub(crate) fn expand(attributes: TokenStream, input: TokenStream) -> Result<TokenStream> {
    let attributes = syn::parse2::<ToolAttrs>(attributes)?;
    let mut function = syn::parse2::<ItemFn>(input)?;

    let is_async = function.sig.asyncness.is_some();
    // Textual, because a proc macro sees tokens and not types. A `Result` alias
    // is not detected and falls back to serializing the value whole, which is
    // what every tool did before this existed.
    let is_fallible = match &function.sig.output {
        syn::ReturnType::Type(_, ty) => match ty.as_ref() {
            syn::Type::Path(path) => path
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "Result"),
            _ => false,
        },
        syn::ReturnType::Default => false,
    };
    if !function.sig.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &function.sig.generics,
            "tool functions cannot be generic",
        ));
    }

    let inputs: Vec<&FnArg> = function.sig.inputs.iter().collect();
    let takes_context = inputs.first().is_some_and(|argument| is_context(argument));
    let parameters = inputs[usize::from(takes_context)..]
        .iter()
        .copied()
        .map(parameter)
        .collect::<Result<Vec<_>>>()?;
    let field_names = parameters.iter().map(|(name, _)| name);
    let parameter_types = parameters.iter().map(|(_, ty)| ty);
    let destructured_names = parameters.iter().map(|(name, _)| name);
    let invocation_names = parameters.iter().map(|(name, _)| name);

    let name = function.sig.ident.clone();
    let implementation_name = format_ident!("__freyja_tool_{name}");
    let arguments_name = format_ident!("__Freyja{}Arguments", pascal_case(&name));
    let definition_name = format_ident!("__freyja_tool_{name}_definition");
    let type_name = format_ident!("__freyja_tool_{name}_type");
    let description = attributes.description;
    let strict = attributes.strict;

    function.sig.ident = implementation_name.clone();
    function.vis = Visibility::Inherited;

    let schema = if strict {
        quote!(::freyja::strict_schema(schema))
    } else {
        quote!(schema)
    };

    let call_expression = if takes_context {
        quote!(#implementation_name(cx, #( #invocation_names ),*))
    } else {
        quote!(#implementation_name( #( #invocation_names ),* ))
    };
    let call_expression = if is_async {
        quote!(#call_expression.await)
    } else {
        call_expression
    };

    let executor_body = if is_fallible {
        quote! {
            match #call_expression {
                Ok(value) => ::freyja::__private::serde_json::to_string(&value)
                    .map_err(::freyja::ToolError::Result),
                Err(error) => Err(::freyja::ToolError::Execution(
                    ::std::string::ToString::to_string(&error),
                )),
            }
        }
    } else {
        quote! {
            ::freyja::__private::serde_json::to_string(&#call_expression)
                .map_err(::freyja::ToolError::Result)
        }
    };

    let context_binding = if takes_context {
        quote!(cx: &'__freyja ::freyja::Context)
    } else {
        quote!(_cx: &'__freyja ::freyja::Context)
    };

    Ok(quote! {
        #function

        #[derive(
            ::freyja::__private::serde::Deserialize,
            ::freyja::__private::schemars::JsonSchema,
        )]
        struct #arguments_name {
            #( #field_names: #parameter_types, )*
        }

        fn #definition_name() -> ::freyja::ToolDefinition {
            let schema = ::freyja::__private::serde_json::to_value(
                ::freyja::__private::schemars::schema_for!(#arguments_name),
            )
            .expect("tool argument schema must serialize");
            let schema = #schema;

            ::freyja::ToolDefinition::new(stringify!(#name), #description)
                .parameters(schema)
                .strict(#strict)
        }

        #[allow(non_camel_case_types)]
        struct #type_name;

        impl ::freyja::Tool for #type_name {
            fn name(&self) -> &str {
                stringify!(#name)
            }

            fn definition(&self) -> ::freyja::ToolDefinition {
                #definition_name()
            }

            fn call<'__freyja>(
                &'__freyja self,
                arguments: &'__freyja str,
                #context_binding,
            ) -> ::freyja::ToolFuture<'__freyja> {
                let parsed =
                    ::freyja::__private::serde_json::from_str::<#arguments_name>(arguments);
                ::std::boxed::Box::pin(async move {
                    let #arguments_name { #( #destructured_names, )* } =
                        parsed.map_err(::freyja::ToolError::Arguments)?;
                    #executor_body
                })
            }
        }

        #[allow(non_upper_case_globals)]
        const #name: #type_name = #type_name;
    })
}

/// Whether a parameter is the optional leading `&Context`.
///
/// Recognised by type and only in first position, so the rule is one sentence:
/// a tool's first parameter may be `&Context`, and it is hidden from the model.
fn is_context(argument: &FnArg) -> bool {
    let FnArg::Typed(argument) = argument else {
        return false;
    };
    let syn::Type::Reference(reference) = argument.ty.as_ref() else {
        return false;
    };
    let syn::Type::Path(path) = reference.elem.as_ref() else {
        return false;
    };
    path.path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "Context")
}

fn parameter(argument: &FnArg) -> Result<(&Ident, &syn::Type)> {
    let FnArg::Typed(argument) = argument else {
        return Err(Error::new_spanned(
            argument,
            "tool functions cannot have receivers",
        ));
    };
    let syn::Pat::Ident(name) = argument.pat.as_ref() else {
        return Err(Error::new_spanned(
            &argument.pat,
            "tool parameters must use identifier patterns",
        ));
    };
    Ok((&name.ident, &argument.ty))
}

fn pascal_case(ident: &Ident) -> String {
    ident
        .to_string()
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            let first = characters.next().expect("non-empty identifier part");
            first.to_uppercase().chain(characters).collect::<String>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ToolAttrs, expand};
    use quote::quote;

    #[test]
    fn parser_requires_a_description() {
        let error = syn::parse2::<ToolAttrs>(quote!(strict = true))
            .err()
            .expect("attributes without a description should fail");
        assert_eq!(error.to_string(), "missing description = \"...\"");
    }

    #[test]
    fn parser_defaults_strict_to_false() {
        let attributes =
            syn::parse2::<ToolAttrs>(quote!(description = "adds")).expect("attributes parse");
        assert_eq!(attributes.description.value(), "adds");
        assert!(!attributes.strict);
    }

    #[test]
    fn parser_accepts_an_explicit_strict_value() {
        let attributes = syn::parse2::<ToolAttrs>(quote!(description = "adds", strict = true))
            .expect("attributes parse");
        assert!(attributes.strict);
    }

    #[test]
    fn parser_rejects_unknown_keys() {
        let error = syn::parse2::<ToolAttrs>(quote!(description = "adds", other = true))
            .err()
            .expect("unknown attributes should fail");
        assert_eq!(
            error.to_string(),
            "unknown tool attribute; expected `description` or `strict`"
        );
    }

    #[test]
    fn parser_rejects_missing_separators() {
        assert!(syn::parse2::<ToolAttrs>(quote!(description = "adds" strict = true)).is_err());
    }

    #[test]
    fn expansion_generates_a_parseable_file() {
        let output = expand(
            quote!(description = "adds two numbers", strict = true),
            quote!(
                fn add(a: i64, b: i64) -> i64 {
                    a + b
                }
            ),
        )
        .expect("supported function expands");
        syn::parse2::<syn::File>(output).expect("expansion is valid Rust");
    }

    #[test]
    fn expansion_rejects_unsupported_signatures() {
        for input in [
            quote!(
                fn add<T>(a: T) -> T {
                    a
                }
            ),
            quote!(
                fn add(&self) -> i64 {
                    0
                }
            ),
            quote!(
                fn add((a, b): (i64, i64)) -> i64 {
                    a + b
                }
            ),
        ] {
            assert!(expand(quote!(description = "adds"), input).is_err());
        }
    }

    #[test]
    fn expansion_accepts_async_functions() {
        let output = expand(
            quote!(description = "fetches a page"),
            quote!(
                async fn fetch(url: String) -> String {
                    url
                }
            ),
        )
        .expect("async function expands");
        assert!(output.to_string().contains("impl :: freyja :: Tool"));
        syn::parse2::<syn::File>(output).expect("expansion is valid Rust");
    }

    #[test]
    fn expansion_accepts_fallible_functions() {
        let output = expand(
            quote!(description = "divides"),
            quote!(
                fn divide(a: i64, b: i64) -> Result<i64, String> {
                    if b == 0 {
                        Err("division by zero".to_string())
                    } else {
                        Ok(a / b)
                    }
                }
            ),
        )
        .expect("fallible function expands");
        syn::parse2::<syn::File>(output).expect("expansion is valid Rust");
    }

    #[test]
    fn expansion_accepts_a_leading_context_parameter() {
        let output = expand(
            quote!(description = "greets"),
            quote!(
                fn greet(cx: &Context, name: String) -> String {
                    let _ = cx;
                    name
                }
            ),
        )
        .expect("context function expands");
        syn::parse2::<syn::File>(output).expect("expansion is valid Rust");
    }
}
