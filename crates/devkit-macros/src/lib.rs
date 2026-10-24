//! `#[devkit::test]`: one test attribute for synchronous and async tests.
//!
//! A test with no parameters costs nothing beyond a plain `#[test]`. An
//! `async fn` test runs on a current-thread tokio runtime (the test crate
//! depends on tokio with the `rt` and `time` features), optionally under
//! `timeout = "30s"`.
//!
//! Each parameter is a fixture: its type implements `devkit::Fixture`, and
//! the test creates it with `Fixture::create()` before the body runs, in
//! parameter order. The test crate depends on `devkit` under that name.
use proc_macro::TokenStream;
use quote::quote;
use syn::{FnArg, ItemFn, LitStr, parse_macro_input, punctuated::Punctuated, spanned::Spanned};

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
    let millis = timeout.as_ref().map(parse_duration).transpose()?;
    let ItemFn {
        attrs,
        vis,
        mut sig,
        block,
    } = function;
    let mut fixtures = Vec::new();
    for input in std::mem::replace(&mut sig.inputs, Punctuated::new()) {
        let FnArg::Typed(parameter) = input else {
            return Err(syn::Error::new(
                input.span(),
                "a test has no receiver; parameters are fixtures",
            ));
        };
        let (pattern, kind) = (&parameter.pat, &parameter.ty);
        fixtures.push(quote! {
            let #pattern: #kind = <#kind as ::devkit::Fixture>::create().unwrap_or_else(|error| {
                panic!("fixture {}: {}", ::core::stringify!(#kind), error)
            });
        });
    }
    if sig.asyncness.is_none() {
        if let Some(t) = &timeout {
            return Err(syn::Error::new(
                t.span(),
                "timeout applies to async tests; nextest's slow-timeout covers the rest",
            ));
        }
        if fixtures.is_empty() {
            return Ok(quote! { #[::core::prelude::v1::test] #(#attrs)* #vis #sig #block });
        }
        return Ok(quote! {
            #[::core::prelude::v1::test]
            #(#attrs)*
            #vis #sig {
                #(#fixtures)*
                #block
            }
        });
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
            #(#fixtures)*
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
