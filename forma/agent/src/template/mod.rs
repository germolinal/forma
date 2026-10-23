use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use genai_client::{JsonSchema, schema::SchemaType};
use protocols::FormaError;
use regex::Regex;
use serde_json::Value;

const VAR_NAME_RE: &str = r"^[a-zA-Z_][a-zA-Z0-9_-]*$";
const FIELD_REGEX: &str =
    r#"\{\{\s*([a-zA-Z_][a-zA-Z0-9_-]*)(?:\s*:\s*([a-zA-Z]+))?(?:\s*"([^"]*)")?\s*\}\}"#;

/// Compiled once: templates are rendered on every LLM call, and building a
/// regex costs far more than running it.
static VAR_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(VAR_NAME_RE).expect("invalid var name regex"));
static FIELD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(FIELD_REGEX).expect("invalid field regex"));

pub fn is_valid_variable_name(name: &str) -> Result<(), String> {
    if VAR_NAME.is_match(name) {
        Ok(())
    } else {
        Err(format!(
            "this field only allows for alphanumneric values, and '_' and '-', and should not start with a number... received '{name}'"
        ))
    }
}

/// Checks if a specific field in a template is valid.
///
/// Fields can be fully described within a template using the following syntax:
///
/// > {{name: optional-type "optional-description"}}
///
/// Valid inputs are in the shape of `{{hola}}`, `{{hola: string}}`,
/// `{{hola "greeting"}}` and `{{hola: string "greeting"}}`
pub fn is_valid_template_field(field: &str) -> Result<(), String> {
    if FIELD.is_match(field) {
        Ok(())
    } else {
        Err("template fields should be in the format {{name:optional-type \"optional-description\"}}, where type and description are optional".to_string())
    }
}

fn render_value(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => s.clone(),
        _ => val.to_string(), // numbers, bools, objects, arrays
    }
}

/// Renders a template, replacing all the fields within the `session`
/// into the template. It will fail if there is a missing variable,
/// and will not check types.
pub fn render_template(template: &str, session: &HashMap<String, Value>) -> Result<String, String> {
    let mut rendered = String::with_capacity(template.len());
    let mut last_idx = 0;

    for caps in FIELD.captures_iter(template) {
        let m = caps.get(0).unwrap();
        let var_name = caps.get(1).unwrap().as_str();

        // copy text before match
        rendered.push_str(&template[last_idx..m.start()]);

        if let Some(val) = session.get(var_name) {
            rendered.push_str(&render_value(val));
        } else {
            return Err(format!("Missing value for variable '{var_name}'"));
        }

        last_idx = m.end();
    }

    // copy remaining tail
    rendered.push_str(&template[last_idx..]);

    Ok(rendered)
}

pub fn extract_placeholder_names(template: &str) -> Result<HashSet<String>, FormaError> {
    let mut placeholders = HashSet::new();
    for cap in FIELD.captures_iter(template) {
        if let Some(name) = cap.get(1) {
            let id = name.as_str().to_string();
            placeholders.insert(id);
        } else {
            return Err("expected placeholder name".to_string().into());
        }
    }
    placeholders.shrink_to_fit();
    Ok(placeholders)
}

pub fn extract_template_schema(template: &str) -> Result<JsonSchema, FormaError> {
    let mut properties = HashMap::new();
    let mut required = Vec::new();

    for cap in FIELD.captures_iter(template) {
        let name = cap.get(1).expect("expected placeholder name").as_str();
        let type_str = cap.get(2).map(|m| m.as_str()).unwrap_or("string");
        let desc = cap.get(3).map(|m| m.as_str());

        let schema_type: SchemaType = type_str.try_into()?;
        match schema_type {
            SchemaType::Boolean | SchemaType::Integer | SchemaType::Number | SchemaType::String => {
                let mut field = JsonSchema {
                    schema_type: Some(schema_type),
                    ..Default::default()
                };
                if let Some(d) = desc {
                    field.description = Some(d.to_string());
                }

                properties.insert(name.to_string(), field);
                required.push(name.to_string());
            }
            SchemaType::Array | SchemaType::Object | SchemaType::Null | SchemaType::Schema => {
                return Err(format!("type '{type_str}' is not yet supported in templates").into());
            }
        };
    }
    properties.shrink_to_fit();
    required.shrink_to_fit();
    Ok(JsonSchema {
        schema_type: Some(SchemaType::Object),
        properties: Some(properties),
        required: Some(required),
        ..Default::default()
    })
}

#[cfg(test)]
mod testing {
    use super::render_template;
    use crate::template::{
        extract_template_schema, is_valid_template_field, is_valid_variable_name,
    };
    use genai_client::{JsonSchema, schema::SchemaType};
    use protocols::FormaError;
    use std::collections::HashMap;

