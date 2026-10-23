use clap::ValueEnum;

/// Where datasets and evaluation results are kept
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum EvalPlatformKind {
    /// Plain files, within the directory given by `FORMA_EVAL_DIR`
    /// (which defaults to `./evals`)
    #[default]
    File,
    /// An Arize Phoenix server, at `PHOENIX_EVAL_ENDPOINT`
    /// (which defaults to `http://localhost:6006`)
    Phoenix,
}
