use anyhow::Result;
use clap::Parser;

mod cli;

#[tokio::main]
async fn main() -> Result<()> {
    cli::commands::run(cli::args::Cli::parse()).await
}
