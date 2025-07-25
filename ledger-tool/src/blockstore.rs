//! The `blockstore` subcommand

use {
    crate::{
        error::{LedgerToolError, Result},
        ledger_path::canonicalize_ledger_path,
        ledger_utils::get_program_ids,
        output::{output_ledger, output_slot, CliDuplicateSlotProof, SlotBounds, SlotInfo},
    },
    chrono::{DateTime, Utc},
    clap::{
        Arg, ArgMatches, Command, ArgAction,
    },
    itertools::Itertools,
    log::*,
    regex::Regex,
    serde_json::json,
    solana_clap_utils::{hidden_unless_forced, input_validators::is_slot},
    solana_cli_output::OutputFormat,
    solana_clock::{Slot, UnixTimestamp},
    solana_hash::Hash,
    solana_ledger::{
        ancestor_iterator::AncestorIterator,
        blockstore::{
            column::{Column, ColumnName},
            Blockstore, PurgeType,
        },
        blockstore_options::AccessType,
        shred::Shred,
    },
    std::{
        borrow::Cow,
        collections::{BTreeMap, BTreeSet, HashMap},
        fs::File,
        io::{stdout, BufRead, BufReader, Write},
        path::{Path, PathBuf},
        sync::atomic::AtomicBool,
        time::{Duration, UNIX_EPOCH},
    },
};

fn analyze_column(blockstore: &Blockstore, column_name: &str) -> Result<()> {
    let mut key_len: u64 = 0;
    let mut key_tot: u64 = 0;
    let mut val_hist = histogram::Histogram::new();
    let mut val_tot: u64 = 0;
    let mut row_hist = histogram::Histogram::new();
    let column_iterator = blockstore.iterator_cf(column_name)?;
    for (key, val) in column_iterator {
        // Key length is fixed, only need to calculate it once
        if key_len == 0 {
            key_len = key.len() as u64;
        }
        let val_len = val.len() as u64;

        key_tot += key_len;
        val_hist.increment(val_len).unwrap();
        val_tot += val_len;

        row_hist.increment(key_len + val_len).unwrap();
    }

    let json_result = if val_hist.entries() > 0 {
        json!({
            "column":column_name,
            "entries":val_hist.entries(),
            "key_stats":{
                "max":key_len,
                "total_bytes":key_tot,
            },
            "val_stats":{
                "p50":val_hist.percentile(50.0).unwrap(),
                "p90":val_hist.percentile(90.0).unwrap(),
                "p99":val_hist.percentile(99.0).unwrap(),
                "p999":val_hist.percentile(99.9).unwrap(),
                "min":val_hist.minimum().unwrap(),
                "max":val_hist.maximum().unwrap(),
                "stddev":val_hist.stddev().unwrap(),
                "total_bytes":val_tot,
            },
            "row_stats":{
                "p50":row_hist.percentile(50.0).unwrap(),
                "p90":row_hist.percentile(90.0).unwrap(),
                "p99":row_hist.percentile(99.0).unwrap(),
                "p999":row_hist.percentile(99.9).unwrap(),
                "min":row_hist.minimum().unwrap(),
                "max":row_hist.maximum().unwrap(),
                "stddev":row_hist.stddev().unwrap(),
                "total_bytes":key_tot + val_tot,
            },
        })
    } else {
        json!({
        "column":column_name,
        "entries":val_hist.entries(),
        "key_stats":{
            "max":key_len,
            "total_bytes":0,
        },
        "val_stats":{
            "total_bytes":0,
        },
        "row_stats":{
            "total_bytes":0,
        },
        })
    };

    println!("{}", serde_json::to_string_pretty(&json_result)?);
    Ok(())
}

fn analyze_storage(blockstore: &Blockstore) -> Result<()> {
    use solana_ledger::blockstore::column::columns::*;
    analyze_column(blockstore, SlotMeta::NAME)?;
    analyze_column(blockstore, Orphans::NAME)?;
    analyze_column(blockstore, DeadSlots::NAME)?;
    analyze_column(blockstore, DuplicateSlots::NAME)?;
    analyze_column(blockstore, ErasureMeta::NAME)?;
    analyze_column(blockstore, BankHash::NAME)?;
    analyze_column(blockstore, Root::NAME)?;
    analyze_column(blockstore, Index::NAME)?;
    analyze_column(blockstore, ShredData::NAME)?;
    analyze_column(blockstore, ShredCode::NAME)?;
    analyze_column(blockstore, TransactionStatus::NAME)?;
    analyze_column(blockstore, AddressSignatures::NAME)?;
    analyze_column(blockstore, TransactionMemos::NAME)?;
    analyze_column(blockstore, TransactionStatusIndex::NAME)?;
    analyze_column(blockstore, Rewards::NAME)?;
    analyze_column(blockstore, Blocktime::NAME)?;
    analyze_column(blockstore, PerfSamples::NAME)?;
    analyze_column(blockstore, BlockHeight::NAME)?;
    analyze_column(blockstore, OptimisticSlots::NAME)
}

