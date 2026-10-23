use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

use crate::dataset::{BrokenDataset, Dataset, DatasetSummary};

/// Turns arbitrary text into something safe to use as a file or
/// directory name (e.g., `[node thinking] My eval!` -> `node-thinking-my-eval`)
pub(crate) fn slug(text: &str) -> String {
    let mut ret = String::with_capacity(text.len());
    let mut last_was_dash = true; // avoids a leading dash
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            ret.push(c.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            ret.push('-');
            last_was_dash = true;
        }
    }
    let ret = ret.trim_end_matches('-');
    // Keep names short enough to be comfortable in a terminal
    let ret: String = ret.chars().take(60).collect();
    let ret = ret.trim_end_matches('-').to_string();
    if ret.is_empty() {
        "unnamed".to_string()
    } else {
        ret
    }
}

/// A dataset as found on disk
pub(crate) struct StoredDataset {
    pub path: PathBuf,
    pub dataset: Dataset,
    pub created_at: String,
    pub updated_at: String,
}

impl StoredDataset {
    fn read(path: PathBuf) -> Result<Self, String> {
        let contents = std::fs::read_to_string(&path)
            .map_err(|e| format!("could not read dataset '{}': {e}", path.display()))?;
        let mut dataset: Dataset = serde_json::from_str(&contents)
            .map_err(|e| format!("could not parse dataset '{}': {e}", path.display()))?;

        // The name within the file wins, but a dataset that does not declare
        // one is still perfectly usable: it is called after its file.
        if dataset.name.is_empty() {
            dataset.name = file_stem(&path);
        }
        let (created_at, updated_at) = timestamps(&path);
        Ok(Self {
            path,
            dataset,
            created_at,
            updated_at,
        })
    }

    pub fn summary(&self) -> DatasetSummary {
        let metadata = if self.dataset.metadata.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::Value::Array(self.dataset.metadata.clone())
        };
        DatasetSummary {
            id: file_stem(&self.path),
            name: self.dataset.name.clone(),
            description: self.dataset.description.clone().unwrap_or_default(),
            metadata,
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            example_count: self.dataset.data.len(),
        }
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Reads the creation and modification dates of a file, as RFC3339 strings.
///
/// Not every platform records a creation date, so that one falls back to
/// the modification date.
fn timestamps(path: &Path) -> (String, String) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (String::new(), String::new());
    };
    let format = |t: std::io::Result<std::time::SystemTime>| {
        t.map(|t| DateTime::<Utc>::from(t).to_rfc3339())
            .unwrap_or_default()
    };
    let modified = format(meta.modified());
    let created = match meta.created() {
        Ok(t) => DateTime::<Utc>::from(t).to_rfc3339(),
        Err(_) => modified.clone(),
    };
    (created, modified)
}

/// What a scan of the store found
#[derive(Default)]
pub(crate) struct Listing {
    /// The datasets that can be used
    pub datasets: Vec<StoredDataset>,
    /// The files that are meant to be datasets, but cannot be read
    pub broken: Vec<BrokenDataset>,
}

/// The datasets kept in a directory, one JSON file each.
///
/// This is the very same format `forma tester` writes and `forma
/// dataset-upload` reads, so a project's `evals/data` directory is
/// already a valid store.
pub(crate) struct DatasetStore {
    dir: PathBuf,
}

