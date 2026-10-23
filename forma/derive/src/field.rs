use crate::common_path::*;
use quote::quote;
use syn::{Meta, MetaList, MetaNameValue};

pub const STATE_ELEMENT_TYPE: &str = "StateElementField";

#[derive(Clone, Debug)]
pub struct Attribute {
    pub name: String,
    pub value: Option<String>,
}

impl Attribute {
    #[cfg(not(tarpaulin_include))]
    pub(crate) fn new(a: &syn::Attribute) -> Self {
        if let syn::AttrStyle::Inner(_) = &a.style {
            panic!("Expecing Outer style attribute")
        }
        let name = a.path().segments[0].ident.clone();
        let mut value: Option<String> = None;

        if name != "doc" {
            value = match &a.meta {
                Meta::Path(_) => None,
                Meta::NameValue(MetaNameValue { .. }) => {
                    unreachable!(
                        "I do not think we support attributes of this kind... please report this"
                    );
                }
                Meta::List(MetaList {
                    path: _, tokens, ..
                }) => Some(format!("{}", tokens)),
            };
        }

        Self {
            name: format!("{}", name),
            value,
        }
    }
}

#[derive(Clone)]
pub struct FieldData {
    pub ident: Option<syn::Ident>,
    pub child: Option<Box<Field>>,
    pub ty: syn::Type,
    pub docs: Option<String>,
    /// `#[serde(flatten)]`: the field's own fields are written in its
    /// parent's, not under its name.
    pub flatten: bool,
}

#[derive(Clone)]
pub enum Field {
    Float(FieldData),
    Int(FieldData),
    Bool(FieldData),
    String(FieldData),

    Option(FieldData),
    Vec(FieldData),
    Rc(FieldData),

    Object(FieldData),
    State(FieldData),
}

impl Field {
    /// Creates a new Field object, with no ident and no attributes.
    ///
    /// This method is meant to be used for nested typs. E.g. the `usize` in `Vec<usize>`
    #[cfg(not(tarpaulin_include))]
    pub(crate) fn from_type(ty: &syn::Type) -> Result<Self, String> {
        let mut data = FieldData {
            ident: None,
            child: None,
            ty: ty.clone(),
            docs: None, // This is nested... the Docs should be in the parent
            flatten: false,
        };

        if let syn::Type::Path(p) = ty {
            if path_is_float(&p.path)? {
                Ok(Self::Float(data))
            } else if path_is_int(&p.path)? {
                Ok(Self::Int(data))
            } else if path_is_bool(&p.path)? {
                Ok(Self::Bool(data))
            } else if path_is_string(&p.path)? {
                Ok(Self::String(data))
            } else if path_is_option(&p.path)? {
                let ty = extract_type_from_path(&p.path)?;
                let aux = Self::from_type(&ty)?;
                data.child = Some(Box::new(aux));
                Ok(Self::Option(data))
            } else if path_is_vec(&p.path)? {
                let ty = extract_type_from_path(&p.path)?;
                let aux = Self::from_type(&ty)?;
                data.child = Some(Box::new(aux));
                Ok(Self::Vec(data))
            } else if path_is_rc(&p.path)? {
                let ty = extract_type_from_path(&p.path)?;
                let aux = Self::from_type(&ty)?;
                data.child = Some(Box::new(aux));
                Ok(Self::Rc(data))
            } else {
                let ty_str = path_to_string(&p.path)?;

                if ty_str == STATE_ELEMENT_TYPE {
                    return Ok(Self::State(data));
                }
                // Therea re object—e.g., enums—that are stored in other object, not in models
                Ok(Self::Object(data))
            }
        } else {
            panic!("TODO: Error handling 3")
        }
    }

