use anyhow::Result;
use cargo_refresh::{
    cli::{Cli, Command},
    config::Config,
    events::EventLog,
};

fn main() -> Result<()> {
    match Cli::parse_cargo_args().command {
        Command::Run(args) => {
            let config = Config::load(args)?;
            println!("{}", serde_json::to_string_pretty(&config)?);
            let mut events = EventLog::open(&config.events)?;
            events.event("start", 0)?;
        }
        Command::Status | Command::Stop => println!("not implemented yet"),
    }
    Ok(())
}
