use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn lenient_properties<'de, D>(
    deserializer: D,
) -> Result<Option<HashMap<String, JsonSchema>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<HashMap<String, serde_json::Value>> = Option::deserialize(deserializer)?;
    Ok(raw.map(|m| {
        m.into_iter()
            .filter_map(|(k, v)| serde_json::from_value(v).ok().map(|s| (k, s)))
            .collect()
    }))
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct JsonSchema {
    #[serde(rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_type: Option<SchemaType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_properties"
    )]
    pub properties: Option<HashMap<String, JsonSchema>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<JsonSchema>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#enum: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub additional_properties: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SchemaType {
    #[serde(rename = "json_schema")]
    Schema,
    Object,
    Array,
    String,
    Number,
    Integer,
    Boolean,
    Null,
}

impl TryFrom<&str> for SchemaType {
    type Error = String;
    fn try_from(item: &str) -> Result<Self, Self::Error> {
        match item.to_lowercase().as_str() {
            "object" => Ok(SchemaType::Object),
            "array" => Ok(SchemaType::Array),
            "string" => Ok(SchemaType::String),
            "number" => Ok(SchemaType::Number),
            "int" | "integer" => Ok(SchemaType::Integer),
            "bool" | "boolean" => Ok(SchemaType::Boolean),
            "null" => Ok(SchemaType::Null),
            "json_schema" => Ok(SchemaType::Schema),
            other => Err(format!("Unknown schema type: '{other}'")),
        }
    }
}
impl From<SchemaType> for &str {
    fn from(item: SchemaType) -> Self {
        match item {
            SchemaType::Object => "object",
            SchemaType::Array => "array",
            SchemaType::String => "string",
            SchemaType::Number => "number",
            SchemaType::Integer => "integer",
            SchemaType::Boolean => "boolean",
            SchemaType::Null => "null",
            SchemaType::Schema => "json_schema",
        }
    }
}

fn sorted_opt_vec_eq(a: &Option<Vec<String>>, b: &Option<Vec<String>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(av), Some(bv)) => {
            let mut av = av.clone();
            av.sort();
            let mut bv = bv.clone();
            bv.sort();
            av == bv
        }
        _ => false,
    }
}

impl PartialEq for JsonSchema {
    fn eq(&self, other: &Self) -> bool {
        self.schema_type == other.schema_type
            && self.name == other.name
            && self.description == other.description
            && self.additional_properties == other.additional_properties
            && self.properties == other.properties
            && self.items == other.items
            && sorted_opt_vec_eq(&self.required, &other.required)
            && sorted_opt_vec_eq(&self.r#enum, &other.r#enum)
    }
}

#[cfg(test)]
mod test {
    use crate::schema::SchemaType;

    #[test]
    fn type_from_str() {
        let s = SchemaType::try_from("object").unwrap();
        assert!(matches!(s, SchemaType::Object));

        let s = SchemaType::try_from("array").unwrap();
        assert!(matches!(s, SchemaType::Array));

        let s = SchemaType::try_from("string").unwrap();
        assert!(matches!(s, SchemaType::String));

        let s = SchemaType::try_from("number").unwrap();
        assert!(matches!(s, SchemaType::Number));

        let s = SchemaType::try_from("int").unwrap();
        assert!(matches!(s, SchemaType::Integer));
        let s = SchemaType::try_from("integer").unwrap();
        assert!(matches!(s, SchemaType::Integer));

        let s = SchemaType::try_from("bool").unwrap();
        assert!(matches!(s, SchemaType::Boolean));
        let s = SchemaType::try_from("boolean").unwrap();
        assert!(matches!(s, SchemaType::Boolean));

        let s = SchemaType::try_from("null").unwrap();
        assert!(matches!(s, SchemaType::Null));

        let s = SchemaType::try_from("json_schema").unwrap();
        assert!(matches!(s, SchemaType::Schema));

        let s = SchemaType::try_from("???");
        assert!(s.is_err());
    }

    #[test]
    fn schema_to_str() {
        let s = SchemaType::Object;
        let s: &str = s.into();
        assert_eq!(s, "object");

        let s = SchemaType::Array;
        let s: &str = s.into();
        assert_eq!(s, "array");

        let s = SchemaType::String;
        let s: &str = s.into();
        assert_eq!(s, "string");

        let s = SchemaType::Number;
        let s: &str = s.into();
        assert_eq!(s, "number");

        let s = SchemaType::Integer;
        let s: &str = s.into();
        assert_eq!(s, "integer");

        let s = SchemaType::Boolean;
        let s: &str = s.into();
        assert_eq!(s, "boolean");

        let s = SchemaType::Null;
        let s: &str = s.into();
        assert_eq!(s, "null");

        let s = SchemaType::Schema;
        let s: &str = s.into();
        assert_eq!(s, "json_schema");
    }

