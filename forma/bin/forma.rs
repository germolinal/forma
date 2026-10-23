use clap::{Parser, Subcommand};
#[cfg(feature = "cli")]
use colored::*;

#[cfg(feature = "cli")]
use forma::cli::{dev::DevArgs, serve::ServeArgs};

#[cfg(feature = "cli")]
use forma::cli::{
    dataset::{DatasetDownloadArgs, DatasetListArgs, DatasetUploadArgs},
    eval::EvalArgs,
    expand::ExpandArgs,
    init::InitArgs,
    login::LoginArgs,
    tester::TesterArgs,
};
#[cfg(not(feature = "cli"))]
use forma::prod::args::ProdServeArgs;

#[cfg(not(feature = "cli"))]
#[derive(Subcommand, Debug)]
enum Commands {
    /// Runs a production server
    Serve(ProdServeArgs),
}

#[cfg(feature = "cli")]
#[derive(Subcommand, Debug)]
enum Commands {
    /// Runs a development server with hot reloading, and a chat UI to talk
    /// to the agent.
    ///
    /// Unlike `serve`, this always exposes the agent over Server Sent Events,
    /// whatever `runtime.yaml` says, because that is what the bundled UI
    /// speaks. Use `serve` to exercise the runtime the agent will ship on.
    Dev(DevArgs),
    /// Runs a development server with hot reloading
    /// for the agent (not the runtime)
    Serve(ServeArgs),
    /// Uploads or appends elements to a dataset.
    ///
    /// A dataset is identified by the `name` field *inside* the JSON file, not
    /// by the file's name: `--action append` merges the examples into whatever
    /// dataset already carries that name, and `--action create` (the default)
    /// replaces it.
    ///
    /// Datasets are read from the store directly, so a file already sitting in
    /// `./evals/data` needs no upload. Use this to bring in a dataset from
    /// somewhere else on disk, or to push one to `--platform phoenix`.
    DatasetUpload(DatasetUploadArgs),
    /// Downloads a datasets and prints it to the stdout
    DatasetDownload(DatasetDownloadArgs),
    /// Lists the datasets found in the store (`./evals/data` by default)
    DatasetList(DatasetListArgs),
    /// Expands the agent into a single file.
    ///
    /// Prints the agent with every `$file` reference inlined and every omitted
    /// field filled in with its default. This is the only way to see what a
    /// definition actually became. A misspelled or unknown key anywhere in
    /// the agent file is rejected with an `unknown field` error that says
    /// where it is. Run this after editing the agent file.
    ///
    /// Add `--compile` to also build the agent, which additionally checks the
    /// tools and the LLM clients. Neither form reads the metric templates
    /// referenced by evaluations; `eval` does that when it starts.
    Expand(ExpandArgs),
    /// Runs evaluations.
    ///
    /// With no filter, everything in the agent that carries an `evals` block is
    /// evaluated. Passing any of `--agent`, `--workflow`, `--node` or `--llm`
    /// narrows the run to what was named, and naming something that has no
    /// evaluations of its own is an error. `--dataset` narrows further, to the
    /// evals that read the named dataset; naming a dataset the agent does not
    /// use is likewise an error. Use `--list` to see the ids available, and
    /// `--dry-run` to see what a set of filters would select.
    ///
    /// Results are written to `./evals/results/<date>_<experiment-name>/` as
    /// they are produced: a `REPORT.md` holding every answer and judgement
    /// worst-first, and one `experiment.json` per dataset. A row is also added
    /// to `./evals/results/README.md`, which is the table to read when deciding
    /// whether a change helped.
    ///
    /// Both files open with a `Scores` table — one row per evaluated element,
    /// one column per metric — followed by a `Breakdown` that splits the same
    /// numbers one dataset at a time. A score in `Scores` is the mean over
    /// every dataset the metric was used on, so read the two together: a
    /// regression on one dataset can be cancelled there by a gain on another.
    /// `Runs` counts the answers that were actually measured; a `⚠️` marks a
    /// metric the judge could not always produce, and a `❌` one that fell
    /// below the `required_score` it declared.
    ///
    /// A metric that declares a `required_score` fails the run — and the
    /// process — when its mean falls below it on any dataset, or when nothing
    /// could be scored at all. Pass `--no-fail` to report the breaches without
    /// ending in an error, which is what you want while iterating and not what
    /// you want in CI.
    Eval(EvalArgs),
    /// Initializes a new agent project.
    ///
    /// If path is '.', the project will be initialized in the current directory.
    /// Otherwise, a new directory will be created at the specified path, and
    /// the project will be initialized there.
    Init(InitArgs),
    /// Asks the 'personas' (aka, virtual testers) to create a dataset of questions.
    ///
    /// Each of the `-n` questions is fed the questions generated so far and
    /// told to generate something different, which helps but does not fix
    /// clustering: a persona described in general terms can still repeat
    /// itself, occasionally verbatim, especially on a small local model. Give
    /// it a concrete list to choose from and a temperature above zero, and
    /// still expect to review the result by hand. The fed-back list also
    /// occasionally leaks its own numbering (`"8. I'm planning..."`) into a
    /// generated question; strip a leading `^\d+\.\s*` if you see it.
    ///
    /// `-a append` adds the new questions to `<output-path>/<persona>.json`
    /// instead of replacing it, so hand-written examples survive. The default
    /// is still `create`, which overwrites the file outright. Personas never
    /// produce reference answers; a dataset with an `expected_output` per
    /// example is written by hand.
    Tester(TesterArgs),
    /// Authenticate MCP servers via browser-based OAuth2 and persist the tokens.
    Login(LoginArgs),
}

