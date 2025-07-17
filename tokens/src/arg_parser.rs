use {
    crate::args::{
        Args, BalancesArgs, Command, DistributeTokensArgs, SenderStakeArgs, SplTokenArgs,
        StakeArgs, TransactionLogArgs,
    },
    clap::{Arg, ArgMatches, Command as ClapCommand},
    solana_clap_utils::{
        input_parsers::{pubkey_of_signer},
        input_validators::{is_url_or_moniker},
        keypair::{signer_from_path},
    },
    solana_cli_config::CONFIG_FILE,
    solana_remote_wallet::remote_wallet::maybe_wallet_manager,
    solana_sdk::native_token::sol_to_lamports,
    std::{error::Error, ffi::OsString, process::exit},
};

fn get_matches<I, T>(args: I) -> ArgMatches
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let default_config_file = CONFIG_FILE.as_ref().unwrap();
    ClapCommand::new("solana-tokens")
        .about("Solana tokens")
        .version("2.0.0")
        .arg(
            Arg::new("config_file")
                .short('C')
                .long("config")
                .value_name("FILEPATH")
                .default_value(default_config_file.as_str())
                .help("Config file"),
        )
        .arg(
            Arg::new("json_rpc_url")
                .short('u')
                .long("url")
                .value_name("URL_OR_MONIKER")
                .global(true)
                .value_parser(|s: &str| is_url_or_moniker(s))
                .help(
                    "URL for Solana's JSON RPC or moniker (or their first letter): \
                       [mainnet-beta, testnet, devnet, localhost]",
                ),
        )
        .subcommand(
            ClapCommand::new("distribute-tokens")
                .about("Distribute SOL")
                .arg(
                    Arg::new("db_path")
                        .long("db-path")
                        .required(true)
                        .value_name("FILE")
                        .help(
                            "Path to file containing a list of recipients",
                        ),
                )
                .arg(
                    Arg::new("input_csv")
                        .long("input-csv")
                        .required(true)
                        .value_name("FILE")
                        .help("Input CSV file"),
                )
                .arg(
                    Arg::new("transfer_amount")
                        .long("transfer-amount")
                        .value_name("AMOUNT")
                        .help("The amount to transfer to each recipient, in SOL; accepts keyword ALL"),
                )
                .arg(
                    Arg::new("dry_run")
                        .long("dry-run")
                        .action(clap::ArgAction::SetTrue)
                        .help("Do not execute any transfers"),
                )
                .arg(
                    Arg::new("output_path")
                        .long("output-path")
                        .value_name("FILE")
                        .help("Write the transaction log to this file"),
                )
                .arg(
                    Arg::new("sender_keypair")
                        .long("from")
                        .value_name("KEYPAIR")
                        .help("Sender keypair; reads the default wallet if not specified"),
                )
                .arg(
                    Arg::new("fee_payer")
                        .long("fee-payer")
                        .value_name("KEYPAIR")
                        .help("Fee payer keypair"),
                ),
        )
        .subcommand(
            ClapCommand::new("create-stake")
                .about("Create stake accounts")
                .arg(
                    Arg::new("db_path")
                        .long("db-path")
                        .required(true)
                        .value_name("FILE")
                        .help(
                            "Path to file containing a list of recipients",
                        ),
                )
                .arg(
                    Arg::new("input_csv")
                        .long("input-csv")
                        .required(true)
                        .value_name("FILE")
                        .help("Input CSV file"),
                )
                .arg(
                    Arg::new("dry_run")
                        .long("dry-run")
                        .action(clap::ArgAction::SetTrue)
                        .help("Do not execute any transfers"),
                )
                .arg(
                    Arg::new("output_path")
                        .long("output-path")
                        .value_name("FILE")
                        .help("Write the transaction log to this file"),
                )
                .arg(
                    Arg::new("sender_keypair")
                        .long("from")
                        .value_name("KEYPAIR")
                        .help("Sender keypair; reads the default wallet if not specified"),
                )
                .arg(
                    Arg::new("unlocked_sol")
                        .long("unlocked-sol")
                        .value_name("AMOUNT")
                        .help("Amount of SOL to leave unlocked in new stake accounts, in SOL"),
                )
                .arg(
                    Arg::new("lockup_authority")
                        .long("lockup-authority")
                        .value_name("KEYPAIR")
                        .help("Lockup authority keypair"),
                )
                .arg(
                    Arg::new("fee_payer")
                        .long("fee-payer")
                        .value_name("KEYPAIR")
                        .help("Fee payer keypair"),
                ),
        )
        .subcommand(
            ClapCommand::new("distribute-stake")
                .about("Distribute stake accounts to recipients")
                .arg(
                    Arg::new("db_path")
                        .long("db-path")
                        .required(true)
                        .value_name("FILE")
                        .help(
                            "Path to file containing a list of recipients",
                        ),
                )
                .arg(
                    Arg::new("input_csv")
                        .long("input-csv")
                        .required(true)
                        .value_name("FILE")
                        .help("Input CSV file"),
                )
                .arg(
                    Arg::new("dry_run")
                        .long("dry-run")
                        .action(clap::ArgAction::SetTrue)
                        .help("Do not execute any transfers"),
                )
                .arg(
                    Arg::new("output_path")
                        .long("output-path")
                        .value_name("FILE")
                        .help("Write the transaction log to this file"),
                )
                .arg(
                    Arg::new("sender_keypair")
                        .long("from")
                        .value_name("KEYPAIR")
                        .help("Sender keypair; reads the default wallet if not specified"),
                )
                .arg(
                    Arg::new("stake_account_address")
                        .long("stake-account-address")
                        .value_name("ADDRESS")
                        .help("Stake account address"),
                )
                .arg(
                    Arg::new("unlocked_sol")
                        .long("unlocked-sol")
                        .value_name("AMOUNT")
                        .help("Amount of SOL to leave unlocked in new stake accounts, in SOL"),
                )
                .arg(
                    Arg::new("stake_authority")
                        .long("stake-authority")
                        .value_name("KEYPAIR")
                        .help("Stake authority keypair"),
                )
                .arg(
                    Arg::new("withdraw_authority")
                        .long("withdraw-authority")
                        .value_name("KEYPAIR")
                        .help("Withdraw authority keypair"),
                )
                .arg(
                    Arg::new("lockup_authority")
                        .long("lockup-authority")
                        .value_name("KEYPAIR")
                        .help("Lockup authority keypair"),
                )
                .arg(
                    Arg::new("fee_payer")
                        .long("fee-payer")
                        .value_name("KEYPAIR")
                        .help("Fee payer keypair"),
                ),
        )
        .subcommand(
            ClapCommand::new("distribute-spl-tokens")
                .about("Distribute SPL tokens")
                .arg(
                    Arg::new("db_path")
                        .long("db-path")
                        .required(true)
                        .value_name("FILE")
                        .help(
                            "Path to file containing a list of recipients",
                        ),
                )
                .arg(
                    Arg::new("input_csv")
                        .long("input-csv")
                        .required(true)
                        .value_name("FILE")
                        .help("Input CSV file"),
                )
                .arg(
                    Arg::new("dry_run")
                        .long("dry-run")
                        .action(clap::ArgAction::SetTrue)
                        .help("Do not execute any transfers"),
                )
                .arg(
                    Arg::new("transfer_amount")
                        .long("transfer-amount")
                        .value_name("AMOUNT")
                        .help("The amount to transfer to each recipient"),
                )
                .arg(
                    Arg::new("output_path")
                        .long("output-path")
                        .value_name("FILE")
                        .help("Write the transaction log to this file"),
                )
                .arg(
                    Arg::new("sender_keypair")
                        .long("from")
                        .value_name("KEYPAIR")
                        .help("Sender keypair; reads the default wallet if not specified"),
                )
                .arg(
                    Arg::new("token_account_address")
                        .long("token-account-address")
                        .value_name("ADDRESS")
                        .help("Token account address"),
                )
                .arg(
                    Arg::new("token_owner")
                        .long("token-owner")
                        .value_name("KEYPAIR")
                        .help("Token owner keypair"),
                )
                .arg(
                    Arg::new("fee_payer")
                        .long("fee-payer")
                        .value_name("KEYPAIR")
                        .help("Fee payer keypair"),
                ),
        )
        .subcommand(
            ClapCommand::new("balances")
                .about("Get account balances")
                .arg(
                    Arg::new("input_csv")
                        .long("input-csv")
                        .value_name("FILE")
                        .help("Input CSV file"),
                )
        )
        .subcommand(
            ClapCommand::new("spl-token-balances")
                .about("Get SPL token account balances")
                .arg(
                    Arg::new("input_csv")
                        .long("input-csv")
                        .value_name("FILE")
                        .help("Input CSV file"),
                )
                .arg(
                    Arg::new("mint_address")
                        .long("mint-address")
                        .value_name("ADDRESS")
                        .help("Mint address"),
                )
        )
        .subcommand(
            ClapCommand::new("transaction-log")
                .about("Transaction log")
                .arg(
                    Arg::new("db_path")
                        .long("db-path")
                        .required(true)
                        .value_name("FILE")
                        .help("Path to file containing a list of recipients"),
                )
                .arg(
                    Arg::new("output_path")
                        .long("output-path")
                        .value_name("FILE")
                        .help("Write the transaction log to this file"),
                )
        )
        .get_matches_from(args)
}

