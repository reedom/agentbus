mod commands;
use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = commands::Cli::parse();
    commands::run(cli).await
}