fn raw_key_to_slot(key: &[u8], column_name: &str) -> Option<Slot> {
    use solana_ledger::blockstore::column::columns as cf;
    match column_name {
        cf::SlotMeta::NAME => Some(cf::SlotMeta::slot(cf::SlotMeta::index(key))),
        cf::Orphans::NAME => Some(cf::Orphans::slot(cf::Orphans::index(key))),
        cf::DeadSlots::NAME => Some(cf::SlotMeta::slot(cf::SlotMeta::index(key))),
        cf::DuplicateSlots::NAME => Some(cf::SlotMeta::slot(cf::SlotMeta::index(key))),
        cf::ErasureMeta::NAME => Some(cf::ErasureMeta::slot(cf::ErasureMeta::index(key))),
        cf::BankHash::NAME => Some(cf::BankHash::slot(cf::BankHash::index(key))),
        cf::Root::NAME => Some(cf::Root::slot(cf::Root::index(key))),
        cf::Index::NAME => Some(cf::Index::slot(cf::Index::index(key))),
        cf::ShredData::NAME => Some(cf::ShredData::slot(cf::ShredData::index(key))),
        cf::ShredCode::NAME => Some(cf::ShredCode::slot(cf::ShredCode::index(key))),
        cf::TransactionStatus::NAME => Some(cf::TransactionStatus::slot(
            cf::TransactionStatus::index(key),
        )),
        cf::AddressSignatures::NAME => Some(cf::AddressSignatures::slot(
            cf::AddressSignatures::index(key),
        )),
        cf::TransactionMemos::NAME => None, // does not implement slot()
        cf::TransactionStatusIndex::NAME => None, // does not implement slot()
        cf::Rewards::NAME => Some(cf::Rewards::slot(cf::Rewards::index(key))),
        cf::Blocktime::NAME => Some(cf::Blocktime::slot(cf::Blocktime::index(key))),
        cf::PerfSamples::NAME => Some(cf::PerfSamples::slot(cf::PerfSamples::index(key))),
        cf::BlockHeight::NAME => Some(cf::BlockHeight::slot(cf::BlockHeight::index(key))),
        cf::OptimisticSlots::NAME => {
            Some(cf::OptimisticSlots::slot(cf::OptimisticSlots::index(key)))
        }
        &_ => None,
    }
}

/// Returns true if the supplied slot contains any nonvote transactions
fn slot_contains_nonvote_tx(blockstore: &Blockstore, slot: Slot) -> bool {
    let (entries, _, _) = blockstore
        .get_slot_entries_with_shred_info(slot, 0, false)
        .expect("Failed to get slot entries");
    let contains_nonvote = entries
        .iter()
        .flat_map(|entry| entry.transactions.iter())
        .flat_map(get_program_ids)
        .any(|program_id| *program_id != solana_vote_program::id());
    contains_nonvote
}

type OptimisticSlotInfo = (Slot, Option<(Hash, UnixTimestamp)>, bool);

/// Return the latest `num_slots` optimistically confirmed slots, including
/// ancestors of optimistically confirmed slots that may not have been marked
/// as optimistically confirmed themselves.
fn get_latest_optimistic_slots(
    blockstore: &Blockstore,
    num_slots: usize,
    exclude_vote_only_slots: bool,
) -> Vec<OptimisticSlotInfo> {
    // Consider a chain X -> Y -> Z where X and Z have been optimistically
    // confirmed. Given that Y is an ancestor of Z, Y can implicitly be
    // considered as optimistically confirmed. However, there isn't an explicit
    // guarantee that Y will be marked as optimistically confirmed in the
    // blockstore.
    //
    // Because retrieving optimistically confirmed slots is an important part
    // of cluster restarts, exercise caution in this function and manually walk
    // the ancestors of the latest optimistically confirmed slot instead of
    // solely relying on the contents of the optimistically confirmed column.
    let Some(latest_slot) = blockstore
        .get_latest_optimistic_slots(1)
        .expect("get_latest_optimistic_slots() failed")
        .pop()
    else {
        eprintln!("Blockstore does not contain any optimistically confirmed slots");
        return vec![];
    };
    let latest_slot = latest_slot.0;

    let slot_iter = AncestorIterator::new_inclusive(latest_slot, blockstore).map(|slot| {
        let contains_nonvote_tx = slot_contains_nonvote_tx(blockstore, slot);
        let hash_and_timestamp_opt = blockstore
            .get_optimistic_slot(slot)
            .expect("get_optimistic_slot() failed");
        if hash_and_timestamp_opt.is_none() {
            warn!(
                "Slot {slot} is an ancestor of latest optimistically confirmed slot \
                 {latest_slot}, but was not marked as optimistically confirmed in blockstore."
            );
        }
        (slot, hash_and_timestamp_opt, contains_nonvote_tx)
    });

    if exclude_vote_only_slots {
        slot_iter
            .filter(|(_, _, contains_nonvote)| *contains_nonvote)
            .take(num_slots)
            .collect()
    } else {
        slot_iter.take(num_slots).collect()
    }
}

