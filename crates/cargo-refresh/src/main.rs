use anyhow::Result;
use cargo_refresh::{
    cli::{Cli, Command},
    config::Config,
    install, session,
};

fn main() -> Result<()> {
    match Cli::parse_cargo_args().command {
        Command::Run(args) => {
            let mut config = Config::load(args)?;
            install::prepare(&mut config);
            if config.verbose > 0 {
                println!("{}", serde_json::to_string_pretty(&config)?);
            }
            session::run(config)?;
        }
        Command::Status | Command::Stop => println!("not implemented yet"),
    }
    Ok(())
}
