use {
    crate::cli::{CliCommand, CliCommandInfo, CliConfig, CliError, ProcessResult},
    clap::{App, AppSettings, Arg, ArgMatches, SubCommand},
    solana_account::from_account,
    solana_address_lookup_table_interface::{
        self as address_lookup_table,
        instruction::{
            close_lookup_table, create_lookup_table, deactivate_lookup_table, extend_lookup_table,
            freeze_lookup_table,
        },
        state::AddressLookupTable,
    },
    solana_clap_v3_utils::{self, input_parsers::*, input_validators::*, keypair::*},
    solana_cli_output::{CliAddressLookupTable, CliAddressLookupTableCreated, CliSignature},
    solana_clock::Clock,
    solana_commitment_config::CommitmentConfig,
    solana_message::Message,
    solana_pubkey::Pubkey,
    solana_remote_wallet::remote_wallet::RemoteWalletManager,
    solana_rpc_client::rpc_client::RpcClient,
    solana_rpc_client_api::config::RpcSendTransactionConfig,
    solana_sdk_ids::sysvar,
    solana_signer::Signer,
    solana_transaction::Transaction,
    std::{rc::Rc, sync::Arc},
};

#[derive(Debug, PartialEq, Eq)]
pub enum AddressLookupTableCliCommand {
    CreateLookupTable {
        authority_pubkey: Pubkey,
        payer_signer_index: SignerIndex,
    },
    FreezeLookupTable {
        lookup_table_pubkey: Pubkey,
        authority_signer_index: SignerIndex,
        bypass_warning: bool,
    },
    ExtendLookupTable {
        lookup_table_pubkey: Pubkey,
        authority_signer_index: SignerIndex,
        payer_signer_index: SignerIndex,
        new_addresses: Vec<Pubkey>,
    },
    DeactivateLookupTable {
        lookup_table_pubkey: Pubkey,
        authority_signer_index: SignerIndex,
        bypass_warning: bool,
    },
    CloseLookupTable {
        lookup_table_pubkey: Pubkey,
        authority_signer_index: SignerIndex,
        recipient_pubkey: Pubkey,
    },
    ShowLookupTable {
        lookup_table_pubkey: Pubkey,
    },
}

pub trait AddressLookupTableSubCommands {
    fn address_lookup_table_subcommands(self) -> Self;
}