impl DatasetStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Every dataset in the store, and every file that was meant to be one
    /// but could not be read.
    ///
    /// A broken file does not make the whole store unreadable—the rest of
    /// the datasets are still returned—but it is not swallowed either: it
    /// comes back in `broken`, for the caller to report.
    pub fn list(&self) -> Result<Listing, String> {
        let mut ret = Listing::default();
        if !self.dir.is_dir() {
            return Ok(ret);
        }
        let entries = std::fs::read_dir(&self.dir)
            .map_err(|e| format!("could not read '{}': {e}", self.dir.display()))?;

        for entry in entries {
            let path = match entry {
                Ok(e) => e.path(),
                Err(e) => {
                    // Nothing was read, so there is no dataset to name
                    ret.broken.push(BrokenDataset {
                        id: String::new(),
                        error: format!("could not read a file in '{}': {e}", self.dir.display()),
                    });
                    continue;
                }
            };
            if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let id = file_stem(&path);
            match StoredDataset::read(path) {
                Ok(d) => ret.datasets.push(d),
                Err(error) => ret.broken.push(BrokenDataset { id, error }),
            }
        }
        ret.datasets
            .sort_by(|a, b| a.dataset.name.cmp(&b.dataset.name));
        ret.broken.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(ret)
    }

    /// Finds a dataset by name, looking for a file called after it before
    /// falling back to scanning the store for a matching `name` field.
    ///
    /// A file that is there but cannot be read is reported as such: saying
    /// that the dataset is not there would hide the actual problem.
    pub fn find(&self, name: &str) -> Result<StoredDataset, String> {
        let direct = self.dir.join(format!("{name}.json"));
        if direct.is_file() {
            match StoredDataset::read(direct) {
                Ok(found) if found.dataset.name == name => return Ok(found),
                // The file is called after another dataset... keep looking
                Ok(_) => {}
                Err(e) => return Err(e),
            }
        }
        let listing = self.list()?;
        if let Some(found) = listing
            .datasets
            .into_iter()
            .find(|d| d.dataset.name == name)
        {
            return Ok(found);
        }
        // The dataset may well be one of the files that could not be read
        match listing.broken.into_iter().find(|b| b.id == name) {
            Some(broken) => Err(broken.error),
            None => Err(format!("dataset '{name}' not found")),
        }
    }

    /// Writes a dataset to the store, returning the path it was written to
    pub fn write(&self, dataset: &Dataset) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("could not create '{}': {e}", self.dir.display()))?;

        // If the dataset is already in the store, keep it where it is; this
        // way uploading a file twice does not leave two copies behind.
        let path = match self.find(&dataset.name) {
            Ok(found) => found.path,
            Err(_) => self.dir.join(format!("{}.json", slug(&dataset.name))),
        };
        let contents = serde_json::to_string_pretty(dataset)
            .map_err(|e| format!("could not serialize dataset '{}': {e}", dataset.name))?;
        super::write_file(&path, &contents)?;
        Ok(path)
    }
}

