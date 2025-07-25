use {
    gorb_validator::{
        admin_rpc_service, cli, dashboard::Dashboard, ledger_lockfile, lock_ledger,
        println_name_value,
    },
    clap::crate_name,
    // Remove value_t, value_t_or_exit, values_t_or_exit - these don't exist in clap v4
    crossbeam_channel::unbounded,
    itertools::Itertools,
    log::*,
    solana_account::AccountSharedData,
    solana_accounts_db::accounts_index::{AccountIndex, AccountSecondaryIndexes},
    solana_clap_utils::{
        input_parsers::parse_cpu_ranges,
        input_validators::normalize_to_url_if_moniker,
    },
    solana_clock::Slot,
    solana_core::consensus::tower_storage::FileTowerStorage,
    solana_epoch_schedule::EpochSchedule,
    solana_faucet::faucet::run_local_faucet_with_port,
    solana_inflation::Inflation,
    solana_keypair::{read_keypair_file, write_keypair_file, Keypair},
    solana_logger::redirect_stderr_to_file,
    solana_native_token::sol_to_lamports,
    solana_pubkey::Pubkey,
    solana_rent::Rent,
    solana_rpc::{
        rpc::{JsonRpcConfig, RpcBigtableConfig},
        rpc_pubsub_service::PubSubConfig,
    },
    solana_rpc_client::rpc_client::RpcClient,
    solana_signer::Signer,
    solana_streamer::socket::SocketAddrSpace,
    solana_system_interface::program as system_program,
    solana_test_validator::*,
    std::{
        collections::{HashMap, HashSet},
        fs, io,
        net::{IpAddr, Ipv4Addr, SocketAddr},
        path::{Path, PathBuf},
        process::exit,
        sync::{Arc, RwLock},
        time::{Duration, SystemTime, UNIX_EPOCH},
    },
};

#[derive(PartialEq, Eq)]
enum Output {
    None,
    Log,
    Dashboard,
}