fn parse_distribute_tokens_args(
    matches: &ArgMatches,
) -> Result<DistributeTokensArgs, Box<dyn Error>> {
    let _maybe_wallet_manager = maybe_wallet_manager()?;
    let input_csv = matches.get_one::<String>("input_csv").unwrap();
    let transaction_db = matches.get_one::<String>("db_path").unwrap();
    let transfer_amount = matches
        .get_one::<String>("transfer_amount")
        .map(|s| s.as_str())
        .and_then(|s| {
            if s == "ALL" {
                None
            } else {
                Some(sol_to_lamports(s.parse::<f64>().unwrap_or_default()))
            }
        });

    let sender_keypair = signer_from_path(
        matches,
        matches
            .get_one::<String>("sender_keypair")
            .map(|s| s.as_str())
            .unwrap_or(""),
        "sender",
        &mut None,
    )?;

    let fee_payer = signer_from_path(
        matches,
        matches
            .get_one::<String>("fee_payer")
            .map(|s| s.as_str())
            .unwrap_or(""),
        "fee_payer",
        &mut None,
    )?;

    Ok(DistributeTokensArgs {
        input_csv: input_csv.to_string(),
        transaction_db: transaction_db.to_string(),
        transfer_amount,
        sender_keypair,
        fee_payer,
        stake_args: None,
        spl_token_args: None,
        output_path: matches.get_one::<String>("output_path").map(|path| path.to_string()),
        dry_run: matches.get_flag("dry_run"),
    })
}

