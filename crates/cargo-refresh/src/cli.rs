use std::{ffi::OsString, path::PathBuf};

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "cargo-refresh", version, about = "Fast Refresh for Rust")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// Cargo invokes external subcommands with an extra `refresh` argument.
    pub fn parse_cargo_args() -> Self {
        Self::parse_from(normalize_args(std::env::args_os()))
    }
}

pub fn normalize_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut args: Vec<_> = args.into_iter().collect();
    if args.get(1).is_some_and(|arg| arg == "refresh") {
        args.remove(1);
    }
    args
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run your program and refresh saved source changes.
    Run(RunArgs),
    /// Show controller state.
    Status,
    /// Stop the controller.
    Stop,
}

#[derive(Debug, Clone, Default, Args)]
pub struct RunArgs {
    #[arg(long)]
    pub bin: Option<String>,
    #[arg(short, long)]
    pub package: Option<String>,
    /// Additional editable crates; replaces metadata.refresh.edit when supplied.
    #[arg(long, value_delimiter = ',', value_parser = clap::builder::NonEmptyStringValueParser::new())]
    pub edit: Option<Vec<String>>,
    #[arg(long)]
    pub strict: bool,
    #[arg(long)]
    pub no_check: bool,
    #[arg(long, value_name = "PATH")]
    pub events: Option<PathBuf>,
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
    #[arg(last = true, allow_hyphen_values = true)]
    pub args: Vec<OsString>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_and_cargo_invocation_accept_the_same_flags() {
        for prefix in [vec!["cargo-refresh"], vec!["cargo-refresh", "refresh"]] {
            let args = prefix.into_iter().chain([
                "run",
                "--bin",
                "app",
                "--package",
                "demo",
                "--edit",
                "core,util",
                "--strict",
                "--no-check",
                "--events",
                "log.jsonl",
                "-vv",
                "--",
                "--port",
                "8080",
            ]);
            let cli = Cli::try_parse_from(normalize_args(args.map(OsString::from))).unwrap();
            let Command::Run(run) = cli.command else {
                panic!("expected run")
            };
            assert_eq!(run.bin.as_deref(), Some("app"));
            assert_eq!(run.package.as_deref(), Some("demo"));
            assert_eq!(run.edit.unwrap(), ["core", "util"]);
            assert!(run.strict && run.no_check);
            assert_eq!(run.events.unwrap(), PathBuf::from("log.jsonl"));
            assert_eq!(run.verbose, 2);
            assert_eq!(run.args, ["--port", "8080"]);
        }
    }

    #[test]
    fn empty_edit_names_are_rejected() {
        assert!(Cli::try_parse_from(["cargo-refresh", "run", "--edit", "core,,util"]).is_err());
    }
}
