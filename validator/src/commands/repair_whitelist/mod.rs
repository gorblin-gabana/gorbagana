use {
    crate::{
        admin_rpc_service,
        commands::{FromClapArgMatches, Result},
    },
    clap::{values_t, Arg, ArgMatches, Command, ArgAction},
    itertools::Itertools,
    solana_clap_utils::input_validators::is_pubkey,
    solana_cli_output::OutputFormat,
    solana_pubkey::Pubkey,
    std::path::Path,
};

pub const COMMAND: &str = "repair-whitelist";

#[derive(Debug, PartialEq)]
pub struct RepairWhitelistGetArgs {
    pub output: OutputFormat,
}

impl FromClapArgMatches for RepairWhitelistGetArgs {
    fn from_clap_arg_match(matches: &ArgMatches) -> Result<Self> {
        Ok(RepairWhitelistGetArgs {
            output: match matches.get_one::<String>("output") {
                Some(output) if output == "json" => OutputFormat::Json,
                Some(output) if output == "json-compact" => OutputFormat::JsonCompact,
                _ => OutputFormat::Display,
            },
        })
    }
}

#[derive(Debug, PartialEq)]
pub struct RepairWhitelistSetArgs {
    pub whitelist: Vec<Pubkey>,
}

impl FromClapArgMatches for RepairWhitelistSetArgs {
    fn from_clap_arg_match(matches: &ArgMatches) -> Result<Self> {
        let whitelist = values_t!(matches, "whitelist", Pubkey)?
            .into_iter()
            .unique()
            .collect::<Vec<_>>();
        Ok(RepairWhitelistSetArgs { whitelist })
    }
}

pub fn command() -> Command {
    Command::new(COMMAND)
        .about("Manage the validator's repair protocol whitelist")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("get")
                .about("Display the validator's repair protocol whitelist")
                .arg(
                    Arg::new("output")
                        .long("output")
                        .value_name("MODE")
                        .value_parser(["json", "json-compact"])
                        .help("Output display mode"),
                ),
        )
        .subcommand(
            Command::new("set")
                .about("Set the validator's repair protocol whitelist")
                .arg_required_else_help(true)
                .arg(
                    Arg::new("whitelist")
                        .long("whitelist")
                        .value_parser(clap::value_parser!(String))
                        .value_name("VALIDATOR IDENTITY")
                        .action(ArgAction::Append)
                        .required(true)
                        .help("Set the validator's repair protocol whitelist"),
                )
                .after_help(
                    "Note: repair protocol whitelist changes only apply to the currently running validator instance",
                ),
        )
        .subcommand(
            Command::new("remove-all")
                .about("Clear the validator's repair protocol whitelist")
                .after_help(
                    "Note: repair protocol whitelist changes only apply to the currently running validator instance",
                ),
        )
}

pub fn execute(matches: &ArgMatches, ledger_path: &Path) -> Result<()> {
    match matches.subcommand() {
        Some(("get", subcommand_matches)) => {
            let repair_whitelist_get_args =
                RepairWhitelistGetArgs::from_clap_arg_match(subcommand_matches)?;

            let admin_client = admin_rpc_service::connect(ledger_path);
            let repair_whitelist = admin_rpc_service::runtime()
                .block_on(async move { admin_client.await?.repair_whitelist().await })?;

            println!(
                "{}",
                repair_whitelist_get_args
                    .output
                    .formatted_string(&repair_whitelist)
            );
        }
        Some(("set", subcommand_matches)) => {
            let RepairWhitelistSetArgs { whitelist } =
                RepairWhitelistSetArgs::from_clap_arg_match(subcommand_matches)?;

            if whitelist.is_empty() {
                return Ok(());
            }

            set_repair_whitelist(ledger_path, whitelist)?;
        }
        Some(("remove-all", _)) => {
            set_repair_whitelist(ledger_path, Vec::default())?;
        }
        _ => unreachable!(),
    }

    Ok(())
}

fn set_repair_whitelist(ledger_path: &Path, whitelist: Vec<Pubkey>) -> Result<()> {
    let admin_client = admin_rpc_service::connect(ledger_path);
    admin_rpc_service::runtime()
        .block_on(async move { admin_client.await?.set_repair_whitelist(whitelist).await })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use {super::*, std::str::FromStr};

    #[test]
    fn verify_args_struct_by_command_repair_whitelist_get_default() {
        let app = command();
        let matches = app.get_matches_from(vec![COMMAND, "get"]);
        let subcommand_matches = matches.subcommand_matches("get").unwrap();
        let args = RepairWhitelistGetArgs::from_clap_arg_match(subcommand_matches).unwrap();
        assert_eq!(
            args,
            RepairWhitelistGetArgs {
                output: OutputFormat::Display
            }
        );
    }

    #[test]
    fn verify_args_struct_by_command_repair_whitelist_get_with_output() {
        let app = command();
        let matches = app.get_matches_from(vec![COMMAND, "get", "--output", "json"]);
        let subcommand_matches = matches.subcommand_matches("get").unwrap();
        let args = RepairWhitelistGetArgs::from_clap_arg_match(subcommand_matches).unwrap();
        assert_eq!(
            args,
            RepairWhitelistGetArgs {
                output: OutputFormat::Json
            }
        );
    }

    #[test]
    fn verify_args_struct_by_command_repair_whitelist_set_with_single_whitelist() {
        let app = command();
        let matches = app.get_matches_from(vec![
            COMMAND,
            "set",
            "--whitelist",
            "ch1do11111111111111111111111111111111111111",
        ]);
        let subcommand_matches = matches.subcommand_matches("set").unwrap();
        let args = RepairWhitelistSetArgs::from_clap_arg_match(subcommand_matches).unwrap();
        assert_eq!(
            args,
            RepairWhitelistSetArgs {
                whitelist: vec![
                    Pubkey::from_str("ch1do11111111111111111111111111111111111111").unwrap(),
                ]
            }
        );
    }

    #[test]
    fn verify_args_struct_by_command_repair_whitelist_set_with_multiple_whitelist() {
        let app = command();
        let matches = app.get_matches_from(vec![
            COMMAND,
            "set",
            "--whitelist",
            "ch1do11111111111111111111111111111111111111",
            "--whitelist",
            "ch1do11111111111111111111111111111111111112",
        ]);
        let subcommand_matches = matches.subcommand_matches("set").unwrap();
        let mut args = RepairWhitelistSetArgs::from_clap_arg_match(subcommand_matches).unwrap();
        args.whitelist.sort(); // the order of the whitelist is not guaranteed. sort it before asserting
        assert_eq!(
            args,
            RepairWhitelistSetArgs {
                whitelist: vec![
                    Pubkey::from_str("ch1do11111111111111111111111111111111111111").unwrap(),
                    Pubkey::from_str("ch1do11111111111111111111111111111111111112").unwrap(),
                ]
            }
        );
    }
}
