use clap::Parser;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Arguments for starting a new project
#[derive(Debug, Parser)]
pub struct InitArgs {
    /// The path where the agent and its dependencies will be stored
    #[arg(short, long)]
    pub path: String,
}

impl InitArgs {
    fn resolve_path(&self) -> Result<PathBuf, String> {
        if self.path.is_empty() {
            return Err("please provide a path using the argument".to_string());
        }

        let input_path = Path::new(&self.path);

        if input_path.is_absolute() {
            // If user provided "/etc/config", return it exactly as is
            Ok(input_path.to_path_buf())
        } else {
            // If user provided "test-agent" or "./test-agent",
            // join it with current_dir.
            let current_dir = std::env::current_dir()
                .map_err(|e| format!("could not get current directory: {e}"))?;

            Ok(current_dir.join(input_path))
        }
    }

    fn validate_path(&self, path: &Path) -> Result<(), String> {
        if path.exists() {
            if !path.is_dir() {
                return Err(format!(
                    "the path '{}' exists but it is not a directory",
                    path.display()
                ));
            }

            let is_empty = path
                .read_dir()
                .map_err(|e| {
                    format!(
                        "could not read the contents of the directory '{}': {e}",
                        path.display()
                    )
                })?
                .next()
                .is_none();
            if !is_empty {
                return Err(format!(
                    "the path '{}' exists, but is not empty.",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    fn mkdir(&self, path: &Path, dir_name: &str) -> Result<PathBuf, String> {
        let dir_path = path.join(dir_name);
        std::fs::create_dir_all(&dir_path)
            .map_err(|e| format!("could not create directory '{}': {e}", dir_path.display()))?;
        Ok(dir_path)
    }

    fn cp(&self, path: &Path, file_name: &str, content: &str) -> Result<(), String> {
        let file_path = path.join(file_name);
        let mut file = std::fs::File::create(&file_path)
            .map_err(|e| format!("could not create file '{}': {e}", file_path.display()))?;
        file.write_all(content.as_bytes())
            .map_err(|e| format!("could not write to file '{}': {e}", file_path.display()))?;
        Ok(())
    }

    /// Initialize a new project
    pub fn init(&self) -> Result<(), String> {
        let path = self.resolve_path()?;
        self.validate_path(&path)?;
        // crate root directory, recursively
        std::fs::create_dir_all(&path)
            .map_err(|e| format!("could not create directory '{}': {e}", path.display()))?;

        self.cp(&path, ".env", include_str!("./init_files/env.txt"))?;
        self.cp(&path, "README.md", include_str!("./init_files/README.md"))?;
        self.cp(
            &path,
            ".gitignore",
            include_str!("./init_files/gitignore.txt"),
        )?;

        // Runtime
        self.cp(
            &path,
            "runtime.yaml",
            include_str!("./init_files/runtime.yaml"),
        )?;

        // Container image for production
        self.cp(&path, "Dockerfile", include_str!("./init_files/Dockerfile"))?;
        self.cp(
            &path,
            ".dockerignore",
            include_str!("./init_files/dockerignore.txt"),
        )?;

        // agent-dev-guidelines
        self.cp(
            &path,
            "agent-dev-guidelines.md",
            include_str!("./init_files/agent-dev-guidelines.md"),
        )?;

        // Dev stuff
        let dev = self.mkdir(&path, "dev")?;
        self.cp(
            &dev,
            "docker-compose.yaml",
            include_str!("./init_files/docker-compose.yaml"),
        )?;

        // A metric
        let evals = self.mkdir(&path, "evals")?;
        let metrics = self.mkdir(&evals, "metrics")?;
        self.cp(
            &metrics,
            "politeness.md",
            include_str!("./init_files/politeness.md"),
        )?;
        let personas = self.mkdir(&evals, "personas")?;
        self.cp(
            &personas,
            "wanderer.yaml",
            include_str!("./init_files/wanderer.yaml"),
        )?;
        let data = self.mkdir(&evals, "data")?;
        self.cp(
            &data,
            "wanderer.json",
            include_str!("./init_files/wanderer.json"),
        )?;

        // The actual agent
        let src = self.mkdir(&path, "src")?;
        self.cp(&src, "agent.yaml", include_str!("./init_files/agent.yaml"))?;
        // prompts
        let prompts = self.mkdir(&src, "prompts")?;
        self.cp(
            &prompts,
            "agent_prompt.md",
            include_str!("./init_files/agent_prompt.md"),
        )?;
        Ok(())
    }
}