impl<'a> AddressLookupTableSubCommands for App<'a> {
    fn address_lookup_table_subcommands(self) -> Self {
        self.subcommand(
            SubCommand::with_name("address-lookup-table")
                .about("Address lookup table management")
                .setting(AppSettings::SubcommandRequiredElseHelp)
                .subcommand(
                    SubCommand::with_name("create")
                        .about("Create a lookup table")
                        .arg(
                            Arg::with_name("authority")
                                .long("authority")
                                .alias("authority-signer")
                                .value_name("AUTHORITY_PUBKEY")
                                .takes_value(true)
                                .validator(crate::clap_app::validate_pubkey_or_keypair)
                                .help(
                                    "Lookup table authority address \
                                    [default: the default configured keypair].",
                                ),
                        )
                        .arg(
                            Arg::with_name("payer")
                                .long("payer")
                                .value_name("PAYER_SIGNER")
                                .takes_value(true)
                                .validator(crate::clap_app::validate_signer)
                                .help(
                                    "Account that will pay rent fees for the created lookup table \
                                     [default: the default configured keypair]",
                                ),
                        ),
                )
                .subcommand(
                    SubCommand::with_name("freeze")
                        .about("Permanently freezes a lookup table")
                        .arg(
                            Arg::with_name("lookup_table_address")
                                .index(1)
                                .value_name("LOOKUP_TABLE_ADDRESS")
                                .takes_value(true)
                                .required(true)
                                .validator(is_pubkey)
                                .help("Address of the lookup table"),
                        )
                        .arg(
                            Arg::with_name("authority")
                                .long("authority")
                                .value_name("AUTHORITY_SIGNER")
                                .takes_value(true)
                                .validator(crate::clap_app::validate_signer)
                                .help(
                                    "Lookup table authority \
                                    [default: the default configured keypair]",
                                ),
                        )
                        .arg(
                            Arg::with_name("bypass_warning")
                                .long("bypass-warning")
                                .takes_value(false)
                                .help("Bypass the permanent lookup table freeze warning"),
                        ),
                )
                .subcommand(
                    SubCommand::with_name("extend")
                        .about("Append more addresses to a lookup table")
                        .arg(
                            Arg::with_name("lookup_table_address")
                                .index(1)
                                .value_name("LOOKUP_TABLE_ADDRESS")
                                .takes_value(true)
                                .required(true)
                                .validator(is_pubkey)
                                .help("Address of the lookup table"),
                        )
                        .arg(
                            Arg::with_name("authority")
                                .long("authority")
                                .value_name("AUTHORITY_SIGNER")
                                .takes_value(true)
                                .validator(crate::clap_app::validate_signer)
                                .help(
                                    "Lookup table authority \
                                    [default: the default configured keypair]",
                                ),
                        )
                        .arg(
                            Arg::with_name("payer")
                                .long("payer")
                                .value_name("PAYER_SIGNER")
                                .takes_value(true)
                                .validator(crate::clap_app::validate_signer)
                                .help(
                                    "Account that will pay rent fees for the extended lookup \
                                     table [default: the default configured keypair]",
                                ),
                        )
                        .arg(
                            Arg::with_name("addresses")
                                .long("addresses")
                                .value_name("ADDRESS_1,ADDRESS_2")
                                .takes_value(true)
                                .use_delimiter(true)
                                .required(true)
                                .validator(is_pubkey)
                                .help("Comma separated list of addresses to append"),
                        ),
                )
                .subcommand(
                    SubCommand::with_name("deactivate")
                        .about("Permanently deactivates a lookup table")
                        .arg(
                            Arg::with_name("lookup_table_address")
                                .index(1)
                                .value_name("LOOKUP_TABLE_ADDRESS")
                                .takes_value(true)
                                .required(true)
                                .help("Address of the lookup table"),
                        )
                        .arg(
                            Arg::with_name("authority")
                                .long("authority")
                                .value_name("AUTHORITY_SIGNER")
                                .takes_value(true)
                                .validator(crate::clap_app::validate_signer)
                                .help(
                                    "Lookup table authority \
                                    [default: the default configured keypair]",
                                ),
                        )
                        .arg(
                            Arg::with_name("bypass_warning")
                                .long("bypass-warning")
                                .takes_value(false)
                                .help("Bypass the permanent lookup table deactivation warning"),
                        ),
                )
                .subcommand(
                    SubCommand::with_name("close")
                        .about("Permanently closes a lookup table")
                        .arg(
                            Arg::with_name("lookup_table_address")
                                .index(1)
                                .value_name("LOOKUP_TABLE_ADDRESS")
                                .takes_value(true)
                                .required(true)
                                .help("Address of the lookup table"),
                        )
                        .arg(
                            Arg::with_name("recipient")
                                .long("recipient")
                                .value_name("RECIPIENT_ADDRESS")
                                .takes_value(true)
                                .validator(is_pubkey)
                                .help(
                                    "Address of the recipient account to deposit the closed \
                                     account's lamports [default: the default configured keypair]",
                                ),
                        )
                        .arg(
                            Arg::with_name("authority")
                                .long("authority")
                                .value_name("AUTHORITY_SIGNER")
                                .takes_value(true)
                                .validator(crate::clap_app::validate_signer)
                                .help(
                                    "Lookup table authority \
                                    [default: the default configured keypair]",
                                ),
                        ),
                )
                .subcommand(
                    SubCommand::with_name("get")
                        .about("Display information about a lookup table")
                        .arg(
                            Arg::with_name("lookup_table_address")
                                .index(1)
                                .value_name("LOOKUP_TABLE_ADDRESS")
                                .takes_value(true)
                                .required(true)
                                .help("Address of the lookup table to show"),
                        ),
                ),
        )
    }
}