fn print_blockstore_file_metadata(blockstore: &Blockstore, file_name: &Option<&str>) -> Result<()> {
    let live_files = blockstore.live_files_metadata()?;

    // All files under live_files_metadata are prefixed with "/".
    let sst_file_name = file_name.as_ref().map(|name| format!("/{name}"));
    for file in live_files {
        if sst_file_name.is_none() || file.name.eq(sst_file_name.as_ref().unwrap()) {
            println!(
                "[{}] cf_name: {}, level: {}, start_slot: {:?}, end_slot: {:?}, size: {}, \
                 num_entries: {}",
                file.name,
                file.column_family_name,
                file.level,
                raw_key_to_slot(&file.start_key.unwrap(), &file.column_family_name),
                raw_key_to_slot(&file.end_key.unwrap(), &file.column_family_name),
                file.size,
                file.num_entries,
            );
            if sst_file_name.is_some() {
                return Ok(());
            }
        }
    }
    if sst_file_name.is_some() {
        return Err(LedgerToolError::BadArgument(format!(
            "failed to find or load the metadata of the specified file {file_name:?}"
        )));
    }
    Ok(())
}

pub trait BlockstoreSubCommand {
    fn blockstore_subcommand(self) -> Self;
}

impl BlockstoreSubCommand for Command {
    fn blockstore_subcommand(self) -> Self {
        self.subcommand(
            Command::new("blockstore")
                .about("Commands to interact with a local Blockstore")
                .subcommand_required(true)
                .subcommands(blockstore_subcommands(false)),
        )
    }
}

