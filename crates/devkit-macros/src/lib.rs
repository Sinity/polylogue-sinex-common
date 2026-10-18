//! `#[devkit::test]`: one test attribute for synchronous and async tests.
//!
//! A test with no parameters costs nothing beyond a plain `#[test]`. An
//! `async fn` test runs on a current-thread tokio runtime (the test crate
//! depends on tokio with the `rt` and `time` features), optionally under
//! `timeout = "30s"`. Test fixtures such as a cloned database arrive as
//! parameters once devkit provides them; until then a parameter is a compile
//! error.
use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, LitStr, parse_macro_input, spanned::Spanned};

#[proc_macro_attribute]
pub fn test(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    let mut timeout: Option<LitStr> = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("timeout") {
            timeout = Some(meta.value()?.parse()?);
            Ok(())
        } else {
            Err(meta.error("expected `timeout = \"30s\"`"))
        }
    });
    parse_macro_input!(attr with parser);
    match expand(function, timeout) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand(function: ItemFn, timeout: Option<LitStr>) -> syn::Result<proc_macro2::TokenStream> {
    if let Some(input) = function.sig.inputs.first() {
        return Err(syn::Error::new(
            input.span(),
            "devkit::test takes no parameters yet; fixtures are not available",
        ));
    }
    let millis = timeout.as_ref().map(parse_duration).transpose()?;
    let ItemFn {
        attrs,
        vis,
        mut sig,
        block,
    } = function;
    if sig.asyncness.is_none() {
        if let Some(t) = &timeout {
            return Err(syn::Error::new(
                t.span(),
                "timeout applies to async tests; nextest's slow-timeout covers the rest",
            ));
        }
        return Ok(quote! { #[::core::prelude::v1::test] #(#attrs)* #vis #sig #block });
    }
    sig.asyncness = None;
    let body = match millis {
        Some(ms) => quote! {
            ::tokio::time::timeout(::std::time::Duration::from_millis(#ms), async move #block)
                .await
                .unwrap_or_else(|_| panic!("test timed out after {} ms", #ms))
        },
        None => quote! { async move #block.await },
    };
    Ok(quote! {
        #[::core::prelude::v1::test]
        #(#attrs)*
        #vis #sig {
            ::tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(async move { #body })
        }
    })
}

/// `"500ms"`, `"30s"` or `"2m"` in milliseconds.
fn parse_duration(literal: &LitStr) -> syn::Result<u64> {
    let text = literal.value();
    let (number, factor) = if let Some(n) = text.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = text.strip_suffix('s') {
        (n, 1_000)
    } else if let Some(n) = text.strip_suffix('m') {
        (n, 60_000)
    } else {
        return Err(syn::Error::new(literal.span(), "use a unit: ms, s or m"));
    };
    number
        .trim()
        .parse::<u64>()
        .map(|n| n * factor)
        .map_err(|_| syn::Error::new(literal.span(), "expected a whole number before the unit"))
}
