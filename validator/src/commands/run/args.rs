use {
    crate::{
        bootstrap::RpcBootstrapConfig,
        cli::{hash_validator, port_range_validator, port_validator, DefaultArgs},
        commands::{FromClapArgMatches, Result},
    },
    clap::{values_t, Command, Arg, ArgMatches, ArgAction},
    solana_clap_utils::{
        hidden_unless_forced,
        input_parsers::keypair_of,
        input_validators::{
            is_keypair_or_ask_keyword, is_non_zero, is_parsable, is_pow2, is_pubkey,
            is_pubkey_or_keypair, is_slot, is_within_range, validate_cpu_ranges,
            validate_maximum_full_snapshot_archives_to_retain,
            validate_maximum_incremental_snapshot_archives_to_retain,
        },
        keypair::SKIP_SEED_PHRASE_VALIDATION_ARG,
    },
    solana_core::{
        banking_trace::DirByteLimit,
        validator::{BlockProductionMethod, BlockVerificationMethod, TransactionStructure},
    },
    solana_keypair::Keypair,
    solana_ledger::use_snapshot_archives_at_startup,
    solana_pubkey::Pubkey,
    solana_runtime::snapshot_utils::{SnapshotVersion, SUPPORTED_ARCHIVE_COMPRESSION},
    solana_send_transaction_service::send_transaction_service::{
        MAX_BATCH_SEND_RATE_MS, MAX_TRANSACTION_BATCH_SIZE,
    },
    solana_signer::Signer,
    solana_unified_scheduler_pool::DefaultSchedulerPool,
    std::{collections::HashSet, net::SocketAddr, str::FromStr},
};

const EXCLUDE_KEY: &str = "account-index-exclude-key";
const INCLUDE_KEY: &str = "account-index-include-key";

pub mod rpc_bootstrap_config;

#[derive(Debug, PartialEq)]
pub struct RunArgs {
    pub identity_keypair: Keypair,
    pub logfile: String,
    pub entrypoints: Vec<SocketAddr>,
    pub known_validators: Option<HashSet<Pubkey>>,
    pub rpc_bootstrap_config: RpcBootstrapConfig,
}