pub fn blockstore_subcommands(hidden: bool) -> Vec<Command> {
    let _hidden = hidden; // Placeholder for now

    let starting_slot_arg = Arg::new("starting_slot")
        .long("starting-slot")
        .value_name("SLOT")
        
        .default_value("0")
        .help("Start at this slot");
    let ending_slot_arg = Arg::new("ending_slot")
        .long("ending-slot")
        .value_name("SLOT")
        
        .help("The last slot to iterate to");
    let allow_dead_slots_arg = Arg::new("allow_dead_slots")
        .long("allow-dead-slots")
        .action(ArgAction::SetTrue)
        .help("Output dead slots as well");

    vec![
        Command::new("analyze-storage")
            .about(
                "Output statistics in JSON format about all column families in the ledger rocksdb",
            ),
        Command::new("bounds")
            .about(
                "Print lowest and highest non-empty slots. Note that there may be empty slots \
                 within the bounds",
            )
            .arg(
                Arg::new("all")
                    .long("all")
                    .action(ArgAction::SetTrue)
                    .required(false)
                    .help("Additionally print all the non-empty slots within the bounds"),
            ),
        Command::new("copy")
            .about("Copy the ledger")
            .arg(&starting_slot_arg)
            .arg(&ending_slot_arg)
            .arg(
                Arg::new("target_ledger")
                    .long("target-ledger")
                    .value_name("DIR")
                    
                    .help("Target ledger directory to write inner \"rocksdb\" within."),
            ),
        Command::new("dead-slots")
            .about("Print all the dead slots in the ledger")
            .arg(&starting_slot_arg),
        Command::new("duplicate-slots")
            .about("Print all the duplicate slots in the ledger")
            .arg(&starting_slot_arg),
        Command::new("latest-optimistic-slots")
            .about(
                "Output up to the most recent <num-slots> optimistic slots with their hashes and \
                 timestamps.",
            )
            // This command is important in cluster restart scenarios, so do not hide it ever
            // such that the subcommand will be visible as the top level of agave-ledger-tool
            .arg(
                Arg::new("num_slots")
                    .long("num-slots")
                    .value_name("NUM")
                    
                    .default_value("1")
                    .required(false)
                    .help("Number of slots in the output"),
            )
            .arg(
                Arg::new("exclude_vote_only_slots")
                    .long("exclude-vote-only-slots")
                    .required(false)
                    .help("Exclude slots that contain only votes from output"),
            ),
        Command::new("list-roots")
            .about(
                "Output up to last <num-roots> root hashes and their heights starting at the \
                 given block height",
            )
            .arg(
                Arg::new("max_height")
                    .long("max-height")
                    .value_name("NUM")
                    
                    .help("Maximum block height"),
            )
            .arg(
                Arg::new("start_root")
                    .long("start-root")
                    .value_name("NUM")
                    
                    .help("First root to start searching from"),
            )
            .arg(
                Arg::new("slot_list")
                    .long("slot-list")
                    .value_name("FILENAME")
                    .required(false)
                    
                    .help(
                        "The location of the output YAML file. A list of rollback slot heights \
                         and hashes will be written to the file",
                    ),
            )
            .arg(
                Arg::new("num_roots")
                    .long("num-roots")
                    .value_name("NUM")
                    
                    .default_value("1")
                    .required(false)
                    .help("Number of roots in the output"),
            ),
        Command::new("parse_full_frozen")
            .about(
                "Parses log for information about critical events about ancestors of the given \
                 `ending_slot`",
            )
            .arg(&starting_slot_arg)
            .arg(&ending_slot_arg)
            .arg(
                Arg::new("log_path")
                    .long("log-path")
                    .value_name("PATH")
                    
                    .help("path to log file to parse"),
            ),
        Command::new("print")
            .about("Print the ledger")
            .arg(&starting_slot_arg)
            .arg(&ending_slot_arg)
            .arg(&allow_dead_slots_arg)
            .arg(
                Arg::new("num_slots")
                    .long("num-slots")
                    .value_name("SLOT")
                    .value_parser(clap::value_parser!(u64))
                    
                    .help("Number of slots to print"),
            )
            .arg(
                Arg::new("only_rooted")
                    .long("only-rooted")
                    .action(ArgAction::SetTrue)
                    .help("Only print root slots"),
            ),
        Command::new("print-file-metadata")
            .about(
                "Print the metadata of the specified ledger-store file. If no file name is \
                 specified, it will print the metadata of all ledger files.",
            )
            .arg(
                Arg::new("file_name")
                    .long("file-name")
                    
                    .value_name("SST_FILE_NAME")
                    .help(
                        "The ledger file name (e.g. 011080.sst.) If no file name is specified, it \
                         will print the metadata of all ledger files.",
                    ),
            ),
        Command::new("purge")
            .about("Delete a range of slots from the ledger")
            .arg(
                Arg::new("start_slot")
                    .index(1)
                    .value_name("SLOT")
                    
                    .required(true)
                    .help("Start slot to purge from (inclusive)"),
            )
            .arg(Arg::new("end_slot").index(2).value_name("SLOT").help(
                "Ending slot to stop purging (inclusive). \
                 [default: the highest slot in the ledger]",
            ))
            .arg(
                Arg::new("batch_size")
                    .long("batch-size")
                    .value_name("NUM")
                    
                    .default_value("1000")
                    .help("Removes at most BATCH_SIZE slots while purging in loop"),
            )
            .arg(
                Arg::new("no_compaction")
                    .long("no-compaction")
                    .required(false)
                    .action(ArgAction::SetTrue)
                    .help(
                        "--no-compaction is deprecated, ledger compaction after purge is disabled \
                         by default",
                    )
                    .conflicts_with("enable_compaction")
                    .hide(hidden_unless_forced()),
            )
            .arg(
                Arg::new("enable_compaction")
                    .long("enable-compaction")
                    .required(false)
                    .action(ArgAction::SetTrue)
                    .help(
                        "Perform ledger compaction after purge. Compaction will optimize storage \
                         space, but may take a long time to complete.",
                    )
                    .conflicts_with("no_compaction"),
            )
            .arg(
                Arg::new("dead_slots_only")
                    .long("dead-slots-only")
                    .required(false)
                    .action(ArgAction::SetTrue)
                    .help("Limit purging to dead slots only"),
            ),
        Command::new("remove-dead-slot")
            .about("Remove the dead flag for a slot")
            .arg(
                Arg::new("slots")
                    .index(1)
                    .value_name("SLOTS")
                    .value_parser(clap::value_parser!(u64))
                    
                    .action(ArgAction::Append)
                    .required(true)
                    .help("Slots to mark as not dead"),
            ),
        Command::new("repair-roots")
            .about(
                "Traverses the AncestorIterator backward from a last known root to restore \
                 missing roots to the Root column",
            )
            .arg(
                Arg::new("start_root")
                    .long("before")
                    .value_name("NUM")
                    
                    .help("Recent root after the range to repair"),
            )
            .arg(
                Arg::new("end_root")
                    .long("until")
                    .value_name("NUM")
                    
                    .help("Earliest slot to check for root repair"),
            )
            .arg(
                Arg::new("max_slots")
                    .long("repair-limit")
                    .value_name("NUM")
                    
                    .default_value("2000")
                    .required(true)
                    .help("Override the maximum number of slots to check for root repair"),
            ),
        Command::new("set-dead-slot")
            .about("Mark one or more slots dead")
            .arg(
                Arg::new("slots")
                    .index(1)
                    .value_name("SLOTS")
                    .value_parser(clap::value_parser!(u64))
                    
                    .action(ArgAction::Append)
                    .required(true)
                    .help("Slots to mark dead"),
            ),
        Command::new("shred-meta")
            .about("Prints raw shred metadata")
            .arg(&starting_slot_arg)
            .arg(&ending_slot_arg),
        Command::new("slot")
            .about("Print the contents of one or more slots")
            .arg(&allow_dead_slots_arg)
            .arg(
                Arg::new("slots")
                    .index(1)
                    .value_name("SLOTS")
                    .value_parser(clap::value_parser!(u64))
                    
                    .action(ArgAction::Append)
                    .required(true)
                    .help("Slots to print"),
            ),
    ]
}