pub fn parse_address_lookup_table_subcommand(
    matches: &ArgMatches,
    default_signer: &DefaultSigner,
    wallet_manager: &mut Option<Rc<RemoteWalletManager>>,
) -> Result<CliCommandInfo, CliError> {
    let (subcommand, sub_matches) = match matches.subcommand() {
        Some((subcommand, sub_matches)) => (subcommand, sub_matches),
        None => return Err(CliError::CommandNotRecognized("No subcommand specified".to_string())),
    };

    let response = match (subcommand, sub_matches) {
        ("create", matches) => {
            let mut bulk_signers = vec![Some(
                default_signer.signer_from_path(matches, wallet_manager)?,
            )];

            let authority_pubkey = if let Some(authority_pubkey) = pubkey_of(matches, "authority") {
                authority_pubkey
            } else {
                default_signer
                    .signer_from_path(matches, wallet_manager)?
                    .pubkey()
            };

            let payer_pubkey = if let Ok((payer_signer, Some(payer_pubkey))) =
                signer_of(matches, "payer", wallet_manager)
            {
                bulk_signers.push(payer_signer);
                Some(payer_pubkey)
            } else {
                Some(
                    default_signer
                        .signer_from_path(matches, wallet_manager)?
                        .pubkey(),
                )
            };

            let signer_info =
                default_signer.generate_unique_signers(bulk_signers, matches, wallet_manager)?;

            CliCommandInfo {
                command: CliCommand::AddressLookupTable(
                    AddressLookupTableCliCommand::CreateLookupTable {
                        authority_pubkey,
                        payer_signer_index: signer_info.index_of(payer_pubkey).unwrap(),
                    },
                ),
                signers: signer_info.signers,
            }
        }
        ("freeze", matches) => {
            let lookup_table_pubkey = pubkey_of(matches, "lookup_table_address").unwrap();

            let mut bulk_signers = vec![Some(
                default_signer.signer_from_path(matches, wallet_manager)?,
            )];

            let authority_pubkey = if let Ok((authority_signer, Some(authority_pubkey))) =
                signer_of(matches, "authority", wallet_manager)
            {
                bulk_signers.push(authority_signer);
                Some(authority_pubkey)
            } else {
                Some(
                    default_signer
                        .signer_from_path(matches, wallet_manager)?
                        .pubkey(),
                )
            };

            let signer_info =
                default_signer.generate_unique_signers(bulk_signers, matches, wallet_manager)?;

            CliCommandInfo {
                command: CliCommand::AddressLookupTable(
                    AddressLookupTableCliCommand::FreezeLookupTable {
                        lookup_table_pubkey,
                        authority_signer_index: signer_info.index_of(authority_pubkey).unwrap(),
                        bypass_warning: matches.is_present("bypass_warning"),
                    },
                ),
                signers: signer_info.signers,
            }
        }
        ("extend", matches) => {
            let lookup_table_pubkey = pubkey_of(matches, "lookup_table_address").unwrap();

            let mut bulk_signers = vec![Some(
                default_signer.signer_from_path(matches, wallet_manager)?,
            )];

            let authority_pubkey = if let Ok((authority_signer, Some(authority_pubkey))) =
                signer_of(matches, "authority", wallet_manager)
            {
                bulk_signers.push(authority_signer);
                Some(authority_pubkey)
            } else {
                Some(
                    default_signer
                        .signer_from_path(matches, wallet_manager)?
                        .pubkey(),
                )
            };

            let payer_pubkey = if let Ok((payer_signer, Some(payer_pubkey))) =
                signer_of(matches, "payer", wallet_manager)
            {
                bulk_signers.push(payer_signer);
                Some(payer_pubkey)
            } else {
                Some(
                    default_signer
                        .signer_from_path(matches, wallet_manager)?
                        .pubkey(),
                )
            };

            let new_addresses: Vec<Pubkey> = values_of(matches, "addresses").unwrap();

            let signer_info =
                default_signer.generate_unique_signers(bulk_signers, matches, wallet_manager)?;

            CliCommandInfo {
                command: CliCommand::AddressLookupTable(
                    AddressLookupTableCliCommand::ExtendLookupTable {
                        lookup_table_pubkey,
                        authority_signer_index: signer_info.index_of(authority_pubkey).unwrap(),
                        payer_signer_index: signer_info.index_of(payer_pubkey).unwrap(),
                        new_addresses,
                    },
                ),
                signers: signer_info.signers,
            }
        }
        ("deactivate", matches) => {
            let lookup_table_pubkey = pubkey_of(matches, "lookup_table_address").unwrap();

            let mut bulk_signers = vec![Some(
                default_signer.signer_from_path(matches, wallet_manager)?,
            )];

            let authority_pubkey = if let Ok((authority_signer, Some(authority_pubkey))) =
                signer_of(matches, "authority", wallet_manager)
            {
                bulk_signers.push(authority_signer);
                Some(authority_pubkey)
            } else {
                Some(
                    default_signer
                        .signer_from_path(matches, wallet_manager)?
                        .pubkey(),
                )
            };

            let signer_info =
                default_signer.generate_unique_signers(bulk_signers, matches, wallet_manager)?;

            CliCommandInfo {
                command: CliCommand::AddressLookupTable(
                    AddressLookupTableCliCommand::DeactivateLookupTable {
                        lookup_table_pubkey,
                        authority_signer_index: signer_info.index_of(authority_pubkey).unwrap(),
                        bypass_warning: matches.is_present("bypass_warning"),
                    },
                ),
                signers: signer_info.signers,
            }
        }
        ("close", matches) => {
            let lookup_table_pubkey = pubkey_of(matches, "lookup_table_address").unwrap();

            let mut bulk_signers = vec![Some(
                default_signer.signer_from_path(matches, wallet_manager)?,
            )];

            let authority_pubkey = if let Ok((authority_signer, Some(authority_pubkey))) =
                signer_of(matches, "authority", wallet_manager)
            {
                bulk_signers.push(authority_signer);
                Some(authority_pubkey)
            } else {
                Some(
                    default_signer
                        .signer_from_path(matches, wallet_manager)?
                        .pubkey(),
                )
            };

            let recipient_pubkey = if let Some(recipient_pubkey) = pubkey_of(matches, "recipient") {
                recipient_pubkey
            } else {
                default_signer
                    .signer_from_path(matches, wallet_manager)?
                    .pubkey()
            };

            let signer_info =
                default_signer.generate_unique_signers(bulk_signers, matches, wallet_manager)?;

            CliCommandInfo {
                command: CliCommand::AddressLookupTable(
                    AddressLookupTableCliCommand::CloseLookupTable {
                        lookup_table_pubkey,
                        authority_signer_index: signer_info.index_of(authority_pubkey).unwrap(),
                        recipient_pubkey,
                    },
                ),
                signers: signer_info.signers,
            }
        }
        ("get", matches) => {
            let lookup_table_pubkey = pubkey_of(matches, "lookup_table_address").unwrap();

            CliCommandInfo::without_signers(CliCommand::AddressLookupTable(
                AddressLookupTableCliCommand::ShowLookupTable {
                    lookup_table_pubkey,
                },
            ))
        }
        _ => unreachable!(),
    };
    Ok(response)
}

