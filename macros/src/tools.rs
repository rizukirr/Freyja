use syn::{Error, Ident, LitBool, LitStr, Result, Token, parse::Parse, parse::ParseStream};

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
            Error::new(proc_macro2::Span::call_site(), "missing tool description")
        })?;

        Ok(Self {
            description,
            strict,
        })
    }
}
