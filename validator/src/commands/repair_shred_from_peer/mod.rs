use {
    crate::{
        admin_rpc_service,
        commands::{FromClapArgMatches, Result},
    },
    clap::{Arg, ArgMatches, Command},
    solana_clap_utils::input_validators::{is_parsable, is_pubkey},
    solana_pubkey::Pubkey,
    std::path::Path,
};

const COMMAND: &str = "repair-shred-from-peer";

#[derive(Debug, PartialEq)]
pub struct RepairShredFromPeerArgs {
    pub pubkey: Option<Pubkey>,
    pub slot: u64,
    pub shred: u64,
}

impl FromClapArgMatches for RepairShredFromPeerArgs {
    fn from_clap_arg_match(matches: &ArgMatches) -> Result<Self> {
        Ok(RepairShredFromPeerArgs {
            pubkey: matches.get_one::<String>("pubkey").and_then(|s| s.parse().ok()),
            slot: matches
                .get_one::<String>("slot")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or_else(|| {
                    eprintln!("slot is required");
                    std::process::exit(1);
                }),
            shred: matches
                .get_one::<String>("shred")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or_else(|| {
                    eprintln!("shred is required");
                    std::process::exit(1);
                }),
        })
    }
}

pub fn command<'a>() -> Command {
    Command::new(COMMAND)
        .about("Request a repair from the specified validator")
        .arg(
            Arg::new("pubkey")
                .long("pubkey")
                .value_name("PUBKEY")
                .required(false)
                
                .value_parser(clap::value_parser!(String))
                .help("Identity pubkey of the validator to repair from"),
        )
        .arg(
            Arg::new("slot")
                .long("slot")
                .value_name("SLOT")
                .required(true)
                
                .value_parser(clap::value_parser!(String))
                .help("Slot to repair"),
        )
        .arg(
            Arg::new("shred")
                .long("shred")
                .value_name("SHRED")
                .required(true)
                
                .value_parser(clap::value_parser!(String))
                .help("Shred to repair"),
        )
}

pub fn execute(matches: &ArgMatches, ledger_path: &Path) -> Result<()> {
    let RepairShredFromPeerArgs {
        pubkey,
        slot,
        shred,
    } = RepairShredFromPeerArgs::from_clap_arg_match(matches)?;

    let admin_client = admin_rpc_service::connect(ledger_path);
    admin_rpc_service::runtime().block_on(async move {
        admin_client
            .await?
            .repair_shred_from_peer(pubkey, slot, shred)
            .await
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::commands::tests::{
            verify_args_struct_by_command, verify_args_struct_by_command_is_error,
        },
        std::str::FromStr,
    };

    #[test]
    fn verify_args_struct_by_command_repair_shred_from_peer_missing_slot_and_shred() {
        verify_args_struct_by_command_is_error::<RepairShredFromPeerArgs>(command(), vec![COMMAND]);
        verify_args_struct_by_command_is_error::<RepairShredFromPeerArgs>(
            command(),
            vec![COMMAND, "--slot", "1"],
        );
        verify_args_struct_by_command_is_error::<RepairShredFromPeerArgs>(
            command(),
            vec![COMMAND, "--shred", "2"],
        );
    }

    #[test]
    fn verify_args_struct_by_command_repair_shred_from_peer_missing_pubkey() {
        verify_args_struct_by_command(
            command(),
            vec![COMMAND, "--slot", "1", "--shred", "2"],
            RepairShredFromPeerArgs {
                pubkey: None,
                slot: 1,
                shred: 2,
            },
        );
    }

    #[test]
    fn verify_args_struct_by_command_repair_shred_from_peer_with_pubkey() {
        verify_args_struct_by_command(
            command(),
            vec![
                COMMAND,
                "--slot",
                "1",
                "--shred",
                "2",
                "--pubkey",
                "ch1do11111111111111111111111111111111111111",
            ],
            RepairShredFromPeerArgs {
                pubkey: Some(
                    Pubkey::from_str("ch1do11111111111111111111111111111111111111").unwrap(),
                ),
                slot: 1,
                shred: 2,
            },
        );
    }
}