pub fn process_address_lookup_table_subcommand(
    rpc_client: Arc<RpcClient>,
    config: &CliConfig,
    subcommand: &AddressLookupTableCliCommand,
) -> ProcessResult {
    match subcommand {
        AddressLookupTableCliCommand::CreateLookupTable {
            authority_pubkey,
            payer_signer_index,
        } => {
            process_create_lookup_table(&rpc_client, config, *authority_pubkey, *payer_signer_index)
        }
        AddressLookupTableCliCommand::FreezeLookupTable {
            lookup_table_pubkey,
            authority_signer_index,
            bypass_warning,
        } => process_freeze_lookup_table(
            &rpc_client,
            config,
            *lookup_table_pubkey,
            *authority_signer_index,
            *bypass_warning,
        ),
        AddressLookupTableCliCommand::ExtendLookupTable {
            lookup_table_pubkey,
            authority_signer_index,
            payer_signer_index,
            new_addresses,
        } => process_extend_lookup_table(
            &rpc_client,
            config,
            *lookup_table_pubkey,
            *authority_signer_index,
            *payer_signer_index,
            new_addresses.to_vec(),
        ),
        AddressLookupTableCliCommand::DeactivateLookupTable {
            lookup_table_pubkey,
            authority_signer_index,
            bypass_warning,
        } => process_deactivate_lookup_table(
            &rpc_client,
            config,
            *lookup_table_pubkey,
            *authority_signer_index,
            *bypass_warning,
        ),
        AddressLookupTableCliCommand::CloseLookupTable {
            lookup_table_pubkey,
            authority_signer_index,
            recipient_pubkey,
        } => process_close_lookup_table(
            &rpc_client,
            config,
            *lookup_table_pubkey,
            *authority_signer_index,
            *recipient_pubkey,
        ),
        AddressLookupTableCliCommand::ShowLookupTable {
            lookup_table_pubkey,
        } => process_show_lookup_table(&rpc_client, config, *lookup_table_pubkey),
    }
}