impl FromClapArgMatches for RunArgs {
    fn from_clap_arg_match(matches: &ArgMatches) -> Result<Self> {
        let identity_keypair = {
            let identity_path = matches
                .get_one::<String>("identity")
                .ok_or_else(|| clap::Error::new(clap::ErrorKind::ArgumentNotFound))?;
            solana_sdk::signer::keypair::read_keypair_file(identity_path)
                .map_err(|err| clap::Error::new(clap::ErrorKind::InvalidValue))?
        };

        let logfile = matches
            .get_one::<String>("logfile")
            .map(|s| s.into())
            .unwrap_or_else(|| format!("agave-validator-{}.log", identity_keypair.pubkey()));

        let mut entrypoints = values_t!(matches, "entrypoint", String).unwrap_or_default();
        // sort() + dedup() to yield a vector of unique elements
        entrypoints.sort();
        entrypoints.dedup();
        let entrypoints = entrypoints
            .into_iter()
            .map(|entrypoint| {
                solana_net_utils::parse_host_port(&entrypoint).map_err(|err| {
                    crate::commands::Error::Dynamic(Box::<dyn std::error::Error>::from(format!(
                        "failed to parse entrypoint address: {err}"
                    )))
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let known_validators = validators_set(
            &identity_keypair.pubkey(),
            matches,
            "known_validators",
            "known validator",
        )?;

        Ok(RunArgs {
            identity_keypair,
            logfile,
            entrypoints,
            known_validators,
            rpc_bootstrap_config: RpcBootstrapConfig::from_clap_arg_match(matches)?,
        })
    }
}

pub fn add_args(app: Command, default_args: &DefaultArgs) -> Command {
    app
    .arg(
        Arg::new(SKIP_SEED_PHRASE_VALIDATION_ARG.name)
            .long(SKIP_SEED_PHRASE_VALIDATION_ARG.long)
            .help(SKIP_SEED_PHRASE_VALIDATION_ARG.help),
    )
    .arg(
        Arg::new("identity")
            .short("i")
            .long("identity")
            .value_name("KEYPAIR")
            
            .value_parser(clap::value_parser!(String))
            .help("Validator identity keypair"),
    )
    .arg(
        Arg::new("authorized_voter_keypairs")
            .long("authorized-voter")
            .value_name("KEYPAIR")
            
            .value_parser(clap::value_parser!(String))
            .requires("vote_account")
            .action(ArgAction::Append)
            .help(
                "Include an additional authorized voter keypair. May be specified multiple \
                 times. [default: the --identity keypair]",
            ),
    )
    .arg(
        Arg::new("vote_account")
            .long("vote-account")
            .value_name("ADDRESS")
            
            .value_parser(clap::value_parser!(String))
            .requires("identity")
            .help(
                "Validator vote account public key. If unspecified, voting will be disabled. \
                 The authorized voter for the account must either be the --identity keypair \
                 or set by the --authorized-voter argument",
            ),
    )
    .arg(
        Arg::new("init_complete_file")
            .long("init-complete-file")
            .value_name("FILE")
            
            .help(
                "Create this file if it doesn't already exist once validator initialization \
                 is complete",
            ),
    )
    .arg(
        Arg::new("ledger_path")
            .short("l")
            .long("ledger")
            .value_name("DIR")
            
            .required(true)
            .default_value(default_args.ledger_path.as_str())
            .help("Use DIR as ledger location"),
    )
    .arg(
        Arg::new("entrypoint")
            .short("n")
            .long("entrypoint")
            .value_name("HOST:PORT")
            
            .action(ArgAction::Append)
            .value_parser(clap::value_parser!(String))
            .help("Rendezvous with the cluster at this gossip entrypoint"),
    )
    .arg(
        Arg::new("no_snapshot_fetch")
            .long("no-snapshot-fetch")
            .action(ArgAction::SetTrue)
            .help(
                "Do not attempt to fetch a snapshot from the cluster, start from a local \
                 snapshot if present",
            ),
    )
    .arg(
        Arg::new("no_genesis_fetch")
            .long("no-genesis-fetch")
            .action(ArgAction::SetTrue)
            .help("Do not fetch genesis from the cluster"),
    )
    .arg(
        Arg::new("no_voting")
            .long("no-voting")
            .action(ArgAction::SetTrue)
            .help("Launch validator without voting"),
    )
    .arg(
        Arg::new("check_vote_account")
            .long("check-vote-account")
            
            .value_name("RPC_URL")
            .requires("entrypoint")
            .conflicts_with_all(&["no_voting"])
            .help(
                "Sanity check vote account state at startup. The JSON RPC endpoint at RPC_URL \
                 must expose `--full-rpc-api`",
            ),
    )
    .arg(
        Arg::new("restricted_repair_only_mode")
            .long("restricted-repair-only-mode")
            .action(ArgAction::SetTrue)
            .help(
                "Do not publish the Gossip, TPU, TVU or Repair Service ports. Doing so causes \
                 the node to operate in a limited capacity that reduces its exposure to the \
                 rest of the cluster. The --no-voting flag is implicit when this flag is \
                 enabled",
            ),
    )
    .arg(
        Arg::new("dev_halt_at_slot")
            .long("dev-halt-at-slot")
            .value_name("SLOT")
            .value_parser(clap::value_parser!(u64))
            
            .help("Halt the validator when it reaches the given slot"),
    )
    .arg(
        Arg::new("rpc_port")
            .long("rpc-port")
            .value_name("PORT")
            
            .value_parser(clap::value_parser!(u16))
            .help("Enable JSON RPC on this port, and the next port for the RPC websocket"),
    )
    .arg(
        Arg::new("full_rpc_api")
            .long("full-rpc-api")
            .action(ArgAction::SetTrue)
            .help("Expose RPC methods for querying chain state and transaction history"),
    )
    .arg(
        Arg::new("private_rpc")
            .long("private-rpc")
            .action(ArgAction::SetTrue)
            .help("Do not publish the RPC port for use by others"),
    )
    .arg(
        Arg::new("no_port_check")
            .long("no-port-check")
            .action(ArgAction::SetTrue)
            .hidden(hidden_unless_forced())
            .help("Do not perform TCP/UDP reachable port checks at start-up"),
    )
    .arg(
        Arg::new("enable_rpc_transaction_history")
            .long("enable-rpc-transaction-history")
            .action(ArgAction::SetTrue)
            .help(
                "Enable historical transaction info over JSON RPC, including the \
                 'getConfirmedBlock' API. This will cause an increase in disk usage and IOPS",
            ),
    )
    .arg(
        Arg::new("enable_rpc_bigtable_ledger_storage")
            .long("enable-rpc-bigtable-ledger-storage")
            .requires("enable_rpc_transaction_history")
            .action(ArgAction::SetTrue)
            .help(
                "Fetch historical transaction info from a BigTable instance as a fallback to \
                 local ledger data",
            ),
    )
    .arg(
        Arg::new("enable_bigtable_ledger_upload")
            .long("enable-bigtable-ledger-upload")
            .requires("enable_rpc_transaction_history")
            .action(ArgAction::SetTrue)
            .help("Upload new confirmed blocks into a BigTable instance"),
    )
    .arg(
        Arg::new("enable_extended_tx_metadata_storage")
            .long("enable-extended-tx-metadata-storage")
            .requires("enable_rpc_transaction_history")
            .action(ArgAction::SetTrue)
            .help(
                "Include CPI inner instructions, logs, and return data in the historical \
                 transaction info stored",
            ),
    )
    .arg(
        Arg::new("rpc_max_multiple_accounts")
            .long("rpc-max-multiple-accounts")
            .value_name("MAX ACCOUNTS")
            
            .default_value(default_args.rpc_max_multiple_accounts.as_str())
            .help(
                "Override the default maximum accounts accepted by the getMultipleAccounts \
                 JSON RPC method",
            ),
    )
    .arg(
        Arg::new("health_check_slot_distance")
            .long("health-check-slot-distance")
            .value_name("SLOT_DISTANCE")
            
            .default_value(default_args.health_check_slot_distance.as_str())
            .help(
                "Report this validator as healthy if its latest replayed optimistically \
                 confirmed slot is within the specified number of slots from the cluster's \
                 latest optimistically confirmed slot",
            ),
    )
    .arg(
        Arg::new("skip_preflight_health_check")
            .long("skip-preflight-health-check")
            .action(ArgAction::SetTrue)
            .help(
                "Skip health check when running a preflight check",
            ),
    )
    .arg(
        Arg::new("rpc_faucet_addr")
            .long("rpc-faucet-address")
            .value_name("HOST:PORT")
            
            .value_parser(clap::value_parser!(String))
            .help("Enable the JSON RPC 'requestAirdrop' API with this faucet address."),
    )
    .arg(
        Arg::new("account_paths")
            .long("accounts")
            .value_name("PATHS")
            
            .action(ArgAction::Append)
            .help(
                "Comma separated persistent accounts location. \
                May be specified multiple times. \
                [default: <LEDGER>/accounts]",
            ),
    )
    .arg(
        Arg::new("account_shrink_path")
            .long("account-shrink-path")
            .value_name("PATH")
            
            .action(ArgAction::Append)
            .help("Path to accounts shrink path which can hold a compacted account set."),
    )
    .arg(
        Arg::new("accounts_hash_cache_path")
            .long("accounts-hash-cache-path")
            .value_name("PATH")
            
            .help(
                "Use PATH as accounts hash cache location \
                 [default: <LEDGER>/accounts_hash_cache]",
            ),
    )
    .arg(
        Arg::new("snapshots")
            .long("snapshots")
            .value_name("DIR")
            
            .help("Use DIR as the base location for snapshots.")
            .long_help(
                "Use DIR as the base location for snapshots. \
                 Snapshot archives will use DIR unless --full-snapshot-archive-path or \
                 --incremental-snapshot-archive-path is specified. \
                 Additionally, a subdirectory named \"snapshots\" will be created in DIR. \
                 This subdirectory holds internal files/data that are used when generating \
                 snapshot archives. \
                 [default: --ledger value]",
             ),
    )
    .arg(
        Arg::new(use_snapshot_archives_at_startup::cli::NAME)
            .long(use_snapshot_archives_at_startup::cli::LONG_ARG)
            
            .possible_values(use_snapshot_archives_at_startup::cli::POSSIBLE_VALUES)
            .default_value(use_snapshot_archives_at_startup::cli::default_value())
            .help(use_snapshot_archives_at_startup::cli::HELP)
            .long_help(use_snapshot_archives_at_startup::cli::LONG_HELP),
    )
    .arg(
        Arg::new("full_snapshot_archive_path")
            .long("full-snapshot-archive-path")
            .value_name("DIR")
            
            .help(
                "Use DIR as full snapshot archives location \
                 [default: --snapshots value]",
             ),
    )
    .arg(
        Arg::new("incremental_snapshot_archive_path")
            .long("incremental-snapshot-archive-path")
            .conflicts_with("no-incremental-snapshots")
            .value_name("DIR")
            
            .help(
                "Use DIR as incremental snapshot archives location \
                 [default: --snapshots value]",
            ),
    )
    .arg(
        Arg::new("tower")
            .long("tower")
            .value_name("DIR")
            
            .help("Use DIR as file tower storage location [default: --ledger value]"),
    )
    .arg(
        Arg::new("gossip_port")
            .long("gossip-port")
            .value_name("PORT")
            
            .help("Gossip port number for the validator"),
    )
    .arg(
        Arg::new("gossip_host")
            .long("gossip-host")
            .value_name("HOST")
            
            .value_parser(clap::value_parser!(String))
            .hidden(hidden_unless_forced())
            .help("DEPRECATED: Use --bind-address instead."),
    )
    .arg(
        Arg::new("public_tpu_addr")
            .long("public-tpu-address")
            .alias("tpu-host-addr")
            .value_name("HOST:PORT")
            
            .value_parser(clap::value_parser!(String))
            .help(
                "Specify TPU address to advertise in gossip \
                 [default: ask --entrypoint or localhost when --entrypoint is not provided]",
            ),
    )
    .arg(
        Arg::new("public_tpu_forwards_addr")
            .long("public-tpu-forwards-address")
            .value_name("HOST:PORT")
            
            .value_parser(clap::value_parser!(String))
            .help(
                "Specify TPU Forwards address to advertise in gossip [default: ask \
                 --entrypoint or localhostwhen --entrypoint is not provided]",
            ),
    )
    .arg(
        Arg::new("tpu_vortexor_receiver_address")
            .long("tpu-vortexor-receiver-address")
            .value_name("HOST:PORT")
            
            .hidden(hidden_unless_forced())
            .value_parser(clap::value_parser!(String))
            .help("TPU Vortexor Receiver address to which verified transaction packet will be forwarded."),
    )
    .arg(
        Arg::new("public_rpc_addr")
            .long("public-rpc-address")
            .value_name("HOST:PORT")
            
            .conflicts_with("private_rpc")
            .value_parser(clap::value_parser!(String))
            .help(
                "RPC address for the validator to advertise publicly in gossip. Useful for \
                 validators running behind a load balancer or proxy [default: use \
                 --rpc-bind-address / --rpc-port]",
            ),
    )
    .arg(
        Arg::new("dynamic_port_range")
            .long("dynamic-port-range")
            .value_name("MIN_PORT-MAX_PORT")
            
            .default_value(default_args.dynamic_port_range.as_str())
            .value_parser(clap::value_parser!(String))
            .help("Range to use for dynamically assigned ports"),
    )
    .arg(
        Arg::new("maximum_local_snapshot_age")
            .long("maximum-local-snapshot-age")
            .value_name("NUMBER_OF_SLOTS")
            
            .default_value(default_args.maximum_local_snapshot_age.as_str())
            .help(
                "Reuse a local snapshot if it's less than this many slots behind the highest \
                 snapshot available for download from other validators",
            ),
    )
    .arg(
        Arg::new("no_snapshots")
            .long("no-snapshots")
            .action(ArgAction::SetTrue)
            .conflicts_with_all(&["no_incremental_snapshots", "snapshot_interval_slots", "full_snapshot_interval_slots"])
            .help("Disable all snapshot generation")
    )
    .arg(
        Arg::new("no_incremental_snapshots")
            .long("no-incremental-snapshots")
            .action(ArgAction::SetTrue)
            .help("Disable incremental snapshots")
    )
    .arg(
        Arg::new("snapshot_interval_slots")
            .long("snapshot-interval-slots")
            .alias("incremental-snapshot-interval-slots")
            .value_name("NUMBER")
            
            .default_value(default_args.incremental_snapshot_archive_interval_slots.as_str())
            .value_parser(clap::value_parser!(u64))
            .help("Number of slots between generating snapshots")
            .long_help(
                "Number of slots between generating snapshots. \
                 If incremental snapshots are enabled, this sets the incremental snapshot interval. \
                 If incremental snapshots are disabled, this sets the full snapshot interval. \
                 Must be greater than zero.",
            ),
    )
    .arg(
        Arg::new("full_snapshot_interval_slots")
            .long("full-snapshot-interval-slots")
            .value_name("NUMBER")
            
            .default_value(default_args.full_snapshot_archive_interval_slots.as_str())
            .value_parser(clap::value_parser!(u64))
            .help("Number of slots between generating full snapshots")
            .long_help(
                "Number of slots between generating full snapshots. \
                 Only used when incremental snapshots are enabled. \
                 Must be greater than the incremental snapshot interval. \
                 Must be greater than zero.",
            ),
    )
    .arg(
        Arg::new("maximum_full_snapshots_to_retain")
            .long("maximum-full-snapshots-to-retain")
            .alias("maximum-snapshots-to-retain")
            .value_name("NUMBER")
            
            .default_value(default_args.maximum_full_snapshot_archives_to_retain.as_str())
            .value_parser(clap::value_parser!(usize))
            .help(
                "The maximum number of full snapshot archives to hold on to when purging \
                 older snapshots.",
            ),
    )
    .arg(
        Arg::new("maximum_incremental_snapshots_to_retain")
            .long("maximum-incremental-snapshots-to-retain")
            .value_name("NUMBER")
            
            .default_value(default_args.maximum_incremental_snapshot_archives_to_retain.as_str())
            .value_parser(clap::value_parser!(usize))
            .help(
                "The maximum number of incremental snapshot archives to hold on to when \
                 purging older snapshots.",
            ),
    )
    .arg(
        Arg::new("snapshot_packager_niceness_adj")
            .long("snapshot-packager-niceness-adjustment")
            .value_name("ADJUSTMENT")
            
            .value_parser(clap::value_parser!(i8))
            .default_value(default_args.snapshot_packager_niceness_adjustment.as_str())
            .help(
                "Add this value to niceness of snapshot packager thread. Negative value \
                 increases priority, positive value decreases priority.",
            ),
    )
    .arg(
        Arg::new("minimal_snapshot_download_speed")
            .long("minimal-snapshot-download-speed")
            .value_name("MINIMAL_SNAPSHOT_DOWNLOAD_SPEED")
            
            .default_value(default_args.min_snapshot_download_speed.as_str())
            .help(
                "The minimal speed of snapshot downloads measured in bytes/second. If the \
                 initial download speed falls below this threshold, the system will retry the \
                 download against a different rpc node.",
            ),
    )
    .arg(
        Arg::new("maximum_snapshot_download_abort")
            .long("maximum-snapshot-download-abort")
            .value_name("MAXIMUM_SNAPSHOT_DOWNLOAD_ABORT")
            
            .default_value(default_args.max_snapshot_download_abort.as_str())
            .help(
                "The maximum number of times to abort and retry when encountering a slow \
                 snapshot download.",
            ),
    )
    .arg(
        Arg::new("contact_debug_interval")
            .long("contact-debug-interval")
            .value_name("CONTACT_DEBUG_INTERVAL")
            
            .default_value(default_args.contact_debug_interval.as_str())
            .help("Milliseconds between printing contact debug from gossip."),
    )
    .arg(
        Arg::new("no_poh_speed_test")
            .long("no-poh-speed-test")
            .hidden(hidden_unless_forced())
            .help("Skip the check for PoH speed."),
    )
    .arg(
        Arg::new("no_os_network_limits_test")
            .hidden(hidden_unless_forced())
            .long("no-os-network-limits-test")
            .help("Skip checks for OS network limits."),
    )
    .arg(
        Arg::new("no_os_memory_stats_reporting")
            .long("no-os-memory-stats-reporting")
            .hidden(hidden_unless_forced())
            .help("Disable reporting of OS memory statistics."),
    )
    .arg(
        Arg::new("no_os_network_stats_reporting")
            .long("no-os-network-stats-reporting")
            .hidden(hidden_unless_forced())
            .help("Disable reporting of OS network statistics."),
    )
    .arg(
        Arg::new("no_os_cpu_stats_reporting")
            .long("no-os-cpu-stats-reporting")
            .hidden(hidden_unless_forced())
            .help("Disable reporting of OS CPU statistics."),
    )
    .arg(
        Arg::new("no_os_disk_stats_reporting")
            .long("no-os-disk-stats-reporting")
            .hidden(hidden_unless_forced())
            .help("Disable reporting of OS disk statistics."),
    )
    .arg(
        Arg::new("snapshot_version")
            .long("snapshot-version")
            .value_name("SNAPSHOT_VERSION")
            .value_parser(clap::value_parser!(String))
            
            .default_value(default_args.snapshot_version.into())
            .help("Output snapshot version"),
    )
    .arg(
        Arg::new("limit_ledger_size")
            .long("limit-ledger-size")
            .value_name("SHRED_COUNT")
            
            .min_values(0)
            .max_values(1)
            /* .default_value() intentionally not used here! */
            .help("Keep this amount of shreds in root slots."),
    )
    .arg(
        Arg::new("rocksdb_shred_compaction")
            .long("rocksdb-shred-compaction")
            .value_name("ROCKSDB_COMPACTION_STYLE")
            
            .possible_values(&["level"])
            .default_value(default_args.rocksdb_shred_compaction.as_str())
            .help(
                "Controls how RocksDB compacts shreds. *WARNING*: You will lose your \
                 Blockstore data when you switch between options. Possible values are: \
                 'level': stores shreds using RocksDB's default (level) compaction.",
            ),
    )
    .arg(
        Arg::new("rocksdb_ledger_compression")
            .hidden(hidden_unless_forced())
            .long("rocksdb-ledger-compression")
            .value_name("COMPRESSION_TYPE")
            
            .possible_values(&["none", "lz4", "snappy", "zlib"])
            .default_value(default_args.rocksdb_ledger_compression.as_str())
            .help(
                "The compression algorithm that is used to compress transaction status data. \
                 Turning on compression can save ~10% of the ledger size.",
            ),
    )
    .arg(
        Arg::new("rocksdb_perf_sample_interval")
            .hidden(hidden_unless_forced())
            .long("rocksdb-perf-sample-interval")
            .value_name("ROCKS_PERF_SAMPLE_INTERVAL")
            
            .value_parser(clap::value_parser!(usize))
            .default_value(default_args.rocksdb_perf_sample_interval.as_str())
            .help(
                "Controls how often RocksDB read/write performance samples are collected. \
                 Perf samples are collected in 1 / ROCKS_PERF_SAMPLE_INTERVAL sampling rate.",
            ),
    )
    .arg(
        Arg::new("skip_startup_ledger_verification")
            .long("skip-startup-ledger-verification")
            .action(ArgAction::SetTrue)
            .help("Skip ledger verification at validator bootup."),
    )
    .arg(
        Arg::new("cuda")
            .long("cuda")
            .action(ArgAction::SetTrue)
            .help("Use CUDA"),
    )
    .arg(
        clap::Arg::new("require_tower")
            .long("require-tower")
            .action(ArgAction::SetTrue)
            .help("Refuse to start if saved tower state is not found"),
    )
    .arg(
        Arg::new("expected_genesis_hash")
            .long("expected-genesis-hash")
            .value_name("HASH")
            
            .value_parser(clap::value_parser!(String))
            .help("Require the genesis have this hash"),
    )
    .arg(
        Arg::new("expected_bank_hash")
            .long("expected-bank-hash")
            .value_name("HASH")
            
            .value_parser(clap::value_parser!(String))
            .help("When wait-for-supermajority <x>, require the bank at <x> to have this hash"),
    )
    .arg(
        Arg::new("expected_shred_version")
            .long("expected-shred-version")
            .value_name("VERSION")
            
            .value_parser(clap::value_parser!(u16))
            .help("Require the shred version be this value"),
    )
    .arg(
        Arg::new("logfile")
            .short("o")
            .long("log")
            .value_name("FILE")
            
            .help(
                "Redirect logging to the specified file, '-' for standard error. Sending the \
                 SIGUSR1 signal to the validator process will cause it to re-open the log file",
            ),
    )
    .arg(
        Arg::new("wait_for_supermajority")
            .long("wait-for-supermajority")
            .requires("expected_bank_hash")
            .requires("expected_shred_version")
            .value_name("SLOT")
            .value_parser(clap::value_parser!(u64))
            .help(
                "After processing the ledger and the next slot is SLOT, wait until a \
                 supermajority of stake is visible on gossip before starting PoH",
            ),
    )
    .arg(
        Arg::new("no_wait_for_vote_to_start_leader")
            .hidden(hidden_unless_forced())
            .long("no-wait-for-vote-to-start-leader")
            .help(
                "If the validator starts up with no ledger, it will wait to start block \
                 production until it sees a vote land in a rooted slot. This prevents \
                 double signing. Turn off to risk double signing a block.",
            ),
    )
    .arg(
        Arg::new("hard_forks")
            .long("hard-fork")
            .value_name("SLOT")
            .value_parser(clap::value_parser!(u64))
            .action(ArgAction::Append)
            
            .help("Add a hard fork at this slot"),
    )
    .arg(
        Arg::new("known_validators")
            .alias("trusted-validator")
            .long("known-validator")
            .value_parser(clap::value_parser!(String))
            .value_name("VALIDATOR IDENTITY")
            .action(ArgAction::Append)
            
            .help(
                "A snapshot hash must be published in gossip by this validator to be \
                 accepted. May be specified multiple times. If unspecified any snapshot hash \
                 will be accepted",
            ),
    )
    .arg(
        Arg::new("debug_key")
            .long("debug-key")
            .value_parser(clap::value_parser!(String))
            .value_name("ADDRESS")
            .action(ArgAction::Append)
            
            .help("Log when transactions are processed which reference a given key."),
    )
    .arg(
        Arg::new("only_known_rpc")
            .alias("no-untrusted-rpc")
            .long("only-known-rpc")
            .action(ArgAction::SetTrue)
            .requires("known_validators")
            .help("Use the RPC service of known validators only"),
    )
    .arg(
        Arg::new("repair_validators")
            .long("repair-validator")
            .value_parser(clap::value_parser!(String))
            .value_name("VALIDATOR IDENTITY")
            .action(ArgAction::Append)
            
            .help(
                "A list of validators to request repairs from. If specified, repair will not \
                 request from validators outside this set [default: all validators]",
            ),
    )
    .arg(
        Arg::new("repair_whitelist")
            .hidden(hidden_unless_forced())
            .long("repair-whitelist")
            .value_parser(clap::value_parser!(String))
            .value_name("VALIDATOR IDENTITY")
            .action(ArgAction::Append)
            
            .help(
                "A list of validators to prioritize repairs from. If specified, repair \
                 requests from validators in the list will be prioritized over requests from \
                 other validators. [default: all validators]",
            ),
    )
    .arg(
        Arg::new("gossip_validators")
            .long("gossip-validator")
            .value_parser(clap::value_parser!(String))
            .value_name("VALIDATOR IDENTITY")
            .action(ArgAction::Append)
            
            .help(
                "A list of validators to gossip with. If specified, gossip will not \
                 push/pull from from validators outside this set. [default: all validators]",
            ),
    )
    .arg(
        Arg::new("tpu_coalesce_ms")
            .long("tpu-coalesce-ms")
            .value_name("MILLISECS")
            
            .value_parser(clap::value_parser!(u64))
            .help("Milliseconds to wait in the TPU receiver for packet coalescing."),
    )
    .arg(
        Arg::new("tpu_disable_quic")
            .long("tpu-disable-quic")
            .action(ArgAction::SetTrue)
            .hidden(hidden_unless_forced())
            .help("DEPRECATED (UDP support will be dropped): Do not use QUIC to send transactions."),
    )
    .arg(
        Arg::new("tpu_enable_udp")
            .long("tpu-enable-udp")
            .action(ArgAction::SetTrue)
            .hidden(hidden_unless_forced())
            .help("DEPRECATED (UDP support will be dropped): Enable UDP for receiving/sending transactions."),
    )
    .arg(
        Arg::new("tpu_connection_pool_size")
            .long("tpu-connection-pool-size")
            
            .default_value(default_args.tpu_connection_pool_size.as_str())
            .value_parser(clap::value_parser!(usize))
            .help("Controls the TPU connection pool size per remote address"),
    )
    .arg(
        Arg::new("tpu_max_connections_per_ipaddr_per_minute")
            .long("tpu-max-connections-per-ipaddr-per-minute")
            
            .default_value(default_args.tpu_max_connections_per_ipaddr_per_minute.as_str())
            .value_parser(clap::value_parser!(u32))
            .hidden(hidden_unless_forced())
            .help("Controls the rate of the clients connections per IpAddr per minute."),
    )
    .arg(
        Arg::new("vote_use_quic")
            .long("vote-use-quic")
            
            .default_value(default_args.vote_use_quic.as_str())
            .hidden(hidden_unless_forced())
            .help("Controls if to use QUIC to send votes."),
    )
    .arg(
        Arg::new("tpu_max_connections_per_peer")
            .long("tpu-max-connections-per-peer")
            
            .default_value(default_args.tpu_max_connections_per_peer.as_str())
            .value_parser(clap::value_parser!(u32))
            .hidden(hidden_unless_forced())
            .help("Controls the max concurrent connections per IpAddr."),
    )
    .arg(
        Arg::new("tpu_max_staked_connections")
            .long("tpu-max-staked-connections")
            
            .default_value(default_args.tpu_max_staked_connections.as_str())
            .value_parser(clap::value_parser!(u32))
            .hidden(hidden_unless_forced())
            .help("Controls the max concurrent connections for TPU from staked nodes."),
    )
    .arg(
        Arg::new("tpu_max_unstaked_connections")
            .long("tpu-max-unstaked-connections")
            
            .default_value(default_args.tpu_max_unstaked_connections.as_str())
            .value_parser(clap::value_parser!(u32))
            .hidden(hidden_unless_forced())
            .help("Controls the max concurrent connections fort TPU from unstaked nodes."),
    )
    .arg(
        Arg::new("tpu_max_fwd_staked_connections")
            .long("tpu-max-fwd-staked-connections")
            
            .default_value(default_args.tpu_max_fwd_staked_connections.as_str())
            .value_parser(clap::value_parser!(u32))
            .hidden(hidden_unless_forced())
            .help("Controls the max concurrent connections for TPU-forward from staked nodes."),
    )
    .arg(
        Arg::new("tpu_max_fwd_unstaked_connections")
            .long("tpu-max-fwd-unstaked-connections")
            
            .default_value(default_args.tpu_max_fwd_unstaked_connections.as_str())
            .value_parser(clap::value_parser!(u32))
            .hidden(hidden_unless_forced())
            .help("Controls the max concurrent connections for TPU-forward from unstaked nodes."),
    )
    .arg(
        Arg::new("tpu_max_streams_per_ms")
            .long("tpu-max-streams-per-ms")
            
            .default_value(default_args.tpu_max_streams_per_ms.as_str())
            .value_parser(clap::value_parser!(usize))
            .hidden(hidden_unless_forced())
            .help("Controls the max number of streams for a TPU service."),
    )
    .arg(
        Arg::new("num_quic_endpoints")
            .long("num-quic-endpoints")
            
            .default_value(default_args.num_quic_endpoints.as_str())
            .value_parser(clap::value_parser!(usize))
            .hidden(hidden_unless_forced())
            .help("The number of QUIC endpoints used for TPU and TPU-Forward. It can be increased to \
                   increase network ingest throughput, at the expense of higher CPU and general \
                   validator load."),
    )
    .arg(
        Arg::new("staked_nodes_overrides")
            .long("staked-nodes-overrides")
            .value_name("PATH")
            
            .help(
                "Provide path to a yaml file with custom overrides for stakes of specific \
                 identities. Overriding the amount of stake this validator considers as valid \
                 for other peers in network. The stake amount is used for calculating the \
                 number of QUIC streams permitted from the peer and vote packet sender stage. \
                 Format of the file: `staked_map_id: {<pubkey>: <SOL stake amount>}",
            ),
    )
    .arg(
        Arg::new("bind_address")
            .long("bind-address")
            .value_name("HOST")
            
            .value_parser(clap::value_parser!(String))
            .default_value(default_args.bind_address.as_str())
            .action(ArgAction::Append)
            .help("Repeatable. IP addresses to bind the validator ports on. First is primary (used on startup), the rest may be switched to during operation."),
        )
    .arg(
        Arg::new("rpc_bind_address")
            .long("rpc-bind-address")
            .value_name("HOST")
            
            .value_parser(clap::value_parser!(String))
            .help(
                "IP address to bind the RPC port [default: 127.0.0.1 if --private-rpc is \
                 present, otherwise use --bind-address]",
            ),
    )
    .arg(
        Arg::new("rpc_threads")
            .long("rpc-threads")
            .value_name("NUMBER")
            .value_parser(clap::value_parser!(usize))
            
            .default_value(default_args.rpc_threads.as_str())
            .help("Number of threads to use for servicing RPC requests"),
    )
    .arg(
        Arg::new("rpc_blocking_threads")
            .long("rpc-blocking-threads")
            .value_name("NUMBER")
            .value_parser(clap::value_parser!(usize))
            
            .default_value(default_args.rpc_blocking_threads.as_str())
            .help("Number of blocking threads to use for servicing CPU bound RPC requests (eg getMultipleAccounts)"),
    )
    .arg(
        Arg::new("rpc_niceness_adj")
            .long("rpc-niceness-adjustment")
            .value_name("ADJUSTMENT")
            
            .value_parser(clap::value_parser!(i8))
            .default_value(default_args.rpc_niceness_adjustment.as_str())
            .help(
                "Add this value to niceness of RPC threads. Negative value increases \
                 priority, positive value decreases priority.",
            ),
    )
    .arg(
        Arg::new("rpc_bigtable_timeout")
            .long("rpc-bigtable-timeout")
            .value_name("SECONDS")
            .value_parser(clap::value_parser!(u64))
            
            .default_value(default_args.rpc_bigtable_timeout.as_str())
            .help("Number of seconds before timing out RPC requests backed by BigTable"),
    )
    .arg(
        Arg::new("rpc_bigtable_instance_name")
            .long("rpc-bigtable-instance-name")
            
            .value_name("INSTANCE_NAME")
            .default_value(default_args.rpc_bigtable_instance_name.as_str())
            .help("Name of the Bigtable instance to upload to"),
    )
    .arg(
        Arg::new("rpc_bigtable_app_profile_id")
            .long("rpc-bigtable-app-profile-id")
            
            .value_name("APP_PROFILE_ID")
            .default_value(default_args.rpc_bigtable_app_profile_id.as_str())
            .help("Bigtable application profile id to use in requests"),
    )
    .arg(
        Arg::new("rpc_bigtable_max_message_size")
            .long("rpc-bigtable-max-message-size")
            .value_name("BYTES")
            .value_parser(clap::value_parser!(usize))
            
            .default_value(default_args.rpc_bigtable_max_message_size.as_str())
            .help("Max encoding and decoding message size used in Bigtable Grpc client"),
    )
    .arg(
        Arg::new("rpc_pubsub_worker_threads")
            .long("rpc-pubsub-worker-threads")
            
            .value_name("NUMBER")
            .value_parser(clap::value_parser!(usize))
            .default_value(default_args.rpc_pubsub_worker_threads.as_str())
            .help("PubSub worker threads"),
    )
    .arg(
        Arg::new("rpc_pubsub_enable_block_subscription")
            .long("rpc-pubsub-enable-block-subscription")
            .requires("enable_rpc_transaction_history")
            .action(ArgAction::SetTrue)
            .help("Enable the unstable RPC PubSub `blockSubscribe` subscription"),
    )
    .arg(
        Arg::new("rpc_pubsub_enable_vote_subscription")
            .long("rpc-pubsub-enable-vote-subscription")
            .action(ArgAction::SetTrue)
            .help("Enable the unstable RPC PubSub `voteSubscribe` subscription"),
    )
    .arg(
        Arg::new("rpc_pubsub_max_active_subscriptions")
            .long("rpc-pubsub-max-active-subscriptions")
            
            .value_name("NUMBER")
            .value_parser(clap::value_parser!(usize))
            .default_value(default_args.rpc_pubsub_max_active_subscriptions.as_str())
            .help(
                "The maximum number of active subscriptions that RPC PubSub will accept \
                 across all connections.",
            ),
    )
    .arg(
        Arg::new("rpc_pubsub_queue_capacity_items")
            .long("rpc-pubsub-queue-capacity-items")
            
            .value_name("NUMBER")
            .value_parser(clap::value_parser!(usize))
            .default_value(default_args.rpc_pubsub_queue_capacity_items.as_str())
            .help(
                "The maximum number of notifications that RPC PubSub will store across all \
                 connections.",
            ),
    )
    .arg(
        Arg::new("rpc_pubsub_queue_capacity_bytes")
            .long("rpc-pubsub-queue-capacity-bytes")
            
            .value_name("BYTES")
            .value_parser(clap::value_parser!(usize))
            .default_value(default_args.rpc_pubsub_queue_capacity_bytes.as_str())
            .help(
                "The maximum total size of notifications that RPC PubSub will store across \
                 all connections.",
            ),
    )
    .arg(
        Arg::new("rpc_pubsub_notification_threads")
            .long("rpc-pubsub-notification-threads")
            .requires("full_rpc_api")
            
            .value_name("NUM_THREADS")
            .value_parser(clap::value_parser!(usize))
            .default_value_if(
                "full_rpc_api",
                None,
                &default_args.rpc_pubsub_notification_threads,
            )
            .help(
                "The maximum number of threads that RPC PubSub will use for generating \
                 notifications. 0 will disable RPC PubSub notifications",
            ),
    )
    .arg(
        Arg::new("rpc_send_transaction_retry_ms")
            .long("rpc-send-retry-ms")
            .value_name("MILLISECS")
            
            .value_parser(clap::value_parser!(u64))
            .default_value(default_args.rpc_send_transaction_retry_ms.as_str())
            .help("The rate at which transactions sent via rpc service are retried."),
    )
    .arg(
        Arg::new("rpc_send_transaction_batch_ms")
            .long("rpc-send-batch-ms")
            .value_name("MILLISECS")
            .hidden(hidden_unless_forced())
            
            .value_parser(clap::value_parser!(u64))
            .default_value(default_args.rpc_send_transaction_batch_ms.as_str())
            .help("The rate at which transactions sent via rpc service are sent in batch."),
    )
    .arg(
        Arg::new("rpc_send_transaction_leader_forward_count")
            .long("rpc-send-leader-count")
            .value_name("NUMBER")
            
            .value_parser(clap::value_parser!(u64))
            .default_value(default_args.rpc_send_transaction_leader_forward_count.as_str())
            .help(
                "The number of upcoming leaders to which to forward transactions sent via rpc \
                 service.",
            ),
    )
    .arg(
        Arg::new("rpc_send_transaction_default_max_retries")
            .long("rpc-send-default-max-retries")
            .value_name("NUMBER")
            
            .value_parser(clap::value_parser!(usize))
            .help(
                "The maximum number of transaction broadcast retries when unspecified by the \
                 request, otherwise retried until expiration.",
            ),
    )
    .arg(
        Arg::new("rpc_send_transaction_service_max_retries")
            .long("rpc-send-service-max-retries")
            .value_name("NUMBER")
            
            .value_parser(clap::value_parser!(usize))
            .default_value(default_args.rpc_send_transaction_service_max_retries.as_str())
            .help(
                "The maximum number of transaction broadcast retries, regardless of requested \
                 value.",
            ),
    )
    .arg(
        Arg::new("rpc_send_transaction_batch_size")
            .long("rpc-send-batch-size")
            .value_name("NUMBER")
            .hidden(hidden_unless_forced())
            
            .value_parser(clap::value_parser!(usize))
            .default_value(default_args.rpc_send_transaction_batch_size.as_str())
            .help("The size of transactions to be sent in batch."),
    )
    .arg(
        Arg::new("rpc_send_transaction_retry_pool_max_size")
            .long("rpc-send-transaction-retry-pool-max-size")
            .value_name("NUMBER")
            
            .value_parser(clap::value_parser!(usize))
            .default_value(default_args.rpc_send_transaction_retry_pool_max_size.as_str())
            .help("The maximum size of transactions retry pool."),
    )
    .arg(
        Arg::new("rpc_send_transaction_tpu_peer")
            .long("rpc-send-transaction-tpu-peer")
            
            .number_of_values(1)
            .action(ArgAction::Append)
            .value_name("HOST:PORT")
            .value_parser(clap::value_parser!(String))
            .help("Peer(s) to broadcast transactions to instead of the current leader")
    )
    .arg(
        Arg::new("rpc_send_transaction_also_leader")
            .long("rpc-send-transaction-also-leader")
            .requires("rpc_send_transaction_tpu_peer")
            .help("With `--rpc-send-transaction-tpu-peer HOST:PORT`, also send to the current leader")
    )
    .arg(
        Arg::new("rpc_scan_and_fix_roots")
            .long("rpc-scan-and-fix-roots")
            .action(ArgAction::SetTrue)
            .requires("enable_rpc_transaction_history")
            .help("Verifies blockstore roots on boot and fixes any gaps"),
    )
    .arg(
        Arg::new("rpc_max_request_body_size")
            .long("rpc-max-request-body-size")
            .value_name("BYTES")
            
            .value_parser(clap::value_parser!(usize))
            .default_value(default_args.rpc_max_request_body_size.as_str())
            .help("The maximum request body size accepted by rpc service"),
    )
    .arg(
        Arg::new("geyser_plugin_config")
            .long("geyser-plugin-config")
            .alias("accountsdb-plugin-config")
            .value_name("FILE")
            
            .action(ArgAction::Append)
            .help("Specify the configuration file for the Geyser plugin."),
    )
    .arg(
        Arg::new("geyser_plugin_always_enabled")
            .long("geyser-plugin-always-enabled")
            .value_name("BOOLEAN")
            .action(ArgAction::SetTrue)
            .help("Еnable Geyser interface even if no Geyser configs are specified."),
    )
    .arg(
        Arg::new("snapshot_archive_format")
            .long("snapshot-archive-format")
            .alias("snapshot-compression") // Legacy name used by Solana v1.5.x and older
            .possible_values(SUPPORTED_ARCHIVE_COMPRESSION)
            .default_value(default_args.snapshot_archive_format.as_str())
            .value_name("ARCHIVE_TYPE")
            
            .help("Snapshot archive format to use."),
    )
    .arg(
        Arg::new("snapshot_zstd_compression_level")
            .long("snapshot-zstd-compression-level")
            .default_value(default_args.snapshot_zstd_compression_level.as_str())
            .value_name("LEVEL")
            
            .help("The compression level to use when archiving with zstd")
            .long_help(
                "The compression level to use when archiving with zstd. \
                 Higher compression levels generally produce higher \
                 compression ratio at the expense of speed and memory. \
                 See the zstd manpage for more information."
            ),
    )
    .arg(
        Arg::new("max_genesis_archive_unpacked_size")
            .long("max-genesis-archive-unpacked-size")
            .value_name("NUMBER")
            
            .default_value(default_args.genesis_archive_unpacked_size.as_str())
            .help("maximum total uncompressed file size of downloaded genesis archive"),
    )
    .arg(
        Arg::new("wal_recovery_mode")
            .long("wal-recovery-mode")
            .value_name("MODE")
            
            .possible_values(&[
                "tolerate_corrupted_tail_records",
                "absolute_consistency",
                "point_in_time",
                "skip_any_corrupted_record",
            ])
            .help("Mode to recovery the ledger db write ahead log."),
    )
    .arg(
        Arg::new("poh_pinned_cpu_core")
            .hidden(hidden_unless_forced())
            .long("experimental-poh-pinned-cpu-core")
            .value_name("CPU_CORE_INDEX")
            .value_parser(clap::value_parser!(usize))
            .help("EXPERIMENTAL: Specify which CPU core PoH is pinned to"),
    )
    .arg(
        Arg::new("poh_hashes_per_batch")
            .hidden(hidden_unless_forced())
            .long("poh-hashes-per-batch")
            
            .value_name("NUM")
            .help("Specify hashes per batch in PoH service"),
    )
    .arg(
        Arg::new("process_ledger_before_services")
            .long("process-ledger-before-services")
            .hidden(hidden_unless_forced())
            .help("Process the local ledger fully before starting networking services"),
    )
    .arg(
        Arg::new("account_indexes")
            .long("account-index")
            
            .action(ArgAction::Append)
            .possible_values(&["program-id", "spl-token-owner", "spl-token-mint"])
            .value_name("INDEX")
            .help("Enable an accounts index, indexed by the selected account field"),
    )
    .arg(
        Arg::new("account_index_exclude_key")
            .long(EXCLUDE_KEY)
            
            .value_parser(clap::value_parser!(String))
            .action(ArgAction::Append)
            .value_name("KEY")
            .help("When account indexes are enabled, exclude this key from the index."),
    )
    .arg(
        Arg::new("account_index_include_key")
            .long(INCLUDE_KEY)
            
            .value_parser(clap::value_parser!(String))
            .conflicts_with("account_index_exclude_key")
            .action(ArgAction::Append)
            .value_name("KEY")
            .help(
                "When account indexes are enabled, only include specific keys in the index. \
                 This overrides --account-index-exclude-key.",
            ),
    )
    .arg(
        Arg::new("accounts_db_verify_refcounts")
            .long("accounts-db-verify-refcounts")
            .help(
                "Debug option to scan all append vecs and verify account index refcounts \
                 prior to clean",
            )
            .hidden(hidden_unless_forced()),
    )
    .arg(
        Arg::new("accounts_db_scan_filter_for_shrinking")
            .long("accounts-db-scan-filter-for-shrinking")
            
            .possible_values(&["all", "only-abnormal", "only-abnormal-with-verify"])
            .help(
                "Debug option to use different type of filtering for accounts index scan in \
                shrinking. \"all\" will scan both in-memory and on-disk accounts index, which is the default. \
                \"only-abnormal\" will scan in-memory accounts index only for abnormal entries and \
                skip scanning on-disk accounts index by assuming that on-disk accounts index contains \
                only normal accounts index entry. \"only-abnormal-with-verify\" is similar to \
                \"only-abnormal\", which will scan in-memory index for abnormal entries, but will also \
                verify that on-disk account entries are indeed normal.",
            )
            .hidden(hidden_unless_forced()),
    )
    .arg(
        Arg::new("no_skip_initial_accounts_db_clean")
            .long("no-skip-initial-accounts-db-clean")
            .help("Do not skip the initial cleaning of accounts when verifying snapshot bank")
            .hidden(hidden_unless_forced()),
    )
    .arg(
        Arg::new("accounts_db_access_storages_method")
            .long("accounts-db-access-storages-method")
            .value_name("METHOD")
            
            .possible_values(&["mmap", "file"])
            .help("Access account storages using this method")
    )
    .arg(
        Arg::new("accounts_db_ancient_append_vecs")
            .long("accounts-db-ancient-append-vecs")
            .value_name("SLOT-OFFSET")
            .value_parser(clap::value_parser!(i64))
            
            .help(
                "AppendVecs that are older than (slots_per_epoch - SLOT-OFFSET) are squashed \
                 together.",
            )
            .hidden(hidden_unless_forced()),
    )
    .arg(
        Arg::new("accounts_db_ancient_storage_ideal_size")
            .long("accounts-db-ancient-storage-ideal-size")
            .value_name("BYTES")
            .value_parser(clap::value_parser!(u64))
            
            .help("The smallest size of ideal ancient storage.")
            .hidden(hidden_unless_forced()),
    )
    .arg(
        Arg::new("accounts_db_max_ancient_storages")
            .long("accounts-db-max-ancient-storages")
            .value_name("USIZE")
            .value_parser(clap::value_parser!(usize))
            
            .help("The number of ancient storages the ancient slot combining should converge to.")
            .hidden(hidden_unless_forced()),
    )
    .arg(
        Arg::new("accounts_db_hash_calculation_pubkey_bins")
            .long("accounts-db-hash-calculation-pubkey-bins")
            .value_name("USIZE")
            .value_parser(clap::value_parser!(usize))
            
            .help("The number of pubkey bins used for accounts hash calculation.")
            .hidden(hidden_unless_forced()),
    )
    .arg(
        Arg::new("accounts_db_cache_limit_mb")
            .long("accounts-db-cache-limit-mb")
            .value_name("MEGABYTES")
            .value_parser(clap::value_parser!(u64))
            
            .help(
                "How large the write cache for account data can become. If this is exceeded, \
                 the cache is flushed more aggressively.",
            ),
    )
    .arg(
        Arg::new("accounts_db_read_cache_limit_mb")
            .long("accounts-db-read-cache-limit-mb")
            .value_name("MAX | LOW,HIGH")
            
            .min_values(1)
            .max_values(2)
            .multiple(false)
            .require_delimiter(true)
            .help("How large the read cache for account data can become, in mebibytes")
            .long_help(
                "How large the read cache for account data can become, in mebibytes. \
                 If given a single value, it will be the maximum size for the cache. \
                 If given a pair of values, they will be the low and high watermarks \
                 for the cache. When the cache exceeds the high watermark, entries will \
                 be evicted until the size reaches the low watermark."
            )
            .hidden(hidden_unless_forced()),
    )
    .arg(
        Arg::new("accounts_db_snapshots_use_experimental_accumulator_hash")
            .long("accounts-db-snapshots-use-experimental-accumulator-hash")
            .help("Snapshots use the experimental accumulator hash")
            .hidden(hidden_unless_forced()),
    )
    .arg(
        Arg::new("accounts_index_scan_results_limit_mb")
            .long("accounts-index-scan-results-limit-mb")
            .value_name("MEGABYTES")
            .value_parser(clap::value_parser!(usize))
            
            .help(
                "How large accumulated results from an accounts index scan can become. If \
                 this is exceeded, the scan aborts.",
            ),
    )
    .arg(
        Arg::new("accounts_index_bins")
            .long("accounts-index-bins")
            .value_name("BINS")
            .value_parser(clap::value_parser!(usize))
            
            .help("Number of bins to divide the accounts index into"),
    )
    .arg(
        Arg::new("accounts_index_path")
            .long("accounts-index-path")
            .value_name("PATH")
            
            .action(ArgAction::Append)
            .help(
                "Persistent accounts-index location. \
                May be specified multiple times. \
                [default: <LEDGER>/accounts_index]",
            ),
    )
    .arg(
        Arg::new("accounts_shrink_optimize_total_space")
            .long("accounts-shrink-optimize-total-space")
            
            .value_name("BOOLEAN")
            .default_value(default_args.accounts_shrink_optimize_total_space.as_str())
            .help(
                "When this is set to true, the system will shrink the most sparse accounts \
                 and when the overall shrink ratio is above the specified \
                 accounts-shrink-ratio, the shrink will stop and it will skip all other less \
                 sparse accounts.",
            ),
    )
    .arg(
        Arg::new("accounts_shrink_ratio")
            .long("accounts-shrink-ratio")
            
            .value_name("RATIO")
            .default_value(default_args.accounts_shrink_ratio.as_str())
            .help(
                "Specifies the shrink ratio for the accounts to be shrunk. The shrink ratio \
                 is defined as the ratio of the bytes alive over the  total bytes used. If \
                 the account's shrink ratio is less than this ratio it becomes a candidate \
                 for shrinking. The value must between 0. and 1.0 inclusive.",
            ),
    )
    .arg(
        Arg::new("allow_private_addr")
            .long("allow-private-addr")
            .action(ArgAction::SetTrue)
            .help("Allow contacting private ip addresses")
            .hidden(hidden_unless_forced()),
    )
    .arg(
        Arg::new("log_messages_bytes_limit")
            .long("log-messages-bytes-limit")
            
            .value_parser(clap::value_parser!(usize))
            .value_name("BYTES")
            .help("Maximum number of bytes written to the program log before truncation"),
    )
    .arg(
        Arg::new("banking_trace_dir_byte_limit")
            // expose friendly alternative name to cli than internal
            // implementation-oriented one
            .long("enable-banking-trace")
            .value_name("BYTES")
            .value_parser(clap::value_parser!(u64))
            
            // Firstly, zero limit value causes tracer to be disabled
            // altogether, intuitively. On the other hand, this non-zero
            // default doesn't enable banking tracer unless this flag is
            // explicitly given, similar to --limit-ledger-size.
            // see configure_banking_trace_dir_byte_limit() for this.
            .default_value(default_args.banking_trace_dir_byte_limit.as_str())
            .help(
                "Enables the banking trace explicitly, which is enabled by default and writes \
                 trace files for simulate-leader-blocks, retaining up to the default or \
                 specified total bytes in the ledger. This flag can be used to override its \
                 byte limit.",
            ),
    )
    .arg(
        Arg::new("disable_banking_trace")
            .long("disable-banking-trace")
            .conflicts_with("banking_trace_dir_byte_limit")
            .action(ArgAction::SetTrue)
            .help("Disables the banking trace"),
    )
    .arg(
        Arg::new("delay_leader_block_for_pending_fork")
            .hidden(hidden_unless_forced())
            .long("delay-leader-block-for-pending-fork")
            .action(ArgAction::SetTrue)
            .help(
                "Delay leader block creation while replaying a block which descends from the \
                current fork and has a lower slot than our next leader slot. If we don't \
                delay here, our new leader block will be on a different fork from the \
                block we are replaying and there is a high chance that the cluster will \
                confirm that block's fork rather than our leader block's fork because it \
                was created before we started creating ours.",
            ),
    )
    .arg(
        Arg::new("block_verification_method")
            .long("block-verification-method")
            .value_name("METHOD")
            
            .possible_values(BlockVerificationMethod::cli_names())
            .default_value(BlockVerificationMethod::default().into())
            .help(BlockVerificationMethod::cli_message()),
    )
    .arg(
        Arg::new("block_production_method")
            .long("block-production-method")
            .value_name("METHOD")
            
            .possible_values(BlockProductionMethod::cli_names())
            .default_value(BlockProductionMethod::default().into())
            .help(BlockProductionMethod::cli_message()),
    )
    .arg(
        Arg::new("transaction_struct")
            .long("transaction-structure")
            .value_name("STRUCT")
            
            .possible_values(TransactionStructure::cli_names())
            .default_value(TransactionStructure::default().into())
            .help(TransactionStructure::cli_message()),
    )
    .arg(
        Arg::new("unified_scheduler_handler_threads")
            .long("unified-scheduler-handler-threads")
            .value_name("COUNT")
            
            .value_parser(clap::value_parser!(usize))
            .help(DefaultSchedulerPool::cli_message()),
    )
    .arg(
        Arg::new("wen_restart")
            .long("wen-restart")
            .hidden(hidden_unless_forced())
            .value_name("FILE")
            
            .required(false)
            .conflicts_with("wait_for_supermajority")
            .requires("wen_restart_coordinator")
            .help(
                "Only used during coordinated cluster restarts.\
                \n\n\
                Need to also specify the leader's pubkey in --wen-restart-leader.\
                \n\n\
                When specified, the validator will enter Wen Restart mode which \
                pauses normal activity. Validators in this mode will gossip their last \
                vote to reach consensus on a safe restart slot and repair all blocks \
                on the selected fork. The safe slot will be a descendant of the latest \
                optimistically confirmed slot to ensure we do not roll back any \
                optimistically confirmed slots. \
                \n\n\
                The progress in this mode will be saved in the file location provided. \
                If consensus is reached, the validator will automatically exit with 200 \
                status code. Then the operators are expected to restart the validator \
                with --wait_for_supermajority and other arguments (including new shred_version, \
                supermajority slot, and bankhash) given in the error log before the exit so \
                the cluster will resume execution. The progress file will be kept around \
                for future debugging. \
                \n\n\
                If wen_restart fails, refer to the progress file (in proto3 format) for \
                further debugging and watch the discord channel for instructions.",
            ),
    )
    .arg(
        Arg::new("wen_restart_coordinator")
            .long("wen-restart-coordinator")
            .hidden(hidden_unless_forced())
            .value_name("PUBKEY")
            
            .required(false)
            .requires("wen_restart")
            .help(
                "Specifies the pubkey of the leader used in wen restart. \
                May get stuck if the leader used is different from others.",
            ),
    )
    .arg(
        Arg::new("retransmit_xdp_interface")
            .hidden(hidden_unless_forced())
            .long("experimental-retransmit-xdp-interface")
            
            .value_name("INTERFACE")
            .requires("retransmit_xdp_cpu_cores")
            .help("EXPERIMENTAL: The network interface to use for XDP retransmit"),
    )
    .arg(
        Arg::new("retransmit_xdp_cpu_cores")
            .hidden(hidden_unless_forced())
            .long("experimental-retransmit-xdp-cpu-cores")
            
            .value_name("CPU_LIST")
            .value_parser(clap::value_parser!(String))
            .help("EXPERIMENTAL: Enable XDP retransmit on the specified CPU cores"),
    )
    .arg(
        Arg::new("retransmit_xdp_zero_copy")
            .hide(hidden_unless_forced())
            .long("experimental-retransmit-xdp-zero-copy")
            .action(ArgAction::SetTrue)
            .requires("retransmit_xdp_cpu_cores")
            .help("EXPERIMENTAL: Enable XDP zero copy. Requires hardware support"),
    )
    .arg(
        Arg::new("use_connection_cache")
            .long("use-connection-cache")
            .action(ArgAction::SetTrue)
            .help(
                "Use connection-cache crate to send transactions over TPU ports. If not set,\
                tpu-client-next is used by default.",
            ),
    )
}

fn validators_set(
    identity_pubkey: &Pubkey,
    matches: &ArgMatches<'_>,
    matches_name: &str,
    arg_name: &str,
) -> Result<Option<HashSet<Pubkey>>> {
    if matches.get_flag(matches_name) {
        let validators_set: Option<HashSet<Pubkey>> = values_t!(matches, matches_name, Pubkey)
            .ok()
            .map(|validators| validators.into_iter().collect());
        if let Some(validators_set) = &validators_set {
            if validators_set.contains(identity_pubkey) {
                return Err(crate::commands::Error::Dynamic(
                    Box::<dyn std::error::Error>::from(format!(
                        "the validator's identity pubkey cannot be a {arg_name}: {}",
                        identity_pubkey
                    )),
                ));
            }
        }
        Ok(validators_set)
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        std::net::{IpAddr, Ipv4Addr},
    };

    impl Default for RunArgs {
        fn default() -> Self {
            let identity_keypair = Keypair::new();
            let logfile = format!("agave-validator-{}.log", identity_keypair.pubkey());
            let entrypoints = vec![];
            let known_validators = None;

            RunArgs {
                identity_keypair,
                logfile,
                entrypoints,
                known_validators,
                rpc_bootstrap_config: RpcBootstrapConfig::default(),
            }
        }
    }

    impl Clone for RunArgs {
        fn clone(&self) -> Self {
            RunArgs {
                identity_keypair: self.identity_keypair.insecure_clone(),
                logfile: self.logfile.clone(),
                entrypoints: self.entrypoints.clone(),
                known_validators: self.known_validators.clone(),
                rpc_bootstrap_config: self.rpc_bootstrap_config.clone(),
            }
        }
    }

    fn verify_args_struct_by_command(
        default_args: &DefaultArgs,
        args: Vec<&str>,
        expected_args: RunArgs,
    ) {
        crate::commands::tests::verify_args_struct_by_command::<RunArgs>(
            add_args(Command::new("run_command"), default_args),
            [&["run_command"], &args[..]].concat(),
            expected_args,
        );
    }

    #[test]
    fn verify_args_struct_by_command_run_with_identity() {
        let default_args = DefaultArgs::default();
        let default_run_args = RunArgs::default();

        // generate a keypair
        let tmp_dir = tempfile::tempdir().unwrap();
        let file = tmp_dir.path().join("id.json");
        let keypair = default_run_args.identity_keypair.insecure_clone();
        solana_keypair::write_keypair_file(&keypair, &file).unwrap();

        let expected_args = RunArgs {
            identity_keypair: keypair.insecure_clone(),
            ..default_run_args
        };

        // short arg
        {
            verify_args_struct_by_command(
                &default_args,
                vec!["-i", file.to_str().unwrap()],
                expected_args.clone(),
            );
        }

        // long arg
        {
            verify_args_struct_by_command(
                &default_args,
                vec!["--identity", file.to_str().unwrap()],
                expected_args.clone(),
            );
        }
    }

    fn verify_args_struct_by_command_run_with_identity_setup(
        default_run_args: RunArgs,
        args: Vec<&str>,
        expected_args: RunArgs,
    ) {
        let default_args = DefaultArgs::default();

        // generate a keypair
        let tmp_dir = tempfile::tempdir().unwrap();
        let file = tmp_dir.path().join("id.json");
        let keypair = default_run_args.identity_keypair.insecure_clone();
        solana_keypair::write_keypair_file(&keypair, &file).unwrap();

        let args = [&["--identity", file.to_str().unwrap()], &args[..]].concat();
        verify_args_struct_by_command(&default_args, args, expected_args);
    }

    #[test]
    fn verify_args_struct_by_command_run_with_log() {
        let default_run_args = RunArgs::default();

        // default
        {
            let expected_args = RunArgs {
                logfile: "agave-validator-".to_string()
                    + &default_run_args.identity_keypair.pubkey().to_string()
                    + ".log",
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args.clone(),
                vec![],
                expected_args,
            );
        }

        // short arg
        {
            let expected_args = RunArgs {
                logfile: "-".to_string(),
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args.clone(),
                vec!["-o", "-"],
                expected_args,
            );
        }

        // long arg
        {
            let expected_args = RunArgs {
                logfile: "custom_log.log".to_string(),
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args.clone(),
                vec!["--log", "custom_log.log"],
                expected_args,
            );
        }
    }

    #[test]
    fn verify_args_struct_by_command_run_with_no_genesis_fetch() {
        // long arg
        {
            let default_run_args = RunArgs::default();
            let expected_args = RunArgs {
                rpc_bootstrap_config: RpcBootstrapConfig {
                    no_genesis_fetch: true,
                    ..RpcBootstrapConfig::default()
                },
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args.clone(),
                vec!["--no-genesis-fetch"],
                expected_args,
            );
        }
    }

    #[test]
    fn verify_args_struct_by_command_run_with_no_snapshot_fetch() {
        // long arg
        {
            let default_run_args = RunArgs::default();
            let expected_args = RunArgs {
                rpc_bootstrap_config: RpcBootstrapConfig {
                    no_snapshot_fetch: true,
                    ..RpcBootstrapConfig::default()
                },
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args.clone(),
                vec!["--no-snapshot-fetch"],
                expected_args,
            );
        }
    }

    #[test]
    fn verify_args_struct_by_command_run_with_entrypoints() {
        // short arg + single entrypoint
        {
            let default_run_args = RunArgs::default();
            let expected_args = RunArgs {
                entrypoints: vec![SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                    8000,
                )],
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args.clone(),
                vec!["-n", "127.0.0.1:8000"],
                expected_args,
            );
        }

        // long arg + single entrypoint
        {
            let default_run_args = RunArgs::default();
            let expected_args = RunArgs {
                entrypoints: vec![SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                    8000,
                )],
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args.clone(),
                vec!["--entrypoint", "127.0.0.1:8000"],
                expected_args,
            );
        }

        // long arg + multiple entrypoints
        {
            let default_run_args = RunArgs::default();
            let expected_args = RunArgs {
                entrypoints: vec![
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8000),
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8001),
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8002),
                ],
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args.clone(),
                vec![
                    "--entrypoint",
                    "127.0.0.1:8000",
                    "--entrypoint",
                    "127.0.0.1:8001",
                    "--entrypoint",
                    "127.0.0.1:8002",
                ],
                expected_args,
            );
        }

