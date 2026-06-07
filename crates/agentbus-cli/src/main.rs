mod commands;

use clap::Parser;

use agentbus_core::store::StoreError;

fn main() {
    let cli = commands::Cli::parse();
    if let Err(e) = commands::run(cli) {
        match e.downcast_ref::<StoreError>() {
            Some(se) => eprintln!("error[{}]: {se}", se.code()),
            None => eprintln!("error: {e}"),
        }
        std::process::exit(1);
    }
}
