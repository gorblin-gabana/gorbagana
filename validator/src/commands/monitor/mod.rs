use {
    crate::{commands::Result, dashboard::Dashboard},
    clap::{ArgMatches, Command},
    std::{path::Path, time::Duration},
};

pub fn command<'a>() -> Command {
    Command::new("monitor").about("Monitor the validator")
}

pub fn execute(_matches: &ArgMatches, ledger_path: &Path) -> Result<()> {
    monitor_validator(ledger_path)
}

pub fn monitor_validator(ledger_path: &Path) -> Result<()> {
    let dashboard = Dashboard::new(ledger_path, None, None);
    dashboard.run(Duration::from_secs(2));

    Ok(())
}