        // long arg + duplicate entrypoints
        {
            let default_run_args = RunArgs::default();
            let expected_args = RunArgs {
                entrypoints: vec![
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8000),
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8001),
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8002),
                ],
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args.clone(),
                vec![
                    "--entrypoint",
                    "127.0.0.1:8000",
                    "--entrypoint",
                    "127.0.0.1:8001",
                    "--entrypoint",
                    "127.0.0.1:8002",
                    "--entrypoint",
                    "127.0.0.1:8000",
                ],
                expected_args,
            );
        }
    }

    #[test]
    fn verify_args_struct_by_command_run_with_check_vote_account() {
        // long arg
        {
            let default_run_args = RunArgs::default();
            let expected_args = RunArgs {
                entrypoints: vec![SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                    8000,
                )],
                rpc_bootstrap_config: RpcBootstrapConfig {
                    check_vote_account: Some("https://api.mainnet-beta.solana.com".to_string()),
                    ..RpcBootstrapConfig::default()
                },
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args,
                vec![
                    // entrypoint is required for check-vote-account
                    "--entrypoint",
                    "127.0.0.1:8000",
                    "--check-vote-account",
                    "https://api.mainnet-beta.solana.com",
                ],
                expected_args,
            );
        }
    }

    #[test]
    fn verify_args_struct_by_command_run_with_known_validators() {
        // long arg + single known validator
        {
            let default_run_args = RunArgs::default();
            let known_validators_pubkey = Pubkey::new_unique();
            let known_validators = Some(HashSet::from([known_validators_pubkey]));
            let expected_args = RunArgs {
                known_validators,
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args,
                vec!["--known-validator", &known_validators_pubkey.to_string()],
                expected_args,
            );
        }

        // alias + single known validator
        {
            let default_run_args = RunArgs::default();
            let known_validators_pubkey = Pubkey::new_unique();
            let known_validators = Some(HashSet::from([known_validators_pubkey]));
            let expected_args = RunArgs {
                known_validators,
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args,
                vec!["--trusted-validator", &known_validators_pubkey.to_string()],
                expected_args,
            );
        }

        // long arg + multiple known validators
        {
            let default_run_args = RunArgs::default();
            let known_validators_pubkey_1 = Pubkey::new_unique();
            let known_validators_pubkey_2 = Pubkey::new_unique();
            let known_validators_pubkey_3 = Pubkey::new_unique();
            let known_validators = Some(HashSet::from([
                known_validators_pubkey_1,
                known_validators_pubkey_2,
                known_validators_pubkey_3,
            ]));
            let expected_args = RunArgs {
                known_validators,
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args,
                vec![
                    "--known-validator",
                    &known_validators_pubkey_1.to_string(),
                    "--known-validator",
                    &known_validators_pubkey_2.to_string(),
                    "--known-validator",
                    &known_validators_pubkey_3.to_string(),
                ],
                expected_args,
            );
        }

        // long arg + duplicate known validators
        {
            let default_run_args = RunArgs::default();
            let known_validators_pubkey_1 = Pubkey::new_unique();
            let known_validators_pubkey_2 = Pubkey::new_unique();
            let known_validators = Some(HashSet::from([
                known_validators_pubkey_1,
                known_validators_pubkey_2,
            ]));
            let expected_args = RunArgs {
                known_validators,
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args,
                vec![
                    "--known-validator",
                    &known_validators_pubkey_1.to_string(),
                    "--known-validator",
                    &known_validators_pubkey_2.to_string(),
                    "--known-validator",
                    &known_validators_pubkey_1.to_string(),
                ],
                expected_args,
            );
        }

        // use identity pubkey as known validator
        {
            let default_args = DefaultArgs::default();
            let default_run_args = RunArgs::default();

            // generate a keypair
            let tmp_dir = tempfile::tempdir().unwrap();
            let file = tmp_dir.path().join("id.json");
            solana_keypair::write_keypair_file(&default_run_args.identity_keypair, &file).unwrap();

            let matches = add_args(Command::new("run_command"), &default_args).get_matches_from(vec![
                "run_command",
                "--identity",
                file.to_str().unwrap(),
                "--known-validator",
                &default_run_args.identity_keypair.pubkey().to_string(),
            ]);
            let result = RunArgs::from_clap_arg_match(&matches);
            assert!(result.is_err());
            let error = result.unwrap_err();
            assert_eq!(
                error.to_string(),
                format!(
                    "the validator's identity pubkey cannot be a known validator: {}",
                    default_run_args.identity_keypair.pubkey()
                )
            );
        }
    }

    #[test]
    fn verify_args_struct_by_command_run_with_only_known_rpc() {
        // long arg
        {
            let default_run_args = RunArgs::default();
            let known_validators_pubkey = Pubkey::new_unique();
            let known_validators = Some(HashSet::from([known_validators_pubkey]));
            let expected_args = RunArgs {
                known_validators,
                rpc_bootstrap_config: RpcBootstrapConfig {
                    only_known_rpc: true,
                    ..RpcBootstrapConfig::default()
                },
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args,
                vec![
                    // --known-validator is required
                    "--known-validator",
                    &known_validators_pubkey.to_string(),
                    "--only-known-rpc",
                ],
                expected_args,
            );
        }

        // alias
        {
            let default_run_args = RunArgs::default();
            let known_validators_pubkey = Pubkey::new_unique();
            let known_validators = Some(HashSet::from([known_validators_pubkey]));
            let expected_args = RunArgs {
                known_validators,
                rpc_bootstrap_config: RpcBootstrapConfig {
                    only_known_rpc: true,
                    ..RpcBootstrapConfig::default()
                },
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args,
                vec![
                    // --known-validator is required
                    "--known-validator",
                    &known_validators_pubkey.to_string(),
                    "--no-untrusted-rpc",
                ],
                expected_args,
            );
        }
    }

    #[test]
    fn verify_args_struct_by_command_run_with_max_genesis_archive_unpacked_size() {
        // long arg
        {
            let default_run_args = RunArgs::default();
            let max_genesis_archive_unpacked_size = 1000000000;
            let expected_args = RunArgs {
                rpc_bootstrap_config: RpcBootstrapConfig {
                    max_genesis_archive_unpacked_size,
                    ..RpcBootstrapConfig::default()
                },
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args,
                vec![
                    "--max-genesis-archive-unpacked-size",
                    &max_genesis_archive_unpacked_size.to_string(),
                ],
                expected_args,
            );
        }
    }

    #[test]
    fn verify_args_struct_by_command_run_with_incremental_snapshot_fetch() {
        // long arg
        {
            let default_run_args = RunArgs::default();
            let expected_args = RunArgs {
                rpc_bootstrap_config: RpcBootstrapConfig {
                    incremental_snapshot_fetch: false,
                    ..RpcBootstrapConfig::default()
                },
                ..default_run_args.clone()
            };
            verify_args_struct_by_command_run_with_identity_setup(
                default_run_args,
                vec!["--no-incremental-snapshots"],
                expected_args,
            );
        }
    }
}
