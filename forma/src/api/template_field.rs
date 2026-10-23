use std::fs;

use protocols::FormaError;
use serde::{Deserialize, Serialize};

use crate::api::ObjectDefinition;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged, bound = "T: ObjectDefinition")]
pub(crate) enum TemplateField<T> {
    File(String),
    Object(T),
}

/// Returns true if the string starts with a '$',
/// indicating it's a filename.
fn is_filename(s: &str) -> bool {
    s.starts_with('$')
}

/// Reads the contents of a file and returns it as a string.
fn read_file(filename: &str) -> Result<String, FormaError> {
    if !fs::metadata(filename).map(|m| m.is_file()).unwrap_or(false) {
        return Err(format!("File '{filename}' does not exist or is not a file").into());
    }
    let file_content = fs::read_to_string(filename).map_err(|e| e.to_string())?;
    Ok(file_content)
}

/// Takes a string and expands it; meaning that, if it is a filename (starts with `$`),
/// it will be expanded to a full path using the parent directory.
pub(crate) fn expand_string(mut s: String, parent_dir: &str) -> Result<String, FormaError> {
    if is_filename(&s) {
        s.remove(0);
        let s = format!("{parent_dir}/{s}");
        read_file(&s)
    } else {
        Ok(s)
    }
}

impl<T> ObjectDefinition for TemplateField<T>
where
    T: ObjectDefinition + Serialize + for<'a> Deserialize<'a> + 'static,
{
    /// Expands the template field, resolving any nested references.
    /// If the field is a file reference (starts with `$`), it will
    /// be expanded to a full path using the parent directory. It allows
    /// for `json` and `yaml` file references.
    fn expand(self, parent_dir: &str) -> Result<Self, FormaError> {
        match self {
            TemplateField::File(mut filename) => {
                if !is_filename(&filename) {
                    return Err("file references must start with '$'".to_string().into());
                }
                // remove the dollar sign
                filename.remove(0);
                let filename = format!("{parent_dir}/{filename}");
                let str = read_file(&filename)?;
                if filename.ends_with(".json") {
                    let a = serde_json::from_str::<T>(&str)?;
                    let a = a.expand(parent_dir)?;
                    Ok(Self::Object(a))
                } else if filename.ends_with(".yaml") {
                    let a = serde_yaml::from_str::<T>(&str).map_err(|e| e.to_string())?;
                    let a = a.expand(parent_dir)?;
                    Ok(Self::Object(a))
                } else {
                    unimplemented!("unsupported file template extention in '{filename}'")
                }
                // maybe: serde_json::from_str(&fs::read_to_string(filename).unwrap()).unwrap()
            }
            TemplateField::Object(_) => Ok(self),
        }
    }

    /// Checks the object this field holds. A field that is still a file
    /// reference has nothing to check: reading it is `expand`'s job, and a
    /// definition is expanded before it is checked.
    fn check(self) -> Result<Self, FormaError> {
        match self {
            TemplateField::Object(o) => Ok(Self::Object(o.check()?)),
            TemplateField::File(_) => Ok(self),
        }
    }
}

#[cfg(test)]
mod testing {

    use super::*;
    use std::io::Write;

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct Dummy {
        name: String,
        value: i32,
    }

    impl ObjectDefinition for Dummy {
        fn expand(self, _parent_dir: &str) -> Result<Self, FormaError> {
            Ok(self)
        }
    }

    #[test]
    fn test_serialize_deserialize_file() {
        let field = TemplateField::<Dummy>::File("test.txt".to_string());
        let yaml = serde_yaml::to_string(&field).unwrap().trim().to_string();
        println!("Serialized File:\n{}", &yaml);
        assert_eq!("test.txt", yaml.clone());

        let deserialized: TemplateField<Dummy> = serde_yaml::from_str(&yaml).unwrap();
        match deserialized {
            TemplateField::File(v) => assert_eq!(v, yaml),
            _ => panic!("This is not what was expected"),
        };
    }

