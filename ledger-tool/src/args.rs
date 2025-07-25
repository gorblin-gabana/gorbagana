use {
    crate::LEDGER_TOOL_DIRECTORY,
    clap::{Arg, ArgMatches, ArgAction},
    solana_account_decoder::{UiAccountEncoding, UiDataSliceConfig},
    solana_accounts_db::{
        accounts_db::{AccountsDb, AccountsDbConfig},
        accounts_file::StorageAccess,
        accounts_index::{AccountsIndexConfig, IndexLimitMb, ScanFilter},
        utils::create_and_canonicalize_directories,
    },
    solana_clap_utils::{
        hidden_unless_forced,
        input_parsers::pubkeys_of,
        input_validators::{is_parsable, is_pow2, is_within_range},
    },
    solana_cli_output::CliAccountNewConfig,
    solana_clock::Slot,
    solana_ledger::{
        blockstore_processor::ProcessOptions,
        use_snapshot_archives_at_startup::{self, UseSnapshotArchivesAtStartup},
    },
    solana_runtime::runtime_config::RuntimeConfig,
    std::{
        collections::HashSet,
        num::NonZeroUsize,
        path::{Path, PathBuf},
        sync::Arc,
    },
};

/// Returns the arguments that configure AccountsDb
pub fn accounts_db_args() -> Box<[Arg]> {
    vec![
        Arg::new("account_paths")
            .long("accounts")
            .value_name("PATHS")
            
            .help(
                "Persistent accounts location. May be specified multiple times. \
                [default: <LEDGER>/accounts]",
            ),
        Arg::new("accounts_index_path")
            .long("accounts-index-path")
            .value_name("PATH")
            
            .action(ArgAction::Append)
            .help(
                "Persistent accounts-index location. May be specified multiple times. \
                [default: <LEDGER>/accounts_index]",
            ),
        Arg::new("accounts_hash_cache_path")
            .long("accounts-hash-cache-path")
            .value_name("PATH")
            
            .help(
                "Use PATH as accounts hash cache location [default: <LEDGER>/accounts_hash_cache]",
            ),
        Arg::new("accounts_index_bins")
            .long("accounts-index-bins")
            .value_name("BINS")
                            .value_parser(clap::value_parser!(usize))
            
            .help("Number of bins to divide the accounts index into"),
        Arg::new("disable_accounts_disk_index")
            .long("disable-accounts-disk-index")
            .help(
                "Disable the disk-based accounts index. It is enabled by default. The entire \
                 accounts index will be kept in memory.",
            ),
        Arg::new("accounts_db_skip_shrink")
            .long("accounts-db-skip-shrink")
            .help(
                "Enables faster starting of ledger-tool by skipping shrink. This option is for \
                 use during testing.",
            ),
        Arg::new("accounts_db_verify_refcounts")
            .long("accounts-db-verify-refcounts")
            .help(
                "Debug option to scan all AppendVecs and verify account index refcounts prior to \
                 clean",
            )
            .hide(hidden_unless_forced()),
        Arg::new("accounts_db_scan_filter_for_shrinking")
            .long("accounts-db-scan-filter-for-shrinking")
            
                            .value_parser(["all", "only-abnormal", "only-abnormal-with-verify"])
            .help(
                "Debug option to use different type of filtering for accounts index scan in \
                 shrinking. \"all\" will scan both in-memory and on-disk accounts index, which is \
                 the default. \"only-abnormal\" will scan in-memory accounts index only for \
                 abnormal entries and skip scanning on-disk accounts index by assuming that \
                 on-disk accounts index contains only normal accounts index entry. \
                 \"only-abnormal-with-verify\" is similar to \"only-abnormal\", which will scan \
                 in-memory index for abnormal entries, but will also verify that on-disk account \
                 entries are indeed normal.",
            )
            .hide(hidden_unless_forced()),
        Arg::new("accounts_db_skip_initial_hash_calculation")
            .long("accounts-db-skip-initial-hash-calculation")
            .help("Do not verify accounts hash at startup.")
            .hide(hidden_unless_forced()),
        Arg::new("accounts_db_ancient_append_vecs")
            .long("accounts-db-ancient-append-vecs")
            .value_name("SLOT-OFFSET")
            .value_parser(clap::value_parser!(String))
            
            .help(
                "AppendVecs that are older than (slots_per_epoch - SLOT-OFFSET) are squashed \
                 together.",
            )
            .hide(hidden_unless_forced()),
        Arg::new("accounts_db_access_storages_method")
            .long("accounts-db-access-storages-method")
            .value_name("METHOD")
            
                            .value_parser(["mmap", "file"])
            .help("Access account storages using this method"),
        Arg::new("accounts_db_snapshots_use_experimental_accumulator_hash")
            .long("accounts-db-snapshots-use-experimental-accumulator-hash")
            .help("Snapshots use the experimental accumulator hash")
            .hide(hidden_unless_forced()),
        Arg::new("accounts_db_hash_threads")
            .long("accounts-db-hash-threads")
            .value_name("NUM_THREADS")
            
                            .value_parser(clap::value_parser!(usize))
            .help("Number of threads to use for background accounts hashing")
            .hide(hidden_unless_forced()),
        Arg::new("accounts_db_ancient_storage_ideal_size")
            .long("accounts-db-ancient-storage-ideal-size")
            .value_name("BYTES")
            .value_parser(clap::value_parser!(String))
            
            .help("The smallest size of ideal ancient storage.")
            .hide(hidden_unless_forced()),
        Arg::new("accounts_db_max_ancient_storages")
            .long("accounts-db-max-ancient-storages")
            .value_name("USIZE")
            .value_parser(clap::value_parser!(String))
            
            .help("The number of ancient storages the ancient slot combining should converge to.")
            .hide(hidden_unless_forced()),
        Arg::new("accounts_db_hash_calculation_pubkey_bins")
            .long("accounts-db-hash-calculation-pubkey-bins")
            .value_name("USIZE")
            .value_parser(clap::value_parser!(String))
            
            .help("The number of pubkey bins used for accounts hash calculation.")
            .hide(hidden_unless_forced()),
    ]
    .into_boxed_slice()
}