fn parse_create_stake_args(
    matches: &ArgMatches,
) -> Result<DistributeTokensArgs, Box<dyn Error>> {
    let _maybe_wallet_manager = maybe_wallet_manager()?;
    let input_csv = matches.get_one::<String>("input_csv").unwrap();
    let transaction_db = matches.get_one::<String>("db_path").unwrap();
    let unlocked_sol = sol_to_lamports(
        matches
            .get_one::<String>("unlocked_sol")
            .unwrap_or(&"1.0".to_string())
            .parse::<f64>()?,
    );

    let sender_keypair = signer_from_path(
        matches,
        matches
            .get_one::<String>("sender_keypair")
            .map(|s| s.as_str())
            .unwrap_or(""),
        "sender",
        &mut None,
    )?;

    let fee_payer = signer_from_path(
        matches,
        matches
            .get_one::<String>("fee_payer")
            .map(|s| s.as_str())
            .unwrap_or(""),
        "fee_payer",
        &mut None,
    )?;

    let lockup_authority = match matches.get_one::<String>("lockup_authority") {
        Some(path) => {
            let signer = signer_from_path(matches, path, "lockup_authority", &mut None)?;
            Some(signer.pubkey())
        }
        None => None,
    };

    let stake_args = StakeArgs {
        unlocked_sol,
        lockup_authority,
        sender_stake_args: None,
    };

    Ok(DistributeTokensArgs {
        input_csv: input_csv.to_string(),
        transaction_db: transaction_db.to_string(),
        transfer_amount: None,
        sender_keypair,
        fee_payer,
        stake_args: Some(stake_args),
        spl_token_args: None,
        output_path: matches.get_one::<String>("output_path").map(|path| path.to_string()),
        dry_run: matches.get_flag("dry_run"),
    })
}

fn parse_distribute_stake_args(
    matches: &ArgMatches,
) -> Result<DistributeTokensArgs, Box<dyn Error>> {
    let _maybe_wallet_manager = maybe_wallet_manager()?;
    let input_csv = matches.get_one::<String>("input_csv").unwrap();
    let transaction_db = matches.get_one::<String>("db_path").unwrap();
    let unlocked_sol = sol_to_lamports(
        matches
            .get_one::<String>("unlocked_sol")
            .unwrap_or(&"1.0".to_string())
            .parse::<f64>()?,
    );

    let sender_keypair = signer_from_path(
        matches,
        matches
            .get_one::<String>("sender_keypair")
            .map(|s| s.as_str())
            .unwrap_or(""),
        "sender",
        &mut None,
    )?;

    let fee_payer = signer_from_path(
        matches,
        matches
            .get_one::<String>("fee_payer")
            .map(|s| s.as_str())
            .unwrap_or(""),
        "fee_payer",
        &mut None,
    )?;

    let stake_account_address = pubkey_of_signer(
        matches,
        "stake_account_address",
        &mut None,
    )?.unwrap_or_default();

    let stake_authority = signer_from_path(
        matches,
        matches
            .get_one::<String>("stake_authority")
            .map(|s| s.as_str())
            .unwrap_or(""),
        "stake_authority",
        &mut None,
    )?;

    let withdraw_authority = signer_from_path(
        matches,
        matches
            .get_one::<String>("withdraw_authority")
            .map(|s| s.as_str())
            .unwrap_or(""),
        "withdraw_authority",
        &mut None,
    )?;

    let lockup_authority_keypair = match matches.get_one::<String>("lockup_authority") {
        Some(path) => Some(signer_from_path(matches, path, "lockup_authority", &mut None)?),
        None => None,
    };

    let lockup_authority_pubkey = lockup_authority_keypair.as_ref().map(|signer| signer.pubkey());

    let sender_stake_args = SenderStakeArgs {
        stake_account_address,
        stake_authority,
        withdraw_authority,
        lockup_authority: lockup_authority_keypair,
        rent_exempt_reserve: None,
    };

    let stake_args = StakeArgs {
        unlocked_sol,
        lockup_authority: lockup_authority_pubkey,
        sender_stake_args: Some(sender_stake_args),
    };

    Ok(DistributeTokensArgs {
        input_csv: input_csv.to_string(),
        transaction_db: transaction_db.to_string(),
        transfer_amount: None,
        sender_keypair,
        fee_payer,
        stake_args: Some(stake_args),
        spl_token_args: None,
        output_path: matches.get_one::<String>("output_path").map(|path| path.to_string()),
        dry_run: matches.get_flag("dry_run"),
    })
}