fn process_create_lookup_table(
    rpc_client: &RpcClient,
    config: &CliConfig,
    authority_address: Pubkey,
    payer_signer_index: usize,
) -> ProcessResult {
    let payer_signer = config.signers[payer_signer_index];

    let get_clock_result = rpc_client
        .get_account_with_commitment(&sysvar::clock::id(), CommitmentConfig::finalized())?;
    let clock_account = get_clock_result.value.expect("Clock account doesn't exist");
    let clock: Clock = from_account(&clock_account).ok_or_else(|| {
        CliError::RpcRequestError("Failed to deserialize clock sysvar".to_string())
    })?;

    let payer_address = payer_signer.pubkey();
    let (create_lookup_table_ix, lookup_table_address) =
        create_lookup_table(authority_address, payer_address, clock.slot);

    let blockhash = rpc_client.get_latest_blockhash()?;
    let mut tx = Transaction::new_unsigned(Message::new(
        &[create_lookup_table_ix],
        Some(&config.signers[0].pubkey()),
    ));

    let keypairs: Vec<&dyn Signer> = vec![config.signers[0], payer_signer];
    tx.try_sign(&keypairs, blockhash)?;
    let result = rpc_client.send_and_confirm_transaction_with_spinner_and_config(
        &tx,
        config.commitment,
        RpcSendTransactionConfig {
            skip_preflight: false,
            preflight_commitment: Some(config.commitment.commitment),
            ..RpcSendTransactionConfig::default()
        },
    );
    match result {
        Err(err) => Err(format!("Create failed: {err}").into()),
        Ok(signature) => Ok(config
            .output_format
            .formatted_string(&CliAddressLookupTableCreated {
                lookup_table_address: lookup_table_address.to_string(),
                signature: signature.to_string(),
            })),
    }
}

pub const FREEZE_LOOKUP_TABLE_WARNING: &str =
    "WARNING! Once a lookup table is frozen, it can never be modified or unfrozen again. To \
     proceed with freezing, rerun the `freeze` command with the `--bypass-warning` flag";

fn process_freeze_lookup_table(
    rpc_client: &RpcClient,
    config: &CliConfig,
    lookup_table_pubkey: Pubkey,
    authority_signer_index: usize,
    bypass_warning: bool,
) -> ProcessResult {
    let authority_signer = config.signers[authority_signer_index];

    let get_lookup_table_result =
        rpc_client.get_account_with_commitment(&lookup_table_pubkey, config.commitment)?;
    let lookup_table_account = get_lookup_table_result.value.ok_or_else(|| {
        format!("Lookup table account {lookup_table_pubkey} not found, was it already closed?")
    })?;
    if !address_lookup_table::program::check_id(&lookup_table_account.owner) {
        return Err(format!(
            "Lookup table account {lookup_table_pubkey} is not owned by the Address Lookup Table \
             program",
        )
        .into());
    }

    if !bypass_warning {
        return Err(String::from(FREEZE_LOOKUP_TABLE_WARNING).into());
    }

    let authority_address = authority_signer.pubkey();
    let freeze_lookup_table_ix = freeze_lookup_table(lookup_table_pubkey, authority_address);

    let blockhash = rpc_client.get_latest_blockhash()?;
    let mut tx = Transaction::new_unsigned(Message::new(
        &[freeze_lookup_table_ix],
        Some(&config.signers[0].pubkey()),
    ));

    tx.try_sign(&[config.signers[0], authority_signer], blockhash)?;
    let result = rpc_client.send_and_confirm_transaction_with_spinner_and_config(
        &tx,
        config.commitment,
        RpcSendTransactionConfig {
            skip_preflight: false,
            preflight_commitment: Some(config.commitment.commitment),
            ..RpcSendTransactionConfig::default()
        },
    );
    match result {
        Err(err) => Err(format!("Freeze failed: {err}").into()),
        Ok(signature) => Ok(config.output_format.formatted_string(&CliSignature {
            signature: signature.to_string(),
        })),
    }
}