    #[test]
    fn plain_variable() -> Result<(), String> {
        let template = r"Hello, {{world}}".to_string();
        let mut state = HashMap::new();
        state.insert(
            "world".to_string(),
            serde_json::Value::String("Snowy.".to_string()),
        );
        let ret = render_template(&template, &state)?;
        assert_eq!(ret, "Hello, Snowy.");
        Ok(())
    }

    #[test]
    fn variable_with_dash() -> Result<(), String> {
        let template = r"Hello, {{big-world}}".to_string();
        let mut state = HashMap::new();
        state.insert(
            "big-world".to_string(),
            serde_json::Value::String("Snowy.".to_string()),
        );
        let ret = render_template(&template, &state)?;
        assert_eq!(ret, "Hello, Snowy.");
        Ok(())
    }

    #[test]
    fn variable_with_underscore() -> Result<(), String> {
        let template = r"Hello, {{big_world}}".to_string();
        let mut state = HashMap::new();
        state.insert(
            "big_world".to_string(),
            serde_json::Value::String("Snowy.".to_string()),
        );
        let ret = render_template(&template, &state)?;
        assert_eq!(ret, "Hello, Snowy.");
        Ok(())
    }

    fn check_types(
        schema: JsonSchema,
        exp: Vec<(&str, SchemaType, Option<&str>)>,
    ) -> Result<(), FormaError> {
        for (name, expected_type, description) in exp {
            let props = schema.properties.clone().unwrap();
            let field = props.get(name).unwrap();
            let typ = field.schema_type.unwrap();
            let exp_type_str: &str = expected_type.into();
            let typ_str: &str = typ.into();
            if typ != expected_type {
                return Err(FormaError::from(format!(
                    "Type mismatch for field '{}': expected '{}', got '{}'",
                    name, exp_type_str, typ_str
                )));
            }
            let desc = field.description.clone();
            if let Some(d) = description {
                if desc.is_none() {
                    return Err("expected no description, but found one..."
                        .to_string()
                        .into());
                }
                assert_eq!(desc.unwrap(), d);
            } else {
                if desc.is_some() {
                    return Err("expected no description, but found none..."
                        .to_string()
                        .into());
                }
            }
        }
        Ok(())
    }

    #[test]
    fn test_extract_schema_default() -> Result<(), FormaError> {
        let template: String = r"Hello, {{world}}".to_string();
        let s = extract_template_schema(&template)?;
        check_types(s, vec![("world", SchemaType::String, None)])
    }

    #[test]
    fn test_extract_schema_string() -> Result<(), FormaError> {
        let template: String = r"Hello, {{world:string}}".to_string();
        let s = extract_template_schema(&template)?;
        check_types(s, vec![("world", SchemaType::String, None)])
    }
    #[test]
    fn test_extract_schema_object() {
        // Objects are not supported
        let template: String = r"Hello, {{world:object}}".to_string();
        assert!(extract_template_schema(&template).is_err());
        // check_types(s, vec![("world", SchemaType::Object, None)])
    }

    #[test]
    fn test_extract_schema_number() -> Result<(), FormaError> {
        let template: String = r"Hello, {{world:number}}".to_string();
        let s = extract_template_schema(&template)?;
        check_types(s, vec![("world", SchemaType::Number, None)])
    }
    #[test]
    fn test_extract_schema_int() -> Result<(), FormaError> {
        let template: String = r"Hello, {{world:int}}".to_string();
        let s = extract_template_schema(&template)?;
        check_types(s, vec![("world", SchemaType::Integer, None)])
    }
    #[test]
    fn test_extract_schema_integer() -> Result<(), FormaError> {
        let template: String = r"Hello, {{world:integer}}".to_string();
        let s = extract_template_schema(&template)?;
        check_types(s, vec![("world", SchemaType::Integer, None)])
    }

    #[test]
    fn test_extract_schema_bool() -> Result<(), FormaError> {
        let template: String = r"Hello, {{world:bool}}".to_string();
        let s = extract_template_schema(&template)?;
        check_types(s, vec![("world", SchemaType::Boolean, None)])
    }
    #[test]
    fn test_extract_schema_boolean() -> Result<(), FormaError> {
        let template: String = r"Hello, {{world:boolean}}".to_string();
        let s = extract_template_schema(&template)?;
        check_types(s, vec![("world", SchemaType::Boolean, None)])
    }
    #[test]
    fn test_extract_schema_boolean_spacing() -> Result<(), FormaError> {
        let template: String = r"Hello, {{world   :    boolean}}".to_string();
        let s = extract_template_schema(&template)?;
        check_types(s, vec![("world", SchemaType::Boolean, None)])
    }