fn parse_distribute_spl_tokens_args(
    matches: &ArgMatches,
) -> Result<DistributeTokensArgs, Box<dyn Error>> {
    let _maybe_wallet_manager = maybe_wallet_manager()?;
    let input_csv = matches.get_one::<String>("input_csv").unwrap();
    let transaction_db = matches.get_one::<String>("db_path").unwrap();
    let transfer_amount = matches
        .get_one::<String>("transfer_amount")
        .map(|s| s.parse::<f64>().unwrap_or_default() as u64);

    let sender_keypair = signer_from_path(
        matches,
        matches
            .get_one::<String>("sender_keypair")
            .map(|s| s.as_str())
            .unwrap_or(""),
        "sender",
        &mut None,
    )?;

    let fee_payer = signer_from_path(
        matches,
        matches
            .get_one::<String>("fee_payer")
            .map(|s| s.as_str())
            .unwrap_or(""),
        "fee_payer",
        &mut None,
    )?;

    let token_account_address = pubkey_of_signer(
        matches,
        "token_account_address",
        &mut None,
    )?.unwrap_or_default();

    let spl_token_args = SplTokenArgs {
        token_account_address,
        ..SplTokenArgs::default()
    };

    Ok(DistributeTokensArgs {
        input_csv: input_csv.to_string(),
        transaction_db: transaction_db.to_string(),
        transfer_amount,
        sender_keypair,
        fee_payer,
        stake_args: None,
        spl_token_args: Some(spl_token_args),
        output_path: matches.get_one::<String>("output_path").map(|path| path.to_string()),
        dry_run: matches.get_flag("dry_run"),
    })
}

fn parse_balances_args(matches: &ArgMatches) -> Result<BalancesArgs, Box<dyn Error>> {
    let input_csv = matches.get_one::<String>("input_csv").unwrap();
    let spl_token_args = pubkey_of_signer(matches, "mint_address", &mut None)?.map(|mint| SplTokenArgs {
        mint,
        ..SplTokenArgs::default()
    });

    Ok(BalancesArgs {
        input_csv: input_csv.to_string(),
        spl_token_args,
    })
}

fn parse_transaction_log_args(matches: &ArgMatches) -> TransactionLogArgs {
    let db_path = matches.get_one::<String>("db_path").unwrap();
    let output_path = matches.get_one::<String>("output_path").unwrap();

    TransactionLogArgs {
        transaction_db: db_path.to_string(),
        output_path: output_path.to_string(),
    }
}

pub fn parse_args<I, T>(args: I) -> Result<Args, Box<dyn Error>>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = get_matches(args);
    let config_file = matches.get_one::<String>("config_file").unwrap().to_string();
    let url = matches.get_one::<String>("json_rpc_url").map(|x| x.to_string());

    let command = match matches.subcommand() {
        Some(("distribute-tokens", matches)) => {
            Command::DistributeTokens(parse_distribute_tokens_args(matches)?)
        }
        Some(("create-stake", matches)) => {
            Command::DistributeTokens(parse_create_stake_args(matches)?)
        }
        Some(("distribute-stake", matches)) => {
            Command::DistributeTokens(parse_distribute_stake_args(matches)?)
        }
        Some(("distribute-spl-tokens", matches)) => {
            Command::DistributeTokens(parse_distribute_spl_tokens_args(matches)?)
        }
        Some(("balances", matches)) => Command::Balances(parse_balances_args(matches)?),
        Some(("spl-token-balances", matches)) => Command::Balances(parse_balances_args(matches)?),
        Some(("transaction-log", matches)) => {
            Command::TransactionLog(parse_transaction_log_args(matches))
        }
        _ => {
            eprintln!("Error: No subcommand specified");
            exit(1);
        }
    };

    Ok(Args {
        config_file,
        url,
        command,
    })
}