/// Forma
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// What to do
    #[command(subcommand)]
    command: Commands,
}

#[cfg(not(feature = "cli"))]
fn main() {
    forma::install_crypto_provider();
    let args = Args::parse();
    let result = forma::runtime().block_on(async {
        match args.command {
            #[cfg(not(feature = "multi-tenant"))]
            Commands::Serve(s) => s.run().await.map_err(|e| format!("{e}")),
            #[cfg(feature = "multi-tenant")]
            Commands::Serve(s) => s.run_multi_tenant().await.map_err(|e| format!("{e}")),
        }
    });
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

#[cfg(feature = "cli")]
fn main() {
    forma::install_crypto_provider();
    let args = Args::parse();

    // Read .env file if the command requires it
    let no_credentials = matches!(
        args.command,
        Commands::Init(_)
            | Commands::Expand(_)
            | Commands::DatasetUpload(_)
            | Commands::DatasetList(_)
    );
    let envs = dotenvy::dotenv();
    if envs.is_ok() && !no_credentials {
        eprintln!("loaded .env file");
    } else if !no_credentials && envs.is_err() {
        eprintln!("could not read .env file");
    }

    let ret = forma::runtime().block_on(async {
        match args.command {
            // Need credentials for these commands
            Commands::DatasetUpload(args) => args.upload().await,
            Commands::DatasetList(args) => args.list().await,
            Commands::DatasetDownload(args) => args.download().await,
            Commands::Dev(args) => args.dev().await.map_err(|e| format!("{e}")),
            Commands::Serve(args) => args.serve().await.map_err(|e| format!("{e}")),
            Commands::Eval(args) => args.eval().await.map_err(|e| format!("{e}")),
            Commands::Tester(args) => args.test().await.map_err(|e| format!("{e}")),
            Commands::Expand(args) => args.expand().await.map_err(|e| format!("{e}")),
            Commands::Init(args) => args.init(),
            Commands::Login(args) => args.login().await.map_err(|e| format!("{e}")),
        }
    });

    if let Err(e) = ret {
        let e = e.red();
        println!("{e}");
        std::process::exit(1)
    }
}