    #[test]
    fn test_serialize_deserialize_object() {
        let dummy = Dummy {
            name: "example".to_string(),
            value: 42,
        };
        let field = TemplateField::Object(dummy.clone());
        let yaml = serde_yaml::to_string(&field).unwrap().trim().to_string();
        assert_eq!("name: example\nvalue: 42", yaml);

        let deserialized: TemplateField<Dummy> = serde_yaml::from_str(&yaml).unwrap();
        match deserialized {
            TemplateField::Object(v) => {
                assert_eq!(v.value, 42);
                assert_eq!(v.name, "example".to_string());
            }
            _ => panic!("This is not what was expected"),
        };
    }

    // ── is_filename ─────────────────────────────────────────────────────────

    #[test]
    fn test_is_filename_with_dollar() {
        assert!(is_filename("$some/file.yaml"));
    }

    #[test]
    fn test_is_filename_without_dollar() {
        assert!(!is_filename("some/file.yaml"));
        assert!(!is_filename(""));
        assert!(!is_filename("plain string"));
    }

    // ── read_file ────────────────────────────────────────────────────────────

    #[test]
    fn test_read_file_success() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "hello").unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let result = read_file(&path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn test_read_file_missing() {
        let result = read_file("/this/path/does/not/exist.txt");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("does not exist or is not a file"));
    }

    #[test]
    fn test_read_file_directory_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap().to_string();
        let result = read_file(&path);
        assert!(result.is_err());
    }

    // ── expand_string ────────────────────────────────────────────────────────

    #[test]
    fn test_expand_string_plain_returns_as_is() {
        let result = expand_string("hello world".to_string(), "/some/dir");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "hello world");
    }

    #[test]
    fn test_expand_string_file_ref_expands() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("content.txt");
        fs::write(&file_path, "file content").unwrap();
        let parent = dir.path().to_str().unwrap();

        let result = expand_string("$content.txt".to_string(), parent);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "file content");
    }

    #[test]
    fn test_expand_string_missing_file_returns_err() {
        let result = expand_string("$nonexistent.txt".to_string(), "/tmp");
        assert!(result.is_err());
    }

    // ── TemplateField::expand ─────────────────────────────────────────────────

    #[test]
    fn test_expand_object_variant_returns_self() {
        let dummy = Dummy {
            name: "x".to_string(),
            value: 1,
        };
        let field = TemplateField::Object(dummy.clone());
        let result = field.expand("/irrelevant");
        assert!(result.is_ok());
        match result.unwrap() {
            TemplateField::Object(v) => assert_eq!(v, dummy),
            _ => panic!("expected Object variant"),
        }
    }

    #[test]
    fn test_expand_file_variant_without_dollar_returns_err() {
        let field = TemplateField::<Dummy>::File("no_dollar.json".to_string());
        let result = field.expand("/irrelevant");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must start with '$'")
        );
    }

    #[test]
    fn test_expand_file_variant_json() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("data.json");
        fs::write(&file_path, r#"{"name":"alice","value":7}"#).unwrap();
        let parent = dir.path().to_str().unwrap();

        let field = TemplateField::<Dummy>::File("$data.json".to_string());
        let result = field.expand(parent);
        assert!(result.is_ok());
        match result.unwrap() {
            TemplateField::Object(v) => {
                assert_eq!(v.name, "alice");
                assert_eq!(v.value, 7);
            }
            _ => panic!("expected Object variant"),
        }
    }

    #[test]
    fn test_expand_file_variant_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("data.yaml");
        fs::write(&file_path, "name: bob\nvalue: 99\n").unwrap();
        let parent = dir.path().to_str().unwrap();

        let field = TemplateField::<Dummy>::File("$data.yaml".to_string());
        let result = field.expand(parent);
        assert!(result.is_ok());
        match result.unwrap() {
            TemplateField::Object(v) => {
                assert_eq!(v.name, "bob");
                assert_eq!(v.value, 99);
            }
            _ => panic!("expected Object variant"),
        }
    }

    #[test]
    fn test_expand_file_variant_missing_file_returns_err() {
        let field = TemplateField::<Dummy>::File("$missing.json".to_string());
        let result = field.expand("/tmp");
        assert!(result.is_err());
    }
}