// For our current version of CLAP, the value passed to Arg::default_value()
// must be a &str. But, we can't convert an integer to a &str at compile time.
// So, declare this constant and enforce equality with the following unit test
// test_max_genesis_archive_unpacked_size_constant
const MAX_GENESIS_ARCHIVE_UNPACKED_SIZE_STR: &str = "10485760";

/// Returns the arguments that configure loading genesis
pub fn load_genesis_arg() -> Arg {
    Arg::new("max_genesis_archive_unpacked_size")
        .long("max-genesis-archive-unpacked-size")
        .value_name("NUMBER")
        
        .default_value(MAX_GENESIS_ARCHIVE_UNPACKED_SIZE_STR)
        .help("maximum total uncompressed size of unpacked genesis archive")
}

/// Returns the arguments that configure snapshot loading
pub fn snapshot_args() -> Box<[Arg]> {
    vec![
        Arg::new("no_snapshot")
            .long("no-snapshot")
            .action(ArgAction::SetTrue)
            .help("Do not start from a local snapshot if present"),
        Arg::new("snapshots")
            .long("snapshots")
            .alias("snapshot-archive-path")
            .alias("full-snapshot-archive-path")
            .value_name("DIR")
            
            .global(true)
            .help("Use DIR for snapshot location [default: --ledger value]"),
        Arg::new("incremental_snapshot_archive_path")
            .long("incremental-snapshot-archive-path")
            .value_name("DIR")
            
            .global(true)
            .help("Use DIR for separate incremental snapshot location"),
        Arg::new(use_snapshot_archives_at_startup::cli::NAME)
            .long(use_snapshot_archives_at_startup::cli::LONG_ARG)
            
                            .value_parser(clap::value_parser!(String))
            .default_value(use_snapshot_archives_at_startup::cli::default_value_for_ledger_tool())
            .help(use_snapshot_archives_at_startup::cli::HELP)
            .long_help(use_snapshot_archives_at_startup::cli::LONG_HELP),
    ]
    .into_boxed_slice()
}