fn main() {
    let default_args = cli::DefaultTestArgs::new();
    let version = solana_version::version!();
    let matches = cli::test_app(version, &default_args).get_matches();

    let output = if matches.get_flag("quiet") {
        Output::None
    } else if matches.get_flag("log") {
        Output::Log
    } else {
        Output::Dashboard
    };

    let ledger_path = matches.get_one::<String>("ledger_path").unwrap().parse::<PathBuf>().unwrap();
    let reset_ledger = matches.get_flag("reset");

    let indexes: HashSet<AccountIndex> = matches
        .get_many::<String>("account_indexes")
        .unwrap_or_default()
        .map(|value| match value.as_str() {
            "program-id" => AccountIndex::ProgramId,
            "spl-token-mint" => AccountIndex::SplTokenMint,
            "spl-token-owner" => AccountIndex::SplTokenOwner,
            _ => unreachable!(),
        })
        .collect();

    let account_indexes = AccountSecondaryIndexes {
        keys: None,
        indexes,
    };

    if !ledger_path.exists() {
        fs::create_dir(&ledger_path).unwrap_or_else(|err| {
            println!(
                "Error: Unable to create directory {}: {}",
                ledger_path.display(),
                err
            );
            exit(1);
        });
    }

    let mut ledger_lock = ledger_lockfile(&ledger_path);
    let _ledger_write_guard = lock_ledger(&ledger_path, &mut ledger_lock);
    if reset_ledger {
        remove_directory_contents(&ledger_path).unwrap_or_else(|err| {
            println!("Error: Unable to remove {}: {}", ledger_path.display(), err);
            exit(1);
        })
    }
    solana_runtime::snapshot_utils::remove_tmp_snapshot_archives(&ledger_path);

    let validator_log_symlink = ledger_path.join("validator.log");

    let logfile = if output != Output::Log {
        let validator_log_with_timestamp = format!(
            "validator-{}.log",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );

        let _ = fs::remove_file(&validator_log_symlink);
        symlink::symlink_file(&validator_log_with_timestamp, &validator_log_symlink).unwrap();

        Some(
            ledger_path
                .join(validator_log_with_timestamp)
                .into_os_string()
                .into_string()
                .unwrap(),
        )
    } else {
        None
    };
    let _logger_thread = redirect_stderr_to_file(logfile);

    info!("{} {}", crate_name!(), solana_version::version!());
    info!("Starting validator with: {:#?}", std::env::args_os());
    solana_core::validator::report_target_features();

    // TODO: Ideally test-validator should *only* allow private addresses.
    let socket_addr_space = SocketAddrSpace::new(/*allow_private_addr=*/ true);
    let cli_config = if let Some(config_file) = matches.get_one::<String>("config_file") {
        solana_cli_config::Config::load(config_file).unwrap_or_default()
    } else {
        solana_cli_config::Config::default()
    };

    let cluster_rpc_client = matches.get_one::<String>("json_rpc_url")
        .map(|s| s.parse::<String>().unwrap())
        .map(normalize_to_url_if_moniker)
        .map(RpcClient::new);

    let (mint_address, random_mint) = if let Some(mint_str) = matches.get_one::<String>("mint_address") {
        if let Ok(pk) = mint_str.parse::<Pubkey>() {
            (pk, false)
        } else {
            let keypair = read_keypair_file(&cli_config.keypair_path)
                .unwrap_or_else(|_| {
                    eprintln!("Error: unable to read keypair file");
                    exit(1);
                });
            (keypair.pubkey(), true)
        }
    } else {
        let keypair = read_keypair_file(&cli_config.keypair_path)
            .unwrap_or_else(|_| {
                eprintln!("Error: unable to read keypair file");
                exit(1);
            });
        (keypair.pubkey(), true)
    };

    let rpc_port = matches.get_one::<String>("rpc_port").unwrap().parse::<u16>().unwrap();
    let enable_vote_subscription = matches.get_flag("rpc_pubsub_enable_vote_subscription");
    let enable_block_subscription = matches.get_flag("rpc_pubsub_enable_block_subscription");
    let faucet_port = matches.get_one::<String>("faucet_port").unwrap().parse::<u16>().unwrap();
    let ticks_per_slot = matches.get_one::<String>("ticks_per_slot").map(|s| s.parse::<u64>().unwrap());
    let slots_per_epoch = matches.get_one::<String>("slots_per_epoch").map(|s| s.parse::<Slot>().unwrap());
    let inflation_fixed = matches.get_one::<String>("inflation_fixed").map(|s| s.parse::<f64>().unwrap());
    let gossip_host = matches.get_one::<String>("gossip_host").map(|gossip_host| {
        warn!("--gossip-host is deprecated. Use --bind-address instead.");
        solana_net_utils::parse_host(gossip_host).unwrap_or_else(|err| {
            eprintln!("Failed to parse --gossip-host: {err}");
            exit(1);
        })
    });
    let gossip_port = matches.get_one::<String>("gossip_port").map(|s| s.parse::<u16>().unwrap());
    let dynamic_port_range = matches.get_one::<String>("dynamic_port_range").map(|port_range| {
        solana_net_utils::parse_port_range(port_range).unwrap_or_else(|| {
            eprintln!("Failed to parse --dynamic-port-range");
            exit(1);
        })
    });
    let bind_address = solana_net_utils::parse_host(
        matches
            .get_one::<String>("bind_address")
            .expect("Bind address has default value"),
    )
    .unwrap_or_else(|err| {
        eprintln!("Failed to parse --bind-address: {err}");
        exit(1);
    });

    let advertised_ip = if let Some(ip) = gossip_host {
        ip
    } else if !bind_address.is_unspecified() && !bind_address.is_loopback() {
        bind_address
    } else {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    };

    let compute_unit_limit = matches.get_one::<String>("compute_unit_limit").map(|s| s.parse::<u64>().unwrap());

    let faucet_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), faucet_port);

    let parse_address = |address: &str, input_type: &str| {
        address
            .parse::<Pubkey>()
            .or_else(|_| read_keypair_file(address).map(|keypair| keypair.pubkey()))
            .unwrap_or_else(|err| {
                println!("Error: invalid {input_type} {address}: {err}");
                exit(1);
            })
    };

    let parse_program_path = |program: &str| {
        let program_path = PathBuf::from(program);
        if !program_path.exists() {
            println!(
                "Error: program file does not exist: {}",
                program_path.display()
            );
            exit(1);
        }
        program_path
    };

    let mut upgradeable_programs_to_load = vec![];
    if let Some(values) = matches.get_many::<String>("bpf_program") {
        for (address, program) in values.into_iter().tuples() {
            let address = parse_address(address, "address");
            let program_path = parse_program_path(program);

            upgradeable_programs_to_load.push(UpgradeableProgramInfo {
                program_id: address,
                loader: solana_sdk_ids::bpf_loader_upgradeable::id(),
                upgrade_authority: Pubkey::default(),
                program_path,
            });
        }
    }

    if let Some(values) = matches.get_many::<String>("upgradeable_program") {
        for (address, program, upgrade_authority) in
            values.into_iter().map(|s| s.as_str()).tuples::<(&str, &str, &str)>()
        {
            let address = parse_address(address, "address");
            let program_path = parse_program_path(program);
            let upgrade_authority_address = if upgrade_authority == "none" {
                Pubkey::default()
            } else {
                upgrade_authority
                    .parse::<Pubkey>()
                    .or_else(|_| {
                        read_keypair_file(upgrade_authority).map(|keypair| keypair.pubkey())
                    })
                    .unwrap_or_else(|err| {
                        println!("Error: invalid upgrade_authority {upgrade_authority}: {err}");
                        exit(1);
                    })
            };

            upgradeable_programs_to_load.push(UpgradeableProgramInfo {
                program_id: address,
                loader: solana_sdk_ids::bpf_loader_upgradeable::id(),
                upgrade_authority: upgrade_authority_address,
                program_path,
            });
        }
    }

    let mut accounts_to_load = vec![];
    if let Some(values) = matches.get_many::<String>("account") {
        for (address, filename) in values.into_iter().tuples() {
            let address = if address == "-" {
                None
            } else {
                Some(address.parse::<Pubkey>().unwrap_or_else(|err| {
                    println!("Error: invalid address {address}: {err}");
                    exit(1);
                }))
            };

            accounts_to_load.push(AccountInfo { address, filename });
        }
    }

    let accounts_from_dirs: HashSet<_> = matches
        .get_many::<String>("account_dir")
        .unwrap_or_default()
        .collect();

    let accounts_to_clone: HashSet<_> = matches.get_many::<String>("clone_account")
        .unwrap_or_default()
        .filter_map(|s| s.parse::<Pubkey>().ok())
        .collect();

    let accounts_to_maybe_clone: HashSet<_> = matches.get_many::<String>("maybe_clone_account")
        .unwrap_or_default()
        .filter_map(|s| s.parse::<Pubkey>().ok())
        .collect();

    let upgradeable_programs_to_clone: HashSet<_> =
        matches.get_many::<String>("clone_upgradeable_program")
            .unwrap_or_default()
            .filter_map(|s| s.parse::<Pubkey>().ok())
            .collect();

    let alt_accounts_to_clone: HashSet<_> = matches.get_many::<String>("deep_clone_address_lookup_table")
        .unwrap_or_default()
        .filter_map(|s| s.parse::<Pubkey>().ok())
        .collect();

    let clone_feature_set = matches.get_flag("clone_feature_set");

    let warp_slot = if matches.get_flag("warp_slot") {
        Some(match matches.get_one::<String>("warp_slot") {
            Some(_) => matches.get_one::<String>("warp_slot").unwrap().parse::<Slot>().unwrap(),
            None => cluster_rpc_client
                .as_ref()
                .unwrap_or_else(|| {
                    println!(
                        "The --url argument must be provided if --warp-slot/-w is used without an \
                         explicit slot"
                    );
                    exit(1);
                })
                .get_slot()
                .unwrap_or_else(|err| {
                    println!("Unable to get current cluster slot: {err}");
                    exit(1);
                }),
        })
    } else {
        None
    };

    let faucet_lamports = sol_to_lamports(matches.get_one::<String>("faucet_sol").unwrap().parse::<f64>().unwrap());
    let faucet_keypair_file = ledger_path.join("faucet-keypair.json");
    if !faucet_keypair_file.exists() {
        write_keypair_file(&Keypair::new(), faucet_keypair_file.to_str().unwrap()).unwrap_or_else(
            |err| {
                println!(
                    "Error: Failed to write {}: {}",
                    faucet_keypair_file.display(),
                    err
                );
                exit(1);
            },
        );
    }

    let faucet_keypair =
        read_keypair_file(faucet_keypair_file.to_str().unwrap()).unwrap_or_else(|err| {
            println!(
                "Error: Failed to read {}: {}",
                faucet_keypair_file.display(),
                err
            );
            exit(1);
        });
    let faucet_pubkey = faucet_keypair.pubkey();

    let faucet_time_slice_secs = matches.get_one::<String>("faucet_time_slice_secs").unwrap().parse::<u64>().unwrap();
    let faucet_per_time_cap = matches.get_one::<String>("faucet_per_time_sol_cap")
        .map(|s| s.parse::<f64>().unwrap())
        .map(sol_to_lamports);
    let faucet_per_request_cap = matches.get_one::<String>("faucet_per_request_sol_cap")
        .map(|s| s.parse::<f64>().unwrap())
        .map(sol_to_lamports);

    let (sender, receiver) = unbounded();
    run_local_faucet_with_port(
        faucet_keypair,
        sender,
        Some(faucet_time_slice_secs),
        faucet_per_time_cap,
        faucet_per_request_cap,
        faucet_addr.port(),
    );
    let _ = receiver.recv().expect("run faucet").unwrap_or_else(|err| {
        println!("Error: failed to start faucet: {err}");
        exit(1);
    });

    let features_to_deactivate: HashSet<_> = matches.get_many::<String>("deactivate_feature")
        .unwrap_or_default()
        .filter_map(|s| s.parse::<Pubkey>().ok())
        .collect();

    if TestValidatorGenesis::ledger_exists(&ledger_path) {
        for (name, long) in &[
            ("bpf_program", "--bpf-program"),
            ("clone_account", "--clone"),
            ("account", "--account"),
            ("mint_address", "--mint"),
            ("ticks_per_slot", "--ticks-per-slot"),
            ("slots_per_epoch", "--slots-per-epoch"),
            ("inflation_fixed", "--inflation-fixed"),
            ("faucet_sol", "--faucet-sol"),
            ("deactivate_feature", "--deactivate-feature"),
        ] {
            if matches.get_flag(name) {
                println!("{long} argument ignored, ledger already exists");
            }
        }
    } else if random_mint {
        println_name_value(
            "\nNotice!",
            "No wallet available. `solana airdrop` localnet SOL after creating one\n",
        );
    }

    let mut genesis = TestValidatorGenesis::default();
    genesis.max_ledger_shreds = matches.get_one::<String>("limit_ledger_size").map(|s| s.parse::<u64>().unwrap());
    genesis.max_genesis_archive_unpacked_size = Some(u64::MAX);
    genesis.log_messages_bytes_limit = matches.get_one::<String>("log_messages_bytes_limit").map(|s| s.parse::<usize>().unwrap());
    genesis.transaction_account_lock_limit =
        matches.get_one::<String>("transaction_account_lock_limit").map(|s| s.parse::<usize>().unwrap());

    let tower_storage = Arc::new(FileTowerStorage::new(ledger_path.clone()));

    let admin_service_post_init = Arc::new(RwLock::new(None));
    // If geyser_plugin_config value is invalid, the validator will exit when the values are extracted below
    let (rpc_to_plugin_manager_sender, rpc_to_plugin_manager_receiver) =
        if matches.get_flag("geyser_plugin_config") {
            let (sender, receiver) = unbounded();
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
    admin_rpc_service::run(
        &ledger_path,
        admin_rpc_service::AdminRpcRequestMetadata {
            rpc_addr: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), rpc_port)),
            start_progress: genesis.start_progress.clone(),
            start_time: std::time::SystemTime::now(),
            validator_exit: genesis.validator_exit.clone(),
            validator_exit_backpressure: HashMap::default(),
            authorized_voter_keypairs: genesis.authorized_voter_keypairs.clone(),
            staked_nodes_overrides: genesis.staked_nodes_overrides.clone(),
            post_init: admin_service_post_init,
            tower_storage: tower_storage.clone(),
            rpc_to_plugin_manager_sender,
        },
    );
    let dashboard = if output == Output::Dashboard {
        Some(Dashboard::new(
            &ledger_path,
            Some(&validator_log_symlink),
            Some(&mut genesis.validator_exit.write().unwrap()),
        ))
    } else {
        None
    };

    let rpc_bigtable_config = if matches.get_flag("enable_rpc_bigtable_ledger_storage")
        || matches.get_flag("enable_bigtable_ledger_upload")
    {
        Some(RpcBigtableConfig {
            enable_bigtable_ledger_upload: matches.get_flag("enable_bigtable_ledger_upload"),
            bigtable_instance_name: matches.get_one::<String>("rpc_bigtable_instance").unwrap_or_else(|| std::process::exit(1)).parse::<String>().unwrap(),
            bigtable_app_profile_id: matches.get_one::<String>("rpc_bigtable_app_profile_id").unwrap_or_else(|| std::process::exit(1)).parse::<String>().unwrap(),
            timeout: None,
            ..RpcBigtableConfig::default()
        })
    } else {
        None
    };

    genesis
        .ledger_path(&ledger_path)
        .tower_storage(tower_storage)
        .add_account(
            faucet_pubkey,
            AccountSharedData::new(faucet_lamports, 0, &system_program::id()),
        )
        .pubsub_config(PubSubConfig {
            enable_vote_subscription,
            enable_block_subscription,
            ..PubSubConfig::default()
        })
        .rpc_port(rpc_port)
        .add_upgradeable_programs_with_path(&upgradeable_programs_to_load)
        .add_accounts_from_json_files(&accounts_to_load)
        .unwrap_or_else(|e| {
            println!("Error: add_accounts_from_json_files failed: {e}");
            exit(1);
        })
        .add_accounts_from_directories(&accounts_from_dirs)
        .unwrap_or_else(|e| {
            println!("Error: add_accounts_from_directories failed: {e}");
            exit(1);
        })
        .deactivate_features(&features_to_deactivate.into_iter().collect::<Vec<_>>());

    genesis.rpc_config(JsonRpcConfig {
        enable_rpc_transaction_history: true,
        enable_extended_tx_metadata_storage: true,
        rpc_bigtable_config,
        faucet_addr: Some(faucet_addr),
        account_indexes,
        ..JsonRpcConfig::default_for_test()
    });

    if !accounts_to_clone.is_empty() {
        if let Err(e) = genesis.clone_accounts(
            accounts_to_clone,
            cluster_rpc_client
                .as_ref()
                .expect("--clone-account requires --json-rpc-url argument"),
            false,
        ) {
            println!("Error: clone_accounts failed: {e}");
            exit(1);
        }
    }

    if !alt_accounts_to_clone.is_empty() {
        if let Err(e) = genesis.deep_clone_address_lookup_table_accounts(
            alt_accounts_to_clone,
            cluster_rpc_client
                .as_ref()
                .expect("--deep-clone-address-lookup-table requires --json-rpc-url argument"),
        ) {
            println!("Error: alt_accounts_to_clone failed: {e}");
            exit(1);
        }
    }

    if !accounts_to_maybe_clone.is_empty() {
        if let Err(e) = genesis.clone_accounts(
            accounts_to_maybe_clone,
            cluster_rpc_client
                .as_ref()
                .expect("--maybe-clone requires --json-rpc-url argument"),
            true,
        ) {
            println!("Error: clone_accounts failed: {e}");
            exit(1);
        }
    }

    if !upgradeable_programs_to_clone.is_empty() {
        if let Err(e) = genesis.clone_upgradeable_programs(
            upgradeable_programs_to_clone,
            cluster_rpc_client
                .as_ref()
                .expect("--clone-upgradeable-program requires --json-rpc-url argument"),
        ) {
            println!("Error: clone_upgradeable_programs failed: {e}");
            exit(1);
        }
    }

    if clone_feature_set {
        if let Err(e) = genesis.clone_feature_set(
            cluster_rpc_client
                .as_ref()
                .expect("--clone-feature-set requires --json-rpc-url argument"),
        ) {
            println!("Error: clone_feature_set failed: {e}");
            exit(1);
        }
    }

    if let Some(warp_slot) = warp_slot {
        genesis.warp_slot(warp_slot);
    }

    if let Some(ticks_per_slot) = ticks_per_slot {
        genesis.ticks_per_slot(ticks_per_slot);
    }

    if let Some(slots_per_epoch) = slots_per_epoch {
        genesis.epoch_schedule(EpochSchedule::custom(
            slots_per_epoch,
            slots_per_epoch,
            /* enable_warmup_epochs = */ false,
        ));

        genesis.rent = Rent::with_slots_per_epoch(slots_per_epoch);
    }

    if let Some(inflation_fixed) = inflation_fixed {
        genesis.inflation(Inflation::new_fixed(inflation_fixed));
    }

    genesis.gossip_host(advertised_ip);

    if let Some(gossip_port) = gossip_port {
        genesis.gossip_port(gossip_port);
    }

    if let Some(dynamic_port_range) = dynamic_port_range {
        genesis.port_range(dynamic_port_range);
    }

    genesis.bind_ip_addr(bind_address);

    if matches.get_flag("geyser_plugin_config") {
        genesis.geyser_plugin_config_files = Some(
            matches.get_many::<String>("geyser_plugin_config").unwrap().map(|s| s.to_string()).collect::<Vec<String>>()
                .into_iter()
                .map(PathBuf::from)
                .collect(),
        );
    }

    if let Some(compute_unit_limit) = compute_unit_limit {
        genesis.compute_unit_limit(compute_unit_limit);
    }

    match genesis.start_with_mint_address_and_geyser_plugin_rpc(
        mint_address,
        socket_addr_space,
        rpc_to_plugin_manager_receiver,
    ) {
        Ok(test_validator) => {
            if let Some(dashboard) = dashboard {
                dashboard.run(Duration::from_millis(250));
            }
            test_validator.join();
        }
        Err(err) => {
            drop(dashboard);
            println!("Error: failed to start validator: {err}");
            exit(1);
        }
    }
}

fn remove_directory_contents(ledger_path: &Path) -> Result<(), io::Error> {
    for entry in fs::read_dir(ledger_path)? {
        let entry = entry?;
        if entry.metadata()?.is_dir() {
            fs::remove_dir_all(entry.path())?
        } else {
            fs::remove_file(entry.path())?
        }
    }
    Ok(())
}