    // ── JsonSchema::PartialEq ─────────────────────────────────────────────────

    use crate::schema::JsonSchema;
    use std::collections::HashMap;

    fn string_field(description: &str) -> JsonSchema {
        JsonSchema {
            schema_type: Some(SchemaType::String),
            description: Some(description.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_eq_identical_simple() {
        let a = string_field("a name");
        let b = string_field("a name");
        assert_eq!(a, b);
    }

    #[test]
    fn test_eq_different_description() {
        let a = string_field("foo");
        let b = string_field("bar");
        assert_ne!(a, b);
    }

    #[test]
    fn test_eq_different_type() {
        let a = JsonSchema {
            schema_type: Some(SchemaType::String),
            ..Default::default()
        };
        let b = JsonSchema {
            schema_type: Some(SchemaType::Number),
            ..Default::default()
        };
        assert_ne!(a, b);
    }

    #[test]
    fn test_eq_required_order_independent() {
        let a = JsonSchema {
            schema_type: Some(SchemaType::Object),
            required: Some(vec!["alpha".to_string(), "beta".to_string()]),
            ..Default::default()
        };
        let b = JsonSchema {
            schema_type: Some(SchemaType::Object),
            required: Some(vec!["beta".to_string(), "alpha".to_string()]),
            ..Default::default()
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_eq_required_different_elements() {
        let a = JsonSchema {
            required: Some(vec!["x".to_string()]),
            ..Default::default()
        };
        let b = JsonSchema {
            required: Some(vec!["y".to_string()]),
            ..Default::default()
        };
        assert_ne!(a, b);
    }

    #[test]
    fn test_eq_required_none_vs_some() {
        let a = JsonSchema {
            required: None,
            ..Default::default()
        };
        let b = JsonSchema {
            required: Some(vec![]),
            ..Default::default()
        };
        assert_ne!(a, b);
    }

    #[test]
    fn test_eq_enum_order_independent() {
        let a = JsonSchema {
            r#enum: Some(vec!["cat".to_string(), "dog".to_string()]),
            ..Default::default()
        };
        let b = JsonSchema {
            r#enum: Some(vec!["dog".to_string(), "cat".to_string()]),
            ..Default::default()
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_eq_properties_order_independent() {
        let mut props_a = HashMap::new();
        props_a.insert("name".to_string(), string_field("the name"));
        props_a.insert(
            "age".to_string(),
            JsonSchema {
                schema_type: Some(SchemaType::Number),
                ..Default::default()
            },
        );

        let mut props_b = HashMap::new();
        props_b.insert(
            "age".to_string(),
            JsonSchema {
                schema_type: Some(SchemaType::Number),
                ..Default::default()
            },
        );
        props_b.insert("name".to_string(), string_field("the name"));

        let a = JsonSchema {
            schema_type: Some(SchemaType::Object),
            properties: Some(props_a),
            ..Default::default()
        };
        let b = JsonSchema {
            schema_type: Some(SchemaType::Object),
            properties: Some(props_b),
            ..Default::default()
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_eq_properties_different_descriptions() {
        let mut props_a = HashMap::new();
        props_a.insert("name".to_string(), string_field("the name"));

        let mut props_b = HashMap::new();
        props_b.insert("name".to_string(), string_field("something else"));

        let a = JsonSchema {
            properties: Some(props_a),
            ..Default::default()
        };
        let b = JsonSchema {
            properties: Some(props_b),
            ..Default::default()
        };
        assert_ne!(a, b);
    }

    #[test]
    fn test_eq_items_recursive() {
        let a = JsonSchema {
            schema_type: Some(SchemaType::Array),
            items: Some(Box::new(string_field("an item"))),
            ..Default::default()
        };
        let b = JsonSchema {
            schema_type: Some(SchemaType::Array),
            items: Some(Box::new(string_field("an item"))),
            ..Default::default()
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_eq_items_different() {
        let a = JsonSchema {
            schema_type: Some(SchemaType::Array),
            items: Some(Box::new(string_field("an item"))),
            ..Default::default()
        };
        let b = JsonSchema {
            schema_type: Some(SchemaType::Array),
            items: Some(Box::new(string_field("different"))),
            ..Default::default()
        };
        assert_ne!(a, b);
    }

    #[test]
    fn test_eq_additional_properties() {
        let a = JsonSchema {
            additional_properties: true,
            ..Default::default()
        };
        let b = JsonSchema {
            additional_properties: false,
            ..Default::default()
        };
        assert_ne!(a, b);
    }
}