/// Parse a `ProcessOptions` from subcommand arguments. This function attempts
/// to parse all flags related to `ProcessOptions`; however, subcommands that
/// use this function may not support all flags.
pub fn parse_process_options(ledger_path: &Path, arg_matches: &ArgMatches) -> ProcessOptions {
    let new_hard_forks = hardforks_of(arg_matches, "hard_forks");
    let accounts_db_config = Some(get_accounts_db_config(ledger_path, arg_matches));
    let log_messages_bytes_limit = arg_matches.get_one::<String>("log_messages_bytes_limit").and_then(|s| s.parse().ok());
    let runtime_config = RuntimeConfig {
        log_messages_bytes_limit,
        ..RuntimeConfig::default()
    };

    if arg_matches.get_flag("skip_poh_verify") {
        eprintln!("--skip-poh-verify is deprecated.  Replace with --skip-verification.");
    }
    let run_verification =
        !(arg_matches.get_flag("skip_poh_verify") || arg_matches.get_flag("skip_verification"));
    let halt_at_slot = arg_matches.get_one::<String>("halt_at_slot").and_then(|s| s.parse().ok());
    let use_snapshot_archives_at_startup = arg_matches
        .get_one::<String>(use_snapshot_archives_at_startup::cli::NAME)
        .unwrap().parse().unwrap();
    let accounts_db_skip_shrink = arg_matches.get_flag("accounts_db_skip_shrink");
    let verify_index = arg_matches.get_flag("verify_accounts_index");
    let limit_load_slot_count_from_snapshot =
        arg_matches.get_one::<String>("limit_load_slot_count_from_snapshot").and_then(|s| s.parse().ok());
    let run_final_accounts_hash_calc = arg_matches.get_flag("run_final_hash_calc");
    let debug_keys = arg_matches.get_many::<String>("debug_key")
        .map(|values| Arc::new(values.filter_map(|s| s.parse().ok()).collect::<HashSet<_>>()));
    let allow_dead_slots = arg_matches.get_flag("allow_dead_slots");
    let abort_on_invalid_block = arg_matches.get_flag("abort_on_invalid_block");
    let no_block_cost_limits = arg_matches.get_flag("no_block_cost_limits");

    ProcessOptions {
        new_hard_forks,
        runtime_config,
        accounts_db_config,
        accounts_db_skip_shrink,
        verify_index,
        limit_load_slot_count_from_snapshot,
        run_final_accounts_hash_calc,
        debug_keys,
        run_verification,
        allow_dead_slots,
        halt_at_slot,
        use_snapshot_archives_at_startup,
        abort_on_invalid_block,
        no_block_cost_limits,
        ..ProcessOptions::default()
    }
}