fn process_extend_lookup_table(
    rpc_client: &RpcClient,
    config: &CliConfig,
    lookup_table_pubkey: Pubkey,
    authority_signer_index: usize,
    payer_signer_index: usize,
    new_addresses: Vec<Pubkey>,
) -> ProcessResult {
    let authority_signer = config.signers[authority_signer_index];
    let payer_signer = config.signers[payer_signer_index];

    if new_addresses.is_empty() {
        return Err("Lookup tables must be extended by at least one address".into());
    }

    let get_lookup_table_result =
        rpc_client.get_account_with_commitment(&lookup_table_pubkey, config.commitment)?;
    let lookup_table_account = get_lookup_table_result.value.ok_or_else(|| {
        format!("Lookup table account {lookup_table_pubkey} not found, was it already closed?")
    })?;
    if !address_lookup_table::program::check_id(&lookup_table_account.owner) {
        return Err(format!(
            "Lookup table account {lookup_table_pubkey} is not owned by the Address Lookup Table \
             program",
        )
        .into());
    }

    let authority_address = authority_signer.pubkey();
    let payer_address = payer_signer.pubkey();
    let extend_lookup_table_ix = extend_lookup_table(
        lookup_table_pubkey,
        authority_address,
        Some(payer_address),
        new_addresses,
    );

    let blockhash = rpc_client.get_latest_blockhash()?;
    let mut tx = Transaction::new_unsigned(Message::new(
        &[extend_lookup_table_ix],
        Some(&config.signers[0].pubkey()),
    ));

    tx.try_sign(&[config.signers[0], authority_signer], blockhash)?;
    let result = rpc_client.send_and_confirm_transaction_with_spinner_and_config(
        &tx,
        config.commitment,
        RpcSendTransactionConfig {
            skip_preflight: false,
            preflight_commitment: Some(config.commitment.commitment),
            ..RpcSendTransactionConfig::default()
        },
    );
    match result {
        Err(err) => Err(format!("Extend failed: {err}").into()),
        Ok(signature) => Ok(config.output_format.formatted_string(&CliSignature {
            signature: signature.to_string(),
        })),
    }
}

pub const DEACTIVATE_LOOKUP_TABLE_WARNING: &str =
    "WARNING! Once a lookup table is deactivated, it is no longer usable by transactions.
Deactivated lookup tables may only be closed and cannot be recreated at the same address. To \
     proceed with deactivation, rerun the `deactivate` command with the `--bypass-warning` flag";

fn process_deactivate_lookup_table(
    rpc_client: &RpcClient,
    config: &CliConfig,
    lookup_table_pubkey: Pubkey,
    authority_signer_index: usize,
    bypass_warning: bool,
) -> ProcessResult {
    let authority_signer = config.signers[authority_signer_index];

    let get_lookup_table_result =
        rpc_client.get_account_with_commitment(&lookup_table_pubkey, config.commitment)?;
    let lookup_table_account = get_lookup_table_result.value.ok_or_else(|| {
        format!("Lookup table account {lookup_table_pubkey} not found, was it already closed?")
    })?;
    if !address_lookup_table::program::check_id(&lookup_table_account.owner) {
        return Err(format!(
            "Lookup table account {lookup_table_pubkey} is not owned by the Address Lookup Table \
             program",
        )
        .into());
    }

    if !bypass_warning {
        return Err(String::from(DEACTIVATE_LOOKUP_TABLE_WARNING).into());
    }

    let authority_address = authority_signer.pubkey();
    let deactivate_lookup_table_ix =
        deactivate_lookup_table(lookup_table_pubkey, authority_address);

    let blockhash = rpc_client.get_latest_blockhash()?;
    let mut tx = Transaction::new_unsigned(Message::new(
        &[deactivate_lookup_table_ix],
        Some(&config.signers[0].pubkey()),
    ));

    tx.try_sign(&[config.signers[0], authority_signer], blockhash)?;
    let result = rpc_client.send_and_confirm_transaction_with_spinner_and_config(
        &tx,
        config.commitment,
        RpcSendTransactionConfig {
            skip_preflight: false,
            preflight_commitment: Some(config.commitment.commitment),
            ..RpcSendTransactionConfig::default()
        },
    );
    match result {
        Err(err) => Err(format!("Deactivate failed: {err}").into()),
        Ok(signature) => Ok(config.output_format.formatted_string(&CliSignature {
            signature: signature.to_string(),
        })),
    }
}