    #[test]
    fn test_extract_schema_default_desc() -> Result<(), FormaError> {
        let template: String = r#"Hello, {{world "the variable"}}"#.to_string();
        let s = extract_template_schema(&template)?;
        check_types(s, vec![("world", SchemaType::String, Some("the variable"))])
    }
    #[test]
    fn test_extract_schema_default_desc_spacing_1() -> Result<(), FormaError> {
        let template: String = r#"Hello, {{world"the variable"}}"#.to_string();
        let s = extract_template_schema(&template)?;
        check_types(s, vec![("world", SchemaType::String, Some("the variable"))])
    }
    #[test]
    fn test_extract_schema_default_desc_spacing_2() -> Result<(), FormaError> {
        let template: String = r#"Hello, {{world     "the variable"   }}"#.to_string();
        let s = extract_template_schema(&template)?;
        check_types(s, vec![("world", SchemaType::String, Some("the variable"))])
    }

    #[test]
    fn test_extract_schema_string_desc() -> Result<(), FormaError> {
        let template: String = r#"Hello, {{world:string"the variable"}}"#.to_string();
        let s = extract_template_schema(&template)?;
        check_types(s, vec![("world", SchemaType::String, Some("the variable"))])
    }
    #[test]
    fn test_extract_schema_object_described() {
        // Objects are not supported
        let template: String = r#"Hello, {{world:object "the variable"}}"#.to_string();
        assert!(extract_template_schema(&template).is_err());
    }

    #[test]
    fn test_extract_schema_number_desc() -> Result<(), FormaError> {
        let template: String = r#"Hello, {{world:number "the variable"}}"#.to_string();
        let s = extract_template_schema(&template)?;
        check_types(s, vec![("world", SchemaType::Number, Some("the variable"))])
    }
    #[test]
    fn test_extract_schema_int_desc() -> Result<(), FormaError> {
        let template: String = r#"Hello, {{world:int "the variable"}}"#.to_string();
        let s = extract_template_schema(&template)?;
        check_types(
            s,
            vec![("world", SchemaType::Integer, Some("the variable"))],
        )
    }
    #[test]
    fn test_extract_schema_integer_desc() -> Result<(), FormaError> {
        let template: String = r#"Hello, {{world:integer "the variable"}}"#.to_string();
        let s = extract_template_schema(&template)?;
        check_types(
            s,
            vec![("world", SchemaType::Integer, Some("the variable"))],
        )
    }

    #[test]
    fn test_extract_schema_bool_desc() -> Result<(), FormaError> {
        let template: String = r#"Hello, {{world:bool "the variable"}}"#.to_string();
        let s = extract_template_schema(&template)?;
        check_types(
            s,
            vec![("world", SchemaType::Boolean, Some("the variable"))],
        )
    }
    #[test]
    fn test_extract_schema_boolean_desc() -> Result<(), FormaError> {
        let template: String = r#"Hello, {{world:boolean "the variable"}}"#.to_string();
        let s = extract_template_schema(&template)?;
        check_types(
            s,
            vec![("world", SchemaType::Boolean, Some("the variable"))],
        )
    }

    #[test]
    fn valid_variable_name() {
        assert!(is_valid_variable_name("name").is_ok());
        assert!(is_valid_variable_name("other-name").is_ok());
        assert!(is_valid_variable_name("underscore_name").is_ok());
        assert!(is_valid_variable_name("BIG_NAME").is_ok());
        assert!(is_valid_variable_name("BIG-NAME2").is_ok());
        assert!(is_valid_variable_name("asfas123").is_ok());
        assert!(is_valid_variable_name("a123").is_ok());
        assert!(is_valid_variable_name("_name").is_ok());

        assert!(is_valid_variable_name("-other-name").is_err());
        assert!(is_valid_variable_name("1word").is_err());
        assert!(is_valid_variable_name("user.name").is_err());
        assert!(is_valid_variable_name("BIG/NAME").is_err());
        assert!(is_valid_variable_name("$user").is_err());
    }

    #[test]
    fn valid_template_field() {
        assert!(is_valid_template_field("{{hola}}").is_ok());
        assert!(is_valid_template_field("{{hola: string}}").is_ok());
        assert!(is_valid_template_field("{{hola: string \"the greeting\"}}").is_ok());

        assert!(is_valid_template_field("{{hola: string \"mismatched quote}}").is_err());
        assert!(is_valid_template_field("{{ user:\"string\" }}").is_err());
        assert!(is_valid_template_field("{{ user:\"object\" }}").is_err());
        assert!(is_valid_template_field("{{ user:\"array\" }}").is_err());
        assert!(is_valid_template_field("{{ user:\"ints\" }}").is_err());
    }
}
