use std::fmt::Write as _;

use syn::{Lit, Meta, MetaNameValue}; // import without risk of name clashing

#[cfg(not(tarpaulin_include))]
pub(crate) fn get_docs(attrs: &[syn::Attribute]) -> Result<String, String> {
    // let mut ret = String::new();
    let mut ret = "".to_string();

    for at in attrs {
        if let Some(segment) = at.path().segments.iter().next() {
            let segment_ident = format!("{}", segment.ident);
            if "doc" == segment_ident {
                // `value()` unescapes the literal, so quotes and backslashes in
                // the doc comment reach the Markdown as they were written.
                let doc = match &at.meta {
                    Meta::NameValue(MetaNameValue { path: _, value, .. }) => {
                        if let syn::Expr::Lit(v) = value {
                            if let Lit::Str(token) = &v.lit {
                                token.value()
                            } else {
                                unreachable!()
                            }
                        } else {
                            unreachable!()
                        }
                    }
                    _ => unreachable!(
                        "Docs should not be anything than NameValue... please report this"
                    ),
                };

                // Drop the single space that follows `///`, and nothing else:
                // any further indentation is meaningful to Markdown.
                let doc = doc.strip_prefix(' ').unwrap_or(&doc);

                writeln!(ret, "{}", doc).map_err(|e| e.to_string())?;
            }
        }
    }

    Ok(ret)
}
