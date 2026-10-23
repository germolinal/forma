#![warn(clippy::dbg_macro)]

mod common_path;
mod docs;
mod field;
mod object;
mod object_enum;
mod object_struct;
mod serde_rename;

use crate::docs::get_docs;
use object::Object;
use proc_macro::TokenStream;
use proc_macro2::{TokenStream as TokenStream2, TokenTree};
use quote::quote;
use syn::{Attribute, DeriveInput, parse_macro_input};

#[derive(Debug, Clone)]
pub(crate) struct SerdeAttribute {
    #[allow(dead_code)]
    pub(crate) ident: String,
    pub(crate) action: String,
    pub(crate) value: String,
}

impl SerdeAttribute {
    fn tokens(a: &Attribute) -> Option<TokenStream2> {
        match a.clone().meta.require_list() {
            Ok(v) => Some(v.tokens.clone()),
            Err(_) => None,
        }
    }

    /// Every `name = "value"` pair in a `#[serde(...)]` attribute, e.g. both
    /// in `#[serde(tag = "kind", rename_all = "kebab-case")]`. Bare words
    /// (`deny_unknown_fields`) carry no value, and are left out.
    pub(crate) fn all(a: &Attribute) -> Vec<Self> {
        const QUOTE: char = '"';
        let Some(seg) = a.path().segments.iter().next() else {
            return vec![];
        };
        let ident = format!("{}", seg.ident);
        if ident != "serde" {
            return vec![];
        }
        let Some(tokens) = Self::tokens(a) else {
            return vec![];
        };

        let tokens: Vec<TokenTree> = tokens.into_iter().collect();
        let mut found = Vec::new();
        for pair in tokens.split(|t| matches!(t, TokenTree::Punct(p) if p.as_char() == ',')) {
            if let [
                TokenTree::Ident(action),
                TokenTree::Punct(eq),
                TokenTree::Literal(value),
            ] = pair
                && eq.as_char() == '='
            {
                let value = format!("{value}")
                    .trim_start_matches(QUOTE)
                    .trim_end_matches(QUOTE)
                    .to_string();
                found.push(Self {
                    ident: ident.clone(),
                    action: format!("{action}"),
                    value,
                });
            }
        }
        found
    }
}

fn get_attributes(ast: &DeriveInput) -> Vec<SerdeAttribute> {
    ast.attrs.iter().flat_map(SerdeAttribute::all).collect()
}

#[proc_macro_derive(FormaDocs)]
pub fn derive_docs(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);

    let attributes = get_attributes(&ast);
    let docs = get_docs(&ast.attrs).expect("Could not generate docs");
    let obj = Object::new(ast.clone(), docs, attributes);
    let object_name = &ast.ident;

    // docs
    let docs = obj.gen_docs();

    // return
    TokenStream::from(quote!(

        impl #object_name {

            #docs

        }
    ))
}