/// Gives every example an identifier, so that answers and judgements can be
/// traced back to the question that produced them.
///
/// Identifiers already present in the file are respected; the rest are named
/// after their position, which keeps them stable across runs.
pub(crate) fn assign_ids(dataset: &mut Dataset) {
    for (i, point) in dataset.data.iter_mut().enumerate() {
        let missing = match &point.id {
            Some(id) => id.is_empty(),
            None => true,
        };
        if missing {
            point.id = Some(format!("ex-{:04}", i + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::DataPoint;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forma-store-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("could not create temp dir");
        dir
    }

    fn dataset(name: &str, n: usize) -> Dataset {
        Dataset {
            name: name.to_string(),
            description: Some(format!("dataset {name}")),
            data: (0..n)
                .map(|i| DataPoint {
                    id: None,
                    input: format!("question {i}").into(),
                    expected_output: None,
                })
                .collect(),
            metadata: vec![],
        }
    }

    #[test]
    fn slugs_are_safe_file_names() {
        assert_eq!(&slug("[node thinking] My eval!"), "node-thinking-my-eval");
        assert_eq!(&slug("already-fine"), "already-fine");
        assert_eq!(&slug("  spaced  out  "), "spaced-out");
        assert_eq!(&slug("../../etc/passwd"), "etc-passwd");
        assert_eq!(&slug("!!!"), "unnamed");
        assert_eq!(&slug(""), "unnamed");
        assert_eq!(&slug("MiXeD CaSe"), "mixed-case");
        assert!(!slug(&"very long name ".repeat(20)).ends_with('-'));
        assert!(slug(&"a".repeat(100)).len() <= 60);
    }

    #[test]
    fn assigns_ids_only_where_missing() {
        let mut d = dataset("wanderer", 3);
        d.data[1].id = Some("my-own-id".to_string());
        assign_ids(&mut d);
        assert_eq!(d.data[0].id.as_deref(), Some("ex-0001"));
        assert_eq!(d.data[1].id.as_deref(), Some("my-own-id"));
        assert_eq!(d.data[2].id.as_deref(), Some("ex-0003"));
    }

    #[test]
    fn assigned_ids_are_stable() {
        let mut first = dataset("wanderer", 5);
        let mut second = dataset("wanderer", 5);
        assign_ids(&mut first);
        assign_ids(&mut second);
        let ids =
            |d: &Dataset| -> Vec<String> { d.data.iter().map(|p| p.id.clone().unwrap()).collect() };
        assert_eq!(ids(&first), ids(&second));
    }

    #[test]
    fn empty_store_lists_nothing() {
        let store = DatasetStore::new(temp_dir("empty").join("data"));
        let listing = store.list().expect("could not list");
        assert!(listing.datasets.is_empty());
        assert!(listing.broken.is_empty());
        assert!(store.find("wanderer").is_err());
    }

    #[test]
    fn writes_reads_and_lists_datasets() {
        let store = DatasetStore::new(temp_dir("roundtrip"));
        let d = dataset("wanderer", 2);
        let path = store.write(&d).expect("could not write");
        assert_eq!(path.file_name().unwrap(), "wanderer.json");

        let found = store.find("wanderer").expect("could not find");
        assert_eq!(found.dataset.data.len(), 2);
        assert_eq!(
            found.dataset.description.as_deref(),
            Some("dataset wanderer")
        );

        let summary = found.summary();
        assert_eq!(&summary.name, "wanderer");
        assert_eq!(summary.example_count, 2);
        assert!(!summary.created_at.is_empty());

        store.write(&dataset("other", 1)).expect("could not write");
        let list = store.list().expect("could not list").datasets;
        assert_eq!(list.len(), 2);
        // ... sorted by name
        assert_eq!(&list[0].dataset.name, "other");
        assert_eq!(&list[1].dataset.name, "wanderer");
    }

    #[test]
    fn writing_twice_does_not_duplicate() {
        let store = DatasetStore::new(temp_dir("twice"));
        store.write(&dataset("wanderer", 2)).expect("first write");
        store.write(&dataset("wanderer", 7)).expect("second write");
        let list = store.list().expect("could not list").datasets;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].dataset.data.len(), 7);
    }

    #[test]
    fn names_that_are_not_file_names_are_still_found() {
        let store = DatasetStore::new(temp_dir("odd-names"));
        let path = store
            .write(&dataset("Wanderer, the 2nd!", 1))
            .expect("could not write");
        assert_eq!(path.file_name().unwrap(), "wanderer-the-2nd.json");
        let found = store.find("Wanderer, the 2nd!").expect("could not find");
        assert_eq!(&found.dataset.name, "Wanderer, the 2nd!");
    }

    #[test]
    fn datasets_are_named_after_their_file_when_anonymous() {
        let dir = temp_dir("anonymous");
        let store = DatasetStore::new(dir.clone());
        std::fs::write(
            dir.join("nameless.json"),
            r#"{"name": "", "description": null, "data": []}"#,
        )
        .expect("could not write");
        let found = store.find("nameless").expect("could not find");
        assert_eq!(&found.dataset.name, "nameless");
    }

    #[test]
    fn broken_files_are_reported_not_fatal() {
        let dir = temp_dir("broken");
        let store = DatasetStore::new(dir.clone());
        store.write(&dataset("good", 1)).expect("could not write");
        std::fs::write(dir.join("broken.json"), "{ not json").expect("could not write");
        std::fs::write(dir.join("notes.txt"), "ignore me").expect("could not write");

        let listing = store.list().expect("could not list");
        assert_eq!(listing.datasets.len(), 1);
        assert_eq!(&listing.datasets[0].dataset.name, "good");
        // ... and the broken one is not swallowed
        assert_eq!(listing.broken.len(), 1);
        assert_eq!(&listing.broken[0].id, "broken");
        assert!(
            listing.broken[0].error.contains("broken.json"),
            "{}",
            listing.broken[0].error
        );
    }

    #[test]
    fn a_broken_dataset_says_why_instead_of_not_found() {
        let dir = temp_dir("broken-find");
        let store = DatasetStore::new(dir.clone());
        // A dataset written before `output` was renamed `expected_output`
        std::fs::write(
            dir.join("wanderer.json"),
            r#"{"name": "wanderer", "data": [{"input": "hi", "output": "hello"}]}"#,
        )
        .expect("could not write");

        let Err(err) = store.find("wanderer") else {
            panic!("a dataset that does not parse should not be found");
        };
        assert!(err.contains("wanderer.json"), "{err}");
        assert!(err.contains("output"), "{err}");

        // ... and a dataset that is really not there still says so
        let Err(err) = store.find("nowhere") else {
            panic!("there is no such dataset");
        };
        assert_eq!(&err, "dataset 'nowhere' not found");
    }
}