// Build an `AccountsDbConfig` from subcommand arguments. All of the arguments
// matched by this functional are either optional or have a default value.
// Thus, a subcommand need not support all of the arguments that are matched
// by this function.
pub fn get_accounts_db_config(
    ledger_path: &Path,
    arg_matches: &ArgMatches,
) -> AccountsDbConfig {
    let ledger_tool_ledger_path = ledger_path.join(LEDGER_TOOL_DIRECTORY);

    let accounts_index_bins = arg_matches.get_one::<String>("accounts_index_bins").and_then(|s| s.parse().ok());
    let accounts_index_index_limit_mb = if arg_matches.get_flag("disable_accounts_disk_index") {
        IndexLimitMb::InMemOnly
    } else {
        IndexLimitMb::Minimal
    };
    let accounts_index_drives = arg_matches.get_many::<String>("accounts_index_path")
        .map(|values| values.map(|s| s.parse::<String>().unwrap()).into_iter().map(PathBuf::from).collect())
        .unwrap_or_else(|| vec![ledger_tool_ledger_path.join("accounts_index")]);
    let accounts_index_config = AccountsIndexConfig {
        bins: accounts_index_bins,
        index_limit_mb: accounts_index_index_limit_mb,
        drives: Some(accounts_index_drives),
        ..AccountsIndexConfig::default()
    };

    let accounts_hash_cache_path = arg_matches
        .get_one::<String>("accounts_hash_cache_path")
        .map(Into::into)
        .unwrap_or_else(|| {
            ledger_tool_ledger_path.join(AccountsDb::DEFAULT_ACCOUNTS_HASH_CACHE_DIR)
        });
    let accounts_hash_cache_path = create_and_canonicalize_directories([&accounts_hash_cache_path])
        .unwrap_or_else(|err| {
            eprintln!(
                "Unable to access accounts hash cache path '{}': {err}",
                accounts_hash_cache_path.display(),
            );
            std::process::exit(1);
        })
        .pop()
        .unwrap();

    let storage_access = arg_matches
        .get_one::<String>("accounts_db_access_storages_method")
        .map(|method| match method.as_str() {
            "mmap" => StorageAccess::Mmap,
            "file" => StorageAccess::File,
            _ => {
                // clap will enforce one of the above values is given
                unreachable!("invalid value given to accounts-db-access-storages-method")
            }
        })
        .unwrap_or_default();

    let scan_filter_for_shrinking = arg_matches
        .get_one::<String>("accounts_db_scan_filter_for_shrinking")
        .map(|filter| match filter.as_str() {
            "all" => ScanFilter::All,
            "only-abnormal" => ScanFilter::OnlyAbnormal,
            "only-abnormal-with-verify" => ScanFilter::OnlyAbnormalWithVerify,
            _ => {
                // clap will enforce one of the above values is given
                unreachable!("invalid value given to accounts_db_scan_filter_for_shrinking")
            }
        })
        .unwrap_or_default();

    let num_hash_threads = arg_matches
        .get_one::<String>("accounts_db_hash_threads")
        .map(|s| s.parse().unwrap());

    AccountsDbConfig {
        index: Some(accounts_index_config),
        base_working_path: Some(ledger_tool_ledger_path),
        accounts_hash_cache_path: Some(accounts_hash_cache_path),
        ancient_append_vec_offset: arg_matches.get_one::<String>("accounts_db_ancient_append_vecs").map(|s| s.parse::<i64>().unwrap()),
        ancient_storage_ideal_size: arg_matches.get_one::<String>("accounts_db_ancient_storage_ideal_size").map(|s| s.parse::<u64>().unwrap()),
        max_ancient_storages: arg_matches.get_one::<String>("accounts_db_max_ancient_storages").and_then(|s| s.parse().ok()),
        hash_calculation_pubkey_bins: arg_matches.get_one::<String>("accounts_db_hash_calculation_pubkey_bins").map(|s| s.parse::<usize>().unwrap()),
        exhaustively_verify_refcounts: arg_matches.get_flag("accounts_db_verify_refcounts"),
        skip_initial_hash_calc: arg_matches.get_flag("accounts_db_skip_initial_hash_calculation"),
        storage_access,
        scan_filter_for_shrinking,
        snapshots_use_experimental_accumulator_hash: arg_matches
            .get_flag("accounts_db_snapshots_use_experimental_accumulator_hash"),
        num_hash_threads,
        ..AccountsDbConfig::default()
    }
}

pub(crate) fn parse_encoding_format(matches: &ArgMatches) -> UiAccountEncoding {
    match matches.get_one::<String>("encoding").map(|s| s.as_str()) {
        Some("jsonParsed") => UiAccountEncoding::JsonParsed,
        Some("base64") => UiAccountEncoding::Base64,
        Some("base64+zstd") => UiAccountEncoding::Base64Zstd,
        _ => UiAccountEncoding::Base64,
    }
}

pub(crate) fn parse_account_output_config(matches: &ArgMatches) -> CliAccountNewConfig {
    let data_encoding = parse_encoding_format(matches);
    let output_account_data = !matches.get_flag("no_account_data");
    let data_slice_config = if output_account_data {
        // None yields the entire account in the slice
        None
    } else {
        // usize::MAX is a sentinel that will yield an
        // empty data slice. Because of this, length is
        // ignored so any value will do
        let offset = usize::MAX;
        let length = 0;
        Some(UiDataSliceConfig { offset, length })
    };

    CliAccountNewConfig {
        data_encoding,
        data_slice_config,
        ..CliAccountNewConfig::default()
    }
}

// This function is duplicated in validator/src/main.rs...
pub fn hardforks_of(matches: &ArgMatches, name: &str) -> Option<Vec<Slot>> {
    if matches.get_flag(name) {
        Some(matches.get_many::<String>(name).unwrap_or_else(|| std::process::exit(1)).map(|s| s.parse::<Slot>().unwrap()).collect::<Vec<_>>())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use {super::*, solana_accounts_db::hardened_unpack::MAX_GENESIS_ARCHIVE_UNPACKED_SIZE};

    #[test]
    fn test_max_genesis_archive_unpacked_size_constant() {
        assert_eq!(
            MAX_GENESIS_ARCHIVE_UNPACKED_SIZE,
            MAX_GENESIS_ARCHIVE_UNPACKED_SIZE_STR
                .parse::<u64>()
                .unwrap()
        );
    }
}
