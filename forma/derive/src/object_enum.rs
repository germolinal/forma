use proc_macro2::TokenStream;
use quote::quote;

use crate::serde_rename::RenameRule;
use crate::{SerdeAttribute, field::Field};
use std::fmt::Write as _; // import without risk of name clashing

#[derive(Clone)]
pub struct VariantData {
    pub ident: syn::Ident,

    #[allow(dead_code)]
    pub attributes: Vec<String>,
    pub fields: Vec<Field>,
    pub docs: String,
    // pub ty : syn::Type,
}

#[derive(Clone)]
pub enum EnumVariant {
    Unit(VariantData),
    Unnamed(VariantData),
    Named(VariantData),
}

impl EnumVariant {
    fn data(&self) -> VariantData {
        match self {
            Self::Unit(d) => d.clone(),
            Self::Unnamed(d) => d.clone(),
            Self::Named(d) => d.clone(),
        }
    }

    #[cfg(not(tarpaulin_include))]
    pub(crate) fn new(variant: syn::Variant) -> Self {
        let ident = variant.ident.clone();

        let attributes: Vec<String> = variant
            .attrs
            .iter()
            .map(|a| format!("{}", a.path().segments[0].ident))
            .collect();

        let docs = crate::docs::get_docs(&variant.attrs).expect("Could not generate docs");

        let mut data = VariantData {
            ident,
            attributes,
            fields: Vec::new(),
            docs,
        };
        match &variant.fields {
            syn::Fields::Unit => Self::Unit(data),
            syn::Fields::Unnamed(fields) => {
                data.fields = fields
                    .unnamed
                    .iter()
                    .map(|x| Field::new(x.clone()).expect("Could not create field"))
                    .collect();
                Self::Unnamed(data)
            }
            syn::Fields::Named(named_fields) => {
                data.fields = named_fields
                    .named
                    .clone()
                    .into_iter()
                    .filter_map(|f| {
                        // Skip State fields
                        let a = Field::new(f).expect("Could not create field");
                        match a {
                            Field::State(_) => None,
                            _ => Some(a),
                        }
                    })
                    .collect();

                Self::Named(data)
            }
        }
    }
}

#[derive(Debug, Clone)]
enum EnumType {
    Unit,
    Named,
    Unnamed,
}

pub struct EnumObject {
    pub(crate) ident: syn::Ident,
    variants: Vec<EnumVariant>,
    docs: String,
    #[allow(dead_code)]
    attributes: Vec<SerdeAttribute>,
    rename: RenameRule,
    tag: Option<String>,
    #[allow(dead_code)]
    kind: EnumType,
}

impl EnumObject {
    #[cfg(not(tarpaulin_include))]
    pub(crate) fn new(
        ident: syn::Ident,
        stru: syn::DataEnum,
        docs: String,
        attributes: Vec<SerdeAttribute>,
    ) -> Self {
        const QUOTE: char = '"';
        let rename = if let Some(rename) = attributes.iter().find(|x| x.action == "rename_all") {
            let mut name = rename.value.clone();
            if name.starts_with(QUOTE) {
                name = name.trim_start_matches(QUOTE).to_string();
            }
            if name.ends_with(QUOTE) {
                name = name.trim_end_matches(QUOTE).to_string();
            }
            match RenameRule::from_str(&name) {
                Ok(k) => k,
                Err(_e) => panic!("unknown rename-rule '{}'", rename.value),
            }
        } else {
            RenameRule::None
        };
        let tag = if let Some(tag) = attributes.iter().find(|x| x.action == "tag") {
            if tag.value.is_empty() {
                panic!("cannot tag with an empty string")
            }
            Some(tag.value.clone())
        } else {
            None
        };

        let variants: Vec<EnumVariant> = stru
            .variants
            .iter()
            .map(|x| EnumVariant::new(x.clone()))
            .collect();

        let kind = if let Some(first_v) = variants.first() {
            match first_v {
                EnumVariant::Named(_) => EnumType::Named,
                EnumVariant::Unnamed(_) => EnumType::Unnamed,
                EnumVariant::Unit(_) => EnumType::Unit,
            }
        } else {
            panic!("empty enum!")
        };

        Self {
            kind,
            rename,
            tag,
            variants,
            ident,
            docs,
            attributes,
        }
    }

    #[cfg(not(tarpaulin_include))]
    pub(crate) fn gen_docs(&self) -> Result<(String, TokenStream), String> {
        let mut ret = String::new();

        // // Title
        // Title
        let suffix = "Definition".to_string();
        let mut title = format!("{}", self.ident);
        if title.ends_with(&suffix) {
            title = title.trim_end_matches(&suffix).to_string();
        }
        writeln!(ret, "# {}\n", title).map_err(|e| e.to_string())?;

        // Written doc
        writeln!(ret, "{}\n\n## Supported Variants\n", self.docs).map_err(|e| e.to_string())?;
        if let Some(tag) = &self.tag {
            writeln!(
                ret,
                "> 🎯 Variants are identified by using the `{tag}` field. For instance \n>\n> ```yaml\n> {tag}: name-of-variant\n> ```\n"
            )
            .map_err(|e| e.to_string())?;
        }

        let mut q = quote! {};
        // Variants can share a type (e.g., two tools with the same
        // definition), and its page should be printed only once.
        let mut printed = std::collections::HashSet::new();
        // Document variants
        for variant in self.variants.iter() {
            match variant {
                EnumVariant::Unnamed(s) => {
                    let rs_ident = s.ident.clone();
                    let content_ident = if let Some(f) = &s.fields.first() {
                        let t = f.data().ty;
                        if let syn::Type::Path(path) = t {
                            // format!("{}", &path.path.segments[0].ident)
                            path.path.segments[0].ident.clone()
                        } else {
                            unreachable!("no path");
                        }
                    } else {
                        rs_ident.clone()
                    };

                    let v_ident = self.rename.apply_to_variant(&rs_ident.to_string());
                    let doc: String = variant.data().docs.trim().to_string();

                    let suffix = "Definition".to_string();
                    let mut name_str = format!("{}", content_ident);
                    if name_str.ends_with(&suffix) {
                        name_str = name_str.trim_end_matches(&suffix).to_string();
                    }
                    name_str = name_str.to_lowercase();

                    if printed.insert(content_ident.to_string()) {
                        q = quote!(
                            #q
                            #content_ident::print_doc(&dir, summary, tic+1).unwrap();
                        );
                    }
                    // Title
                    writeln!(ret, "### [`{}`](./auto-{}.md)\n{}", v_ident, name_str, doc)
                        .map_err(|x| x.to_string())?;
                }
                EnumVariant::Named(_s) => {
                    todo!("named enums do not support auto-documentation")
                }
                EnumVariant::Unit(s) => {
                    let v_ident = format!("{}", s.ident);
                    let v_ident = self.rename.apply_to_variant(&v_ident);
                    writeln!(ret, "###  `{}`\n", v_ident).map_err(|e| e.to_string())?;
                    writeln!(ret, "{}\n\n", variant.data().docs).map_err(|e| e.to_string())?;
                }
            }
        }

        Ok((ret, q))
    }
}
