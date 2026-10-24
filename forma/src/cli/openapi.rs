use clap::Parser;
use protocols::FormaError;

use crate::server::openapi::{APIS, spec};

/// Arguments for describing one of Forma's HTTP APIs.
#[derive(Debug, Parser)]
pub struct OpenApiArgs {
    /// The API: the runtime's, for one client (`sync`, `sse`, `ai-sdk-v5`,
    /// `whatsapp`, `telegram`), or the manager's (`manager`)
    #[arg(value_parser = clap::builder::PossibleValuesParser::new(APIS))]
    pub api: String,

    /// The file to write the description to. Printed when omitted.
    #[arg(short, long)]
    pub output: Option<String>,
}

impl OpenApiArgs {
    /// Writes, or prints, the OpenAPI description of the API.
    pub fn describe(&self) -> Result<(), FormaError> {
        let json = serde_json::to_string_pretty(&spec(&self.api)?)?;
        match &self.output {
            Some(path) => std::fs::write(path, json + "\n").map_err(|e| {
                FormaError::Input(format!("could not write the description to '{path}': {e}"))
            })?,
            None => println!("{json}"),
        }
        Ok(())
    }
}
