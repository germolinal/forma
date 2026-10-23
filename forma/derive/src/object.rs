use crate::SerdeAttribute;
use crate::object_enum::EnumObject;
use crate::object_struct::StructObject;
use proc_macro2::TokenStream as TokenStream2;
use quote::*;

/// A `SIMPLE` object
pub enum Object {
    Enum(EnumObject),
    StructObject(StructObject),
}

impl Object {
    #[cfg(not(tarpaulin_include))]
    pub(crate) fn new(
        ast: syn::DeriveInput,
        docs: String,
        attributes: Vec<SerdeAttribute>,
    ) -> Self {
        match ast.data {
            syn::Data::Struct(stru) => {
                let data = StructObject::new(ast.ident.clone(), stru, docs);
                Object::StructObject(data)
            }
            syn::Data::Enum(stru) => {
                let data = EnumObject::new(ast.ident.clone(), stru, docs, attributes);
                Object::Enum(data)
            }
            _ => {
                panic!("SIMPLE MAcros only work for Enums and StructObjects");
            }
        }
    }

    #[cfg(not(tarpaulin_include))]
    pub(crate) fn gen_docs(&self) -> TokenStream2 {
        let (ident, docs, q) = match self {
            Self::StructObject(s) => {
                let i = s.ident.clone();
                let docs = s.gen_docs().expect("could not generate docs");
                let q = quote!();
                (i, docs, q)
            }
            Self::Enum(s) => {
                let i = s.ident.clone();
                let (docs, q) = s.gen_docs().expect("Could not generate docs");
                (i, docs, q)
            }
        };
        let suffix = "Definition".to_string();
        let mut name_str = format!("{}", ident);
        if name_str.ends_with(&suffix) {
            name_str = name_str.trim_end_matches(&suffix).to_string();
        }
        quote!(
            /// Prints the docs for this object

            #[cfg(debug_assertions)]
            pub fn print_doc(dir: &str, summary: &mut String, tic: usize)->std::io::Result<()>{
                let doc = #docs.as_bytes();
                let filename = format!("auto-{}.md", #name_str).to_lowercase();
                let full_filename = format!("{}/{}", dir, filename);
                let tabs : String = std::iter::repeat('\t').take(tic).collect();
                #[allow(clippy::format_push_string)] // this is really not relevant... it happens only during testing, not at runtime.
                summary.push_str(&format!("{}- [{}](./reference_docs/{})\n",tabs,#name_str, filename));

                #q

                std::fs::write(&full_filename, doc)?;
                Ok(())
            }
        )
    }
}
