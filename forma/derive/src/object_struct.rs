use crate::field::Field;
use quote::quote;
use std::fmt::Write as _; // import without risk of name clashing

pub struct StructObject {
    pub ident: syn::Ident,
    pub fields: Vec<Field>,
    docs: String,
    // optional_fields: Vec<Field>,
    // state_fields: Vec<Field>,
}

impl StructObject {
    #[cfg(not(tarpaulin_include))]
    pub(crate) fn new(ident: syn::Ident, stru: syn::DataStruct, docs: String) -> Self {
        let fields: Vec<Field> = Self::get_object_fields(&stru)
            .into_iter()
            .filter_map(|x| Field::new(x).ok())
            .collect();

        StructObject {
            ident,
            fields,
            docs,
        }
    }

    #[cfg(not(tarpaulin_include))]
    pub(crate) fn gen_docs(&self) -> Result<String, String> {
        let mut ret = String::new();

        // Title
        let suffix = "Definition".to_string();
        let mut title = format!("{}", self.ident);
        if title.ends_with(&suffix) {
            title = title.trim_end_matches(&suffix).to_string();
        }
        writeln!(ret, "# {}\n", title).map_err(|e| e.to_string())?;

        // Written doc
        let docs = &self.docs;
        writeln!(ret, "{}\n", docs).map_err(|e| e.to_string())?;
        if self.fields.iter().all(|f| matches!(f, Field::State(_))) {
            // Nothing to set, and so nothing to specify.
            return Ok(ret);
        }
        writeln!(ret, "## Full Specification\n").map_err(|e| e.to_string())?;

        // A codeblock with the all the fields
        ret += "```yaml\n"; // Open template
        // writeln!(ret, "{} {{", self.ident).map_err(|e| e.to_string())?;

        for field in self.fields.iter() {
            if let Field::State(_) = field {
                continue;
            }
            if field.data().flatten {
                // Its fields are not under its name, but next to these.
                let ty = field.flattened_type()?;
                writeln!(ret, "# ...and the fields of {ty}, at this level")
                    .map_err(|e| e.to_string())?;
                continue;
            }
            writeln!(ret, "{}", field.get_documentation()?).map_err(|e| e.to_string())?;
        }
        ret += "```\n\n";

        // Documentations for fields
        for field in self.fields.iter() {
            let f_ident = field.data().ident.clone().ok_or("No identity")?;
            if f_ident == "index" {
                continue;
            }
            if let Field::State(_) = field {
                continue;
            }

            if field.data().flatten {
                let ty = field.flattened_type()?;
                write!(
                    ret,
                    "#### The fields of [`{ty}`](./auto-{}.md)",
                    ty.to_lowercase()
                )
                .map_err(|e| e.to_string())?;
            } else {
                write!(ret, "#### `{}`", f_ident).map_err(|e| e.to_string())?;
            }
            if let Field::Option(_) = field {
                ret += " (*optional*)";
            }
            ret += "\n\n";

            let errmsg = format!("Field '{}' has no docs", f_ident);
            writeln!(ret, "{}", field.data().docs.expect(&errmsg)).map_err(|e| e.to_string())?;
        }
        ret += "\n";

        Ok(ret)
    }

    fn get_object_fields(
        stru: &syn::DataStruct,
    ) -> syn::punctuated::Punctuated<syn::Field, syn::token::Comma> {
        if let syn::DataStruct {
            fields: syn::Fields::Named(syn::FieldsNamed { named, .. }),
            ..
        } = stru
        {
            named.clone()
        } else {
            panic!(
                "Unhandled object when get_object_fields... {:?}",
                quote!(stru)
            );
        }
    }
}