    /// Creates a new Field object based on the tokens in an actual Struct.
    #[cfg(not(tarpaulin_include))]
    pub(crate) fn new(field: syn::Field) -> Result<Self, String> {
        let ident = field.ident.clone();

        if let syn::Type::Path(t) = &field.ty {
            let attributes: Vec<Attribute> = field.attrs.iter().map(Attribute::new).collect();

            let mut api_alias: Option<String> = None;
            for a in attributes.iter() {
                if a.name == "physical" || a.name == "operational" {
                    api_alias.clone_from(&a.value);
                    break;
                }
            }

            let flatten = attributes.iter().any(|a| {
                a.name == "serde"
                    && a.value
                        .as_deref()
                        .is_some_and(|v| v.split(',').any(|arg| arg.trim() == "flatten"))
            });

            let path = t.path.clone();
            let ty = field.ty.clone();
            let docs = Some(crate::docs::get_docs(&field.attrs)?);

            let mut data = FieldData {
                ident,
                ty,
                docs,
                child: None,
                flatten,
            };

            if path_is_float(&path)? {
                Ok(Self::Float(data))
            } else if path_is_int(&path)? {
                Ok(Self::Int(data))
            } else if path_is_bool(&path)? {
                Ok(Self::Bool(data))
            } else if path_is_string(&path)? {
                Ok(Self::String(data))
            } else if path_is_option(&path)? {
                let ty = extract_type_from_path(&path)?;
                let aux = Self::from_type(&ty)?;
                data.child = Some(Box::new(aux));
                Ok(Self::Option(data))
            } else if path_is_vec(&path)? {
                let ty = extract_type_from_path(&path)?;
                let aux = Self::from_type(&ty)?;
                data.child = Some(Box::new(aux));
                Ok(Self::Vec(data))
            } else if path_is_rc(&path)? {
                let ty = extract_type_from_path(&path)?;
                let aux = Self::from_type(&ty)?;
                data.child = Some(Box::new(aux));
                Ok(Self::Rc(data))
            } else {
                let ty_str = path_to_string(&path)?;
                if ty_str == STATE_ELEMENT_TYPE {
                    return Ok(Self::State(data));
                }
                // Therea re object—e.g., enums—that are stored in other object, not in models
                Ok(Self::Object(data))
            }
        } else {
            panic!(
                "Unhandled syn::Type '{:?}' when Field::new()",
                quote!(field.ty)
            )
        }
    }

    #[cfg(not(tarpaulin_include))]
    pub(crate) fn data(&self) -> FieldData {
        match self {
            Self::Float(d)
            | Self::Int(d)
            | Self::Bool(d)
            | Self::String(d)
            | Self::Vec(d)
            | Self::Rc(d)
            | Self::Option(d)
            | Self::Object(d)
            | Self::State(d) => d.clone(),
        }
    }

    #[cfg(not(tarpaulin_include))]
    pub(crate) fn get_documentation_type(&self) -> Result<String, String> {
        let r = match self {
            Field::Float(_) => "number".to_string(),
            Field::Int(_) => "int".to_string(),
            Field::Bool(_d) => "boolean".to_string(),
            Field::String(_d) => "string".to_string(),

            Field::Vec(d) => {
                let child_type = d
                    .child
                    .clone()
                    .ok_or("no child?")?
                    .get_documentation_type()?;
                format!("\n\t- {}\n\t- ...", child_type)
            }
            Field::Rc(d) => d
                .child
                .clone()
                .ok_or("no child found")?
                .get_documentation_type()?,
            Field::Option(d) => {
                let child_type = d
                    .child
                    .clone()
                    .ok_or("no child?")?
                    .get_documentation_type()?;
                format!("{} # optional", child_type)
            }
            Field::Object(d) => {
                if let syn::Type::Path(t) = &d.ty {
                    let mut title = path_to_string(&t.path)?;
                    let suffix = "Definition".to_string();
                    if title.ends_with(&suffix) {
                        title = title.trim_end_matches(&suffix).to_string();
                    }
                    title
                } else {
                    unreachable!("Weird object when getting docs")
                }
            }
            Field::State(_d) => {
                unreachable!("Trying to doc-type of State field")
            }
        };
        Ok(r)
    }

    /// The name of the type a `#[serde(flatten)]` field holds, as its page is
    /// titled. Only objects (structs and enums with their own docs) can be
    /// flattened.
    #[cfg(not(tarpaulin_include))]
    pub(crate) fn flattened_type(&self) -> Result<String, String> {
        match self {
            Field::Object(_) => self.get_documentation_type(),
            _ => Err(format!(
                "field '{}': only objects can be flattened",
                self.data().ident.map(|i| i.to_string()).unwrap_or_default()
            )),
        }
    }

    #[cfg(not(tarpaulin_include))]
    pub(crate) fn get_documentation(&self) -> Result<String, String> {
        let f_ident = self.data().ident.ok_or("No ident")?;

        let d = match self {
            Field::Float(_)
            | Field::Int(_)
            | Field::Bool(_)
            | Field::String(_)
            | Field::Vec(_)
            | Field::Rc(_)
            | Field::Object(_)
            | Field::Option(_) => {
                format!("{}: {}", f_ident, self.get_documentation_type()?)
            }
            Field::State(_d) => {
                panic!("Trying to get verification for State field")
            }
        };
        Ok(d)
    }
}
