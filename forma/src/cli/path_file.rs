use std::path::Path;

#[derive(Debug)]
pub(crate) struct PathAndFile {
    pub(crate) path: String,
    pub(crate) filename: String,
}

pub(crate) fn assume_file_format(filename: &str) -> String {
    if filename.ends_with(".json") {
        return filename.to_string();
    }
    if filename.ends_with(".yaml") {
        return filename.to_string();
    }
    format!("{}.yaml", filename)
}

impl PathAndFile {
    /// Checks that filename is either YAML or JSON; and modifies
    /// the name of Filename to contain not only the file name, but also
    /// the preceding path.
    fn check_filename(mut self) -> Result<Self, String> {
        self.filename = assume_file_format(&self.filename);
        self.filename = format!("{}/{}", self.path, self.filename);

        // Check if file exists
        if !Path::new(&self.filename).exists() {
            let err = format!("  Error: File '{}' does not exist", self.filename);
            return Err(err);
        }
        Ok(self)
    }
    fn check_path(mut self) -> Result<Self, String> {
        if self.path.ends_with('/') {
            self.path = self.path.trim_end_matches('/').to_string();
        }
        // Check if path exists
        let path_obj = Path::new(&self.path);
        if !path_obj.exists() {
            return Err(format!("  Error: Path '{}' does not exist", self.path));
        }
        if !path_obj.is_dir() {
            return Err(format!("  Error: Path '{}' is not a directory", self.path));
        }

        Ok(self)
    }

    pub fn validate(self) -> Self {
        let s = match self.check_path() {
            Err(e) => {
                eprint!("{e}");
                std::process::exit(1)
            }
            Ok(v) => v,
        };
        match s.check_filename() {
            Err(e) => {
                eprint!("{e}");
                std::process::exit(1)
            }
            Ok(v) => v,
        }
    }
}

#[cfg(test)]
mod test {

    use crate::cli::path_file::assume_file_format;

    use super::PathAndFile;
    use std::env;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;

    #[test]
    fn test_no_path() {
        let pathname = "the-path".to_string();
        let filename = "the-file".to_string();

        // First, path does not exist
        let filepath = PathAndFile {
            path: pathname.clone(),
            filename: filename.clone(),
        };

        let res = filepath.check_path();
        assert!(res.is_err());
        if let Err(msg) = res {
            assert_eq!(msg, "  Error: Path 'the-path' does not exist".to_string())
        }
    }

    #[test]
    fn test_no_file_no_extension() {
        // First, no extension
        let filepath = PathAndFile {
            path: "the-path".to_string(),
            filename: "the-file".to_string(),
        };
        let res = filepath.check_filename();
        assert!(res.is_err());
        if let Err(msg) = res {
            assert_eq!(
                msg,
                "  Error: File 'the-path/the-file.yaml' does not exist".to_string()
            )
        }
    }
    #[test]
    fn test_no_file_yaml() {
        // with YAML extension
        let filepath = PathAndFile {
            path: "the-path".to_string(),
            filename: "the-file.yaml".to_string(),
        };
        let res = filepath.check_filename();
        assert!(res.is_err());
        if let Err(msg) = res {
            assert_eq!(
                msg,
                "  Error: File 'the-path/the-file.yaml' does not exist".to_string()
            )
        }
    }
    #[test]
    fn test_no_file_json() {
        // with JSON extension
        let filepath = PathAndFile {
            path: "the-path".to_string(),
            filename: "the-file.json".to_string(),
        };
        let res = filepath.check_filename();
        assert!(res.is_err());
        if let Err(msg) = res {
            assert_eq!(
                msg,
                "  Error: File 'the-path/the-file.json' does not exist".to_string()
            )
        }
    }

    fn mkfile(pathname: &str, filename: &str) -> (PathBuf, PathBuf) {
        // Create a temp directory
        let mut temp_dir = env::temp_dir();
        temp_dir.push(pathname);
        fs::create_dir_all(&temp_dir).unwrap();

        // Create the file inside the temp directory
        let file_path = temp_dir.join(&filename);
        let mut file = fs::File::create(&file_path).unwrap();
        writeln!(file, "test content").unwrap();

        (file_path, temp_dir)
    }

    #[test]
    fn test_path_is_not_directory() {
        // let's create a file now.
        let pathname = "the-path".to_string();
        let filename = "the-file".to_string();

        let (file_path, _) = mkfile(&pathname, &filename);

        // First, path is not a directory
        let filepath = PathAndFile {
            path: file_path.to_str().unwrap().to_string(),
            filename: filename.clone(),
        };

        let res = filepath.check_path();
        assert!(res.is_err());
    }
    #[test]
    fn test_assume_file_format() {
        // assume .yaml
        let filename = "aaa";
        assert_eq!(assume_file_format(filename), "aaa.yaml".to_string());
        // respects existing extension
        let filename = "aaa.yaml";
        assert_eq!(assume_file_format(filename), "aaa.yaml".to_string());
        // respects existing extension
        let filename = "aaa.json";
        assert_eq!(assume_file_format(filename), "aaa.json".to_string());
        // weird, but we'll allow ut
        let filename = "aaa.jsonb";
        assert_eq!(assume_file_format(filename), "aaa.jsonb.yaml".to_string())
    }
    #[test]
    fn test_success() {
        // let's create a file now.
        let pathname = "the-path".to_string();
        let filename = "the-file.yaml".to_string();

        let (file_path, temp_dir) = mkfile(&pathname, &filename);

        // Now test check_filename with the real file
        let filepath = PathAndFile {
            path: temp_dir.to_str().unwrap().to_string(),
            filename: filename.clone(),
        };
        let res = filepath.check_filename();
        assert!(res.is_ok());

        // Clean up
        fs::remove_file(file_path).unwrap();
        fs::remove_dir_all(temp_dir).unwrap();
    }
}