pub fn blockstore_process_command(ledger_path: &Path, matches: &ArgMatches) {
    do_blockstore_process_command(ledger_path, matches).unwrap_or_else(|err| {
        eprintln!("Failed to complete command: {err:?}");
        std::process::exit(1);
    });
}

fn do_blockstore_process_command(ledger_path: &Path, matches: &ArgMatches) -> Result<()> {
    let ledger_path = canonicalize_ledger_path(ledger_path);
    let verbose_level = matches.get_count("verbose");

    match matches.subcommand() {
        Some(("analyze-storage", arg_matches)) => analyze_storage(&crate::open_blockstore(
            &ledger_path,
            arg_matches,
            AccessType::Secondary,
        ))?,
        Some(("bounds", arg_matches)) => {
            let output_format = if arg_matches.get_one::<String>("output_format").map(|s| s.as_str()) == Some("json") { OutputFormat::Json } else { OutputFormat::Display };
            let all = arg_matches.get_flag("all");

            let blockstore =
                crate::open_blockstore(&ledger_path, arg_matches, AccessType::Secondary);
            let slot_meta_iterator = blockstore.slot_meta_iterator(0)?;
            let slots: Vec<_> = slot_meta_iterator.map(|(slot, _)| slot).collect();

            let slot_bounds = if slots.is_empty() {
                SlotBounds::default()
            } else {
                // Collect info about slot bounds
                let mut bounds = SlotBounds {
                    slots: SlotInfo {
                        total: slots.len(),
                        first: Some(*slots.first().unwrap()),
                        last: Some(*slots.last().unwrap()),
                        ..SlotInfo::default()
                    },
                    ..SlotBounds::default()
                };
                if all {
                    bounds.all_slots = Some(&slots);
                }

                // Consider also rooted slots, if present
                let rooted_slot_iterator = blockstore.rooted_slot_iterator(0)?;
                let mut first_rooted = None;
                let mut last_rooted = None;
                let mut total_rooted = 0;
                for (i, slot) in rooted_slot_iterator.into_iter().enumerate() {
                    if i == 0 {
                        first_rooted = Some(slot);
                    }
                    last_rooted = Some(slot);
                    total_rooted += 1;
                }
                let last_root_for_comparison = last_rooted.unwrap_or_default();
                let count_past_root = slots
                    .iter()
                    .rev()
                    .take_while(|slot| *slot > &last_root_for_comparison)
                    .count();

                bounds.roots = SlotInfo {
                    total: total_rooted,
                    first: first_rooted,
                    last: last_rooted,
                    num_after_last_root: Some(count_past_root),
                };

                bounds
            };

            // Print collected data
            println!("{}", output_format.formatted_string(&slot_bounds));
        }
        Some(("copy", arg_matches)) => {
            let starting_slot = arg_matches.get_one::<String>("starting_slot").unwrap().parse().unwrap();
            let ending_slot = arg_matches.get_one::<String>("ending_slot").unwrap().parse().unwrap();
            let target_ledger =
                PathBuf::from(arg_matches.get_one::<String>("target_ledger").unwrap().clone());

            let source = crate::open_blockstore(&ledger_path, arg_matches, AccessType::Secondary);
            let target = crate::open_blockstore(
                &target_ledger,
                arg_matches,
                AccessType::PrimaryForMaintenance,
            );

            for (slot, _meta) in source.slot_meta_iterator(starting_slot)? {
                if slot > ending_slot {
                    break;
                }
                let shreds = source.get_data_shreds_for_slot(slot, 0)?;
                let shreds = shreds.into_iter().map(Cow::Owned);
                if target.insert_cow_shreds(shreds, None, true).is_err() {
                    warn!("error inserting shreds for slot {slot}");
                }
            }
        }
        Some(("dead-slots", arg_matches)) => {
            let blockstore =
                crate::open_blockstore(&ledger_path, arg_matches, AccessType::Secondary);
            let starting_slot = arg_matches.get_one::<String>("starting_slot").unwrap().parse().unwrap();
            for slot in blockstore.dead_slots_iterator(starting_slot)? {
                println!("{slot}");
            }
        }
        Some(("duplicate-slots", arg_matches)) => {
            let blockstore =
                crate::open_blockstore(&ledger_path, arg_matches, AccessType::Secondary);
            let starting_slot = arg_matches.get_one::<String>("starting_slot").unwrap().parse().unwrap();
            let output_format =
                if arg_matches.get_one::<String>("output_format").map(|s| s.as_str()) == Some("json") { OutputFormat::Json } else { OutputFormat::Display };
            for slot in blockstore.duplicate_slots_iterator(starting_slot)? {
                println!("{slot}");
                if verbose_level > 0 {
                    let proof = blockstore.get_duplicate_slot(slot).unwrap();
                    let cli_duplicate_proof = CliDuplicateSlotProof::from(proof);
                    println!("{}", output_format.formatted_string(&cli_duplicate_proof));
                }
            }
        }
        Some(("latest-optimistic-slots", arg_matches)) => {
            let blockstore =
                crate::open_blockstore(&ledger_path, arg_matches, AccessType::Secondary);
            let num_slots = arg_matches.get_one::<String>("num_slots").unwrap().parse().unwrap();
            let exclude_vote_only_slots = arg_matches.get_flag("exclude_vote_only_slots");
            let slots =
                get_latest_optimistic_slots(&blockstore, num_slots, exclude_vote_only_slots);

            println!(
                "{:>20} {:>44} {:>32} {:>13}",
                "Slot", "Hash", "Timestamp", "Vote Only?"
            );
            for (slot, hash_and_timestamp_opt, contains_nonvote) in slots.iter() {
                let (time_str, hash_str) = if let Some((hash, timestamp)) = hash_and_timestamp_opt {
                    let secs: u64 = (timestamp / 1_000) as u64;
                    let nanos: u32 = ((timestamp % 1_000) * 1_000_000) as u32;
                    let t = UNIX_EPOCH + Duration::new(secs, nanos);
                    let datetime: DateTime<Utc> = t.into();

                    (datetime.to_rfc3339(), format!("{hash}"))
                } else {
                    let unknown = "Unknown";
                    (String::from(unknown), String::from(unknown))
                };
                println!(
                    "{:>20} {:>44} {:>32} {:>13}",
                    slot, &hash_str, &time_str, !contains_nonvote
                );
            }
        }
        Some(("list-roots", arg_matches)) => {
            let blockstore =
                crate::open_blockstore(&ledger_path, arg_matches, AccessType::Secondary);

                            let max_height = arg_matches.get_one::<String>("max_height")
                    .map(|s| s.parse().unwrap()).unwrap_or(usize::MAX);
                            let start_root = arg_matches.get_one::<String>("start_root")
                    .map(|s| s.parse().unwrap()).unwrap_or(0);
            let num_roots = arg_matches.get_one::<String>("num_roots").unwrap().parse().unwrap();

            let iter = blockstore.rooted_slot_iterator(start_root)?;

            let mut output: Box<dyn Write> = if let Some(path) = arg_matches.get_one::<String>("slot_list") {
                match File::create(path) {
                    Ok(file) => Box::new(file),
                    _ => Box::new(stdout()),
                }
            } else {
                Box::new(stdout())
            };

            for slot in iter
                .take(num_roots)
                .take_while(|slot| *slot <= max_height as u64)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                let blockhash = blockstore.get_slot_entries(slot, 0)?.last().unwrap().hash;
                writeln!(output, "{slot}: {blockhash:?}").expect("failed to write");
            }
        }
        Some(("parse_full_frozen", arg_matches)) => {
            let starting_slot = arg_matches.get_one::<String>("starting_slot").unwrap().parse().unwrap();
            let ending_slot = arg_matches.get_one::<String>("ending_slot").unwrap().parse().unwrap();
            let blockstore =
                crate::open_blockstore(&ledger_path, arg_matches, AccessType::Secondary);
            let mut ancestors = BTreeSet::new();
            assert!(
                blockstore.meta(ending_slot)?.is_some(),
                "Ending slot doesn't exist"
            );
            for a in AncestorIterator::new(ending_slot, &blockstore) {
                ancestors.insert(a);
                if a <= starting_slot {
                    break;
                }
            }
            println!("ancestors: {:?}", ancestors.iter());

            let mut frozen = BTreeMap::new();
            let mut full = BTreeMap::new();
            let frozen_regex = Regex::new(r"bank frozen: (\d*)").unwrap();
            let full_regex = Regex::new(r"slot (\d*) is full").unwrap();

            let log_file = PathBuf::from(arg_matches.get_one::<String>("log_path").unwrap().clone());
            let f = BufReader::new(File::open(log_file)?);
            println!("Reading log file");
            for line in f.lines().map_while(std::io::Result::ok) {
                let parse_results = {
                    if let Some(slot_string) = frozen_regex.captures_iter(&line).next() {
                        Some((slot_string, &mut frozen))
                    } else {
                        full_regex
                            .captures_iter(&line)
                            .next()
                            .map(|slot_string| (slot_string, &mut full))
                    }
                };

                if let Some((slot_string, map)) = parse_results {
                    let slot = slot_string
                        .get(1)
                        .expect("Only one match group")
                        .as_str()
                        .parse::<u64>()
                        .unwrap();
                    if ancestors.contains(&slot) && !map.contains_key(&slot) {
                        map.insert(slot, line);
                    }
                    if slot == ending_slot && frozen.contains_key(&slot) && full.contains_key(&slot)
                    {
                        break;
                    }
                }
            }

            for ((slot1, frozen_log), (slot2, full_log)) in frozen.iter().zip(full.iter()) {
                assert_eq!(slot1, slot2);
                println!("Slot: {slot1}\n, full: {full_log}\n, frozen: {frozen_log}");
            }
        }
        Some(("print", arg_matches)) => {
            let starting_slot = arg_matches.get_one::<String>("starting_slot").unwrap().parse().unwrap();
            let ending_slot = arg_matches.get_one::<String>("ending_slot")
                .map(|s| s.parse().unwrap()).unwrap_or(Slot::MAX);
            let num_slots = arg_matches.get_one::<String>("num_slots")
                .map(|s| s.parse().ok()).flatten();
            let allow_dead_slots = arg_matches.get_flag("allow_dead_slots");
            let only_rooted = arg_matches.get_flag("only_rooted");
            let output_format = if arg_matches.get_one::<String>("output_format").map(|s| s.as_str()) == Some("json") { OutputFormat::Json } else { OutputFormat::Display };

            output_ledger(
                crate::open_blockstore(&ledger_path, arg_matches, AccessType::Secondary),
                starting_slot,
                ending_slot,
                allow_dead_slots,
                output_format,
                num_slots,
                verbose_level as u64,
                only_rooted,
            )?;
        }
        Some(("print-file-metadata", arg_matches)) => {
            let blockstore =
                crate::open_blockstore(&ledger_path, arg_matches, AccessType::Secondary);
            let sst_file_name = arg_matches.get_one::<String>("file_name").map(|s| s.as_str());
            print_blockstore_file_metadata(&blockstore, &sst_file_name)?;
        }
        Some(("purge", arg_matches)) => {
            let start_slot = arg_matches.get_one::<String>("start_slot").unwrap().parse().unwrap();
            let end_slot = arg_matches.get_one::<String>("end_slot").map(|s| s.parse::<Slot>().unwrap());
            let perform_compaction = arg_matches.get_flag("enable_compaction");
            if arg_matches.get_flag("no_compaction") {
                warn!("--no-compaction is deprecated and is now the default behavior.");
            }
            let dead_slots_only = arg_matches.get_flag("dead_slots_only");
            let batch_size = arg_matches.get_one::<String>("batch_size").unwrap().parse().unwrap();

            let blockstore = crate::open_blockstore(
                &ledger_path,
                arg_matches,
                AccessType::PrimaryForMaintenance,
            );

            let Some(highest_slot) = blockstore.highest_slot()? else {
                return Err(LedgerToolError::BadArgument(
                    "blockstore is empty".to_string(),
                ));
            };

            let end_slot = if let Some(slot) = end_slot {
                std::cmp::min(slot, highest_slot)
            } else {
                highest_slot
            };
            if end_slot < start_slot {
                return Err(LedgerToolError::BadArgument(format!(
                    "starting slot {start_slot} should be less than or equal to ending slot \
                     {end_slot}"
                )));
            }

            info!(
                "Purging data from slots {} to {} ({} slots) (do compaction: {}) (dead slot only: \
                 {})",
                start_slot,
                end_slot,
                end_slot - start_slot,
                perform_compaction,
                dead_slots_only,
            );
            let purge_from_blockstore = |start_slot, end_slot| {
                blockstore.purge_from_next_slots(start_slot, end_slot);
                if perform_compaction {
                    blockstore.purge_and_compact_slots(start_slot, end_slot);
                } else {
                    blockstore.purge_slots(start_slot, end_slot, PurgeType::Exact);
                }
            };
            if !dead_slots_only {
                let slots_iter = &(start_slot..=end_slot).chunks(batch_size);
                for slots in slots_iter {
                    let slots = slots.collect::<Vec<_>>();
                    assert!(!slots.is_empty());

                    let start_slot = *slots.first().unwrap();
                    let end_slot = *slots.last().unwrap();
                    info!(
                        "Purging chunked slots from {} to {} ({} slots)",
                        start_slot,
                        end_slot,
                        end_slot - start_slot
                    );
                    purge_from_blockstore(start_slot, end_slot);
                }
            } else {
                let dead_slots_iter = blockstore
                    .dead_slots_iterator(start_slot)?
                    .take_while(|s| *s <= end_slot);
                for dead_slot in dead_slots_iter {
                    info!("Purging dead slot {dead_slot}");
                    purge_from_blockstore(dead_slot, dead_slot);
                }
            }
        }
        Some(("remove-dead-slot", arg_matches)) => {
            let slots = arg_matches.get_many::<String>("slots").unwrap_or_else(|| std::process::exit(1)).map(|s| s.parse::<Slot>().unwrap()).collect::<Vec<_>>();
            let blockstore = crate::open_blockstore(
                &ledger_path,
                arg_matches,
                AccessType::PrimaryForMaintenance,
            );
            for slot in slots {
                blockstore
                    .remove_dead_slot(slot)
                    .map(|_| println!("Slot {slot} not longer marked dead"))?;
            }
        }
        Some(("repair-roots", arg_matches)) => {
            let blockstore = crate::open_blockstore(
                &ledger_path,
                arg_matches,
                AccessType::PrimaryForMaintenance,
            );

            let start_root =
                arg_matches.get_one::<String>("start_root").map(|s| s.parse::<Slot>().unwrap()).unwrap_or_else(|| blockstore.max_root());
            let max_slots = arg_matches.get_one::<String>("max_slots").unwrap().parse().unwrap();
            let end_root = arg_matches.get_one::<String>("end_root").map(|s| s.parse::<Slot>().unwrap())
                .unwrap_or_else(|| start_root.saturating_sub(max_slots));
            assert!(start_root > end_root);
            // Adjust by one since start_root need not be checked
            let num_slots = start_root - end_root - 1;
            if arg_matches.get_flag("end_root") && num_slots > max_slots {
                return Err(LedgerToolError::BadArgument(format!(
                    "Requested range {num_slots} too large, max {max_slots}. Either adjust \
                     `--until` value, or pass a larger `--repair-limit` to override the limit",
                )));
            }

            let num_repaired_roots = blockstore.scan_and_fix_roots(
                Some(start_root),
                Some(end_root),
                &AtomicBool::new(false),
            )?;
            println!("Successfully repaired {num_repaired_roots} roots");
        }
        Some(("set-dead-slot", arg_matches)) => {
            let slots = arg_matches.get_many::<String>("slots").unwrap_or_else(|| std::process::exit(1)).map(|s| s.parse::<Slot>().unwrap()).collect::<Vec<_>>();
            let blockstore = crate::open_blockstore(
                &ledger_path,
                arg_matches,
                AccessType::PrimaryForMaintenance,
            );
            for slot in slots {
                blockstore
                    .set_dead_slot(slot)
                    .map(|_| println!("Slot {slot} marked dead"))?;
            }
        }
        Some(("shred-meta", arg_matches)) => {
            #[derive(Debug)]
            #[allow(dead_code)]
            struct ShredMeta<'a> {
                slot: Slot,
                full_slot: bool,
                shred_index: usize,
                data: bool,
                code: bool,
                last_in_slot: bool,
                data_complete: bool,
                shred: &'a Shred,
            }
            let starting_slot = arg_matches.get_one::<String>("starting_slot").unwrap().parse().unwrap();
            let ending_slot = arg_matches.get_one::<String>("ending_slot").map(|s| s.parse::<Slot>().unwrap()).unwrap_or(Slot::MAX);
            let ledger = crate::open_blockstore(&ledger_path, arg_matches, AccessType::Secondary);
            for (slot, _meta) in ledger
                .slot_meta_iterator(starting_slot)?
                .take_while(|(slot, _)| *slot <= ending_slot)
            {
                let full_slot = ledger.is_full(slot);
                if let Ok(shreds) = ledger.get_data_shreds_for_slot(slot, 0) {
                    for (shred_index, shred) in shreds.iter().enumerate() {
                        println!(
                            "{:#?}",
                            ShredMeta {
                                slot,
                                full_slot,
                                shred_index,
                                data: shred.is_data(),
                                code: shred.is_code(),
                                data_complete: shred.data_complete(),
                                last_in_slot: shred.last_in_slot(),
                                shred,
                            }
                        );
                    }
                }
            }
        }
        Some(("slot", arg_matches)) => {
            let slots = arg_matches.get_many::<String>("slots").unwrap_or_else(|| std::process::exit(1)).map(|s| s.parse::<Slot>().unwrap()).collect::<Vec<_>>();
            let allow_dead_slots = arg_matches.get_flag("allow_dead_slots");
            let output_format = if arg_matches.get_one::<String>("output_format").map(|s| s.as_str()) == Some("json") { OutputFormat::Json } else { OutputFormat::Display };

            let blockstore =
                crate::open_blockstore(&ledger_path, arg_matches, AccessType::Secondary);
            for slot in slots {
                output_slot(
                    &blockstore,
                    slot,
                    allow_dead_slots,
                    &output_format,
                    verbose_level as u64,
                    &mut HashMap::new(),
                )?;
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use {
        super::*,
        solana_ledger::{blockstore::make_many_slot_entries, get_tmp_ledger_path_auto_delete},
    };

    #[test]
    fn test_latest_optimistic_ancestors() {
        let ledger_path = get_tmp_ledger_path_auto_delete!();
        let blockstore = Blockstore::open(ledger_path.path()).unwrap();

        // Insert 5 slots into blockstore
        let start_slot = 0;
        let num_slots = 5;
        let entries_per_shred = 5;
        let (shreds, _) = make_many_slot_entries(start_slot, num_slots, entries_per_shred);
        blockstore.insert_shreds(shreds, None, false).unwrap();

        // Mark even shreds as optimistically confirmed
        (0..num_slots).step_by(2).for_each(|slot| {
            blockstore
                .insert_optimistic_slot(slot, &Hash::default(), UnixTimestamp::default())
                .unwrap();
        });

        let exclude_vote_only_slots = false;
        let optimistic_slots: Vec<_> =
            get_latest_optimistic_slots(&blockstore, num_slots as usize, exclude_vote_only_slots)
                .iter()
                .map(|(slot, _, _)| *slot)
                .collect();

        // Should see all slots here since they're all chained, despite only evens getting marked
        // get_latest_optimistic_slots() returns slots in descending order so use .rev()
        let expected: Vec<_> = (start_slot..num_slots).rev().collect();
        assert_eq!(optimistic_slots, expected);
    }
}