fn process_close_lookup_table(
    rpc_client: &RpcClient,
    config: &CliConfig,
    lookup_table_pubkey: Pubkey,
    authority_signer_index: usize,
    recipient_pubkey: Pubkey,
) -> ProcessResult {
    let authority_signer = config.signers[authority_signer_index];

    let get_lookup_table_result =
        rpc_client.get_account_with_commitment(&lookup_table_pubkey, config.commitment)?;
    let lookup_table_account = get_lookup_table_result.value.ok_or_else(|| {
        format!("Lookup table account {lookup_table_pubkey} not found, was it already closed?")
    })?;
    if !address_lookup_table::program::check_id(&lookup_table_account.owner) {
        return Err(format!(
            "Lookup table account {lookup_table_pubkey} is not owned by the Address Lookup Table \
             program",
        )
        .into());
    }

    let lookup_table_account = AddressLookupTable::deserialize(&lookup_table_account.data)?;
    if lookup_table_account.meta.deactivation_slot == u64::MAX {
        return Err(format!(
            "Lookup table account {lookup_table_pubkey} is not deactivated. Only deactivated \
             lookup tables may be closed",
        )
        .into());
    }

    let authority_address = authority_signer.pubkey();
    let close_lookup_table_ix =
        close_lookup_table(lookup_table_pubkey, authority_address, recipient_pubkey);

    let blockhash = rpc_client.get_latest_blockhash()?;
    let mut tx = Transaction::new_unsigned(Message::new(
        &[close_lookup_table_ix],
        Some(&config.signers[0].pubkey()),
    ));

    tx.try_sign(&[config.signers[0], authority_signer], blockhash)?;
    let result = rpc_client.send_and_confirm_transaction_with_spinner_and_config(
        &tx,
        config.commitment,
        RpcSendTransactionConfig {
            skip_preflight: false,
            preflight_commitment: Some(config.commitment.commitment),
            ..RpcSendTransactionConfig::default()
        },
    );
    match result {
        Err(err) => Err(format!("Close failed: {err}").into()),
        Ok(signature) => Ok(config.output_format.formatted_string(&CliSignature {
            signature: signature.to_string(),
        })),
    }
}

fn process_show_lookup_table(
    rpc_client: &RpcClient,
    config: &CliConfig,
    lookup_table_pubkey: Pubkey,
) -> ProcessResult {
    let get_lookup_table_result =
        rpc_client.get_account_with_commitment(&lookup_table_pubkey, config.commitment)?;
    let lookup_table_account = get_lookup_table_result.value.ok_or_else(|| {
        format!("Lookup table account {lookup_table_pubkey} not found, was it already closed?")
    })?;
    if !address_lookup_table::program::check_id(&lookup_table_account.owner) {
        return Err(format!(
            "Lookup table account {lookup_table_pubkey} is not owned by the Address Lookup Table \
             program",
        )
        .into());
    }

    let lookup_table_account = AddressLookupTable::deserialize(&lookup_table_account.data)?;
    Ok(config
        .output_format
        .formatted_string(&CliAddressLookupTable {
            lookup_table_address: lookup_table_pubkey.to_string(),
            authority: lookup_table_account
                .meta
                .authority
                .as_ref()
                .map(ToString::to_string),
            deactivation_slot: lookup_table_account.meta.deactivation_slot,
            last_extended_slot: lookup_table_account.meta.last_extended_slot,
            addresses: lookup_table_account
                .addresses
                .iter()
                .map(ToString::to_string)
                .collect(),
        }))
}
