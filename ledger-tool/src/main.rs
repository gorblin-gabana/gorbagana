#![allow(clippy::arithmetic_side_effects)]
use {
    crate::{
        args::*,
        bigtable::*,
        blockstore::*,
        ledger_path::*,
        ledger_utils::*,
        output::{
            AccountsOutputConfig, AccountsOutputMode, AccountsOutputStreamer, CliAccounts,
            SlotBankHash,
        },
        program::*,
    },
    agave_feature_set::{self as feature_set, FeatureSet},
    agave_reserved_account_keys::ReservedAccountKeys,
    clap::{
        crate_description, crate_name,
        Arg, ArgMatches, Command as ClapCommand, ArgAction,
    },
    dashmap::DashMap,
    log::*,
    serde_derive::Serialize,
    solana_account::{state_traits::StateMut, AccountSharedData, ReadableAccount, WritableAccount},
    solana_accounts_db::accounts_index::{ScanConfig, ScanOrder},
    solana_clap_utils::{
        input_parsers::{cluster_type_of, pubkey_of, pubkeys_of},
        input_validators::{
            is_parsable, is_pubkey, is_pubkey_or_keypair, is_slot, is_valid_percentage,
            is_within_range,
        },
    },
    solana_cli_output::{CliAccount, OutputFormat},
    solana_clock::{Epoch, Slot},
    solana_core::{
        banking_simulation::{BankingSimulator, BankingTraceEvents},
        system_monitor_service::{SystemMonitorService, SystemMonitorStatsReportConfig},
        validator::{BlockProductionMethod, BlockVerificationMethod, TransactionStructure},
    },
    solana_cost_model::{cost_model::CostModel, cost_tracker::CostTracker},
    solana_feature_gate_interface::{self as feature, Feature},
    solana_genesis_config::ClusterType,
    solana_inflation::Inflation,
    solana_instruction::TRANSACTION_LEVEL_STACK_HEIGHT,
    solana_ledger::{
        blockstore::{banking_trace_path, create_new_ledger, Blockstore},
        blockstore_options::{AccessType, LedgerColumnOptions},
        blockstore_processor::{
            ProcessSlotCallback, TransactionStatusMessage, TransactionStatusSender,
        },
    },
    solana_measure::{measure::Measure, measure_time},
    solana_message::SimpleAddressLoader,
    solana_native_token::{lamports_to_sol, sol_to_lamports, Sol},
    solana_pubkey::Pubkey,
    solana_rent::Rent,
    solana_runtime::{
        bank::{
            bank_hash_details::{self, SlotDetails, TransactionDetails},
            Bank, RewardCalculationEvent,
        },
        bank_forks::BankForks,
        inflation_rewards::points::{InflationPointCalculationEvent, PointValue},
        snapshot_archive_info::SnapshotArchiveInfoGetter,
        snapshot_bank_utils,
        snapshot_minimizer::SnapshotMinimizer,
        snapshot_utils::{
            ArchiveFormat, SnapshotVersion, DEFAULT_ARCHIVE_COMPRESSION,
            SUPPORTED_ARCHIVE_COMPRESSION,
        },
    },
    solana_runtime_transaction::runtime_transaction::RuntimeTransaction,
    solana_shred_version::compute_shred_version,
    solana_stake_interface::{self as stake, state::StakeStateV2},
    solana_stake_program::stake_state,
    solana_system_interface::program as system_program,
    solana_transaction::sanitized::MessageHash,
    solana_transaction_status::parse_ui_instruction,
    solana_unified_scheduler_pool::DefaultSchedulerPool,
    solana_vote::vote_state_view::VoteStateView,
    solana_vote_program::{
        self,
        vote_state::{self, VoteStateV3},
    },
    std::{
        collections::{HashMap, HashSet},
        ffi::{OsStr, OsString},
        fs::{read_dir, File},
        io::{self, Write},
        mem::swap,
        path::{Path, PathBuf},
        process::{exit, Command, Stdio},
        str::FromStr,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex, RwLock,
        },
        thread::JoinHandle,
    },
};

mod args;
mod bigtable;
mod blockstore;
mod error;
mod ledger_path;
mod ledger_utils;
mod output;
mod program;

fn render_dot(dot: String, output_file: &str, output_format: &str) -> io::Result<()> {
    let mut child = Command::new("dot")
        .arg(format!("-T{output_format}"))
        .arg(format!("-o{output_file}"))
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|err| {
            eprintln!("Failed to spawn dot: {err:?}");
            err
        })?;

    let stdin = child.stdin.as_mut().unwrap();
    stdin.write_all(&dot.into_bytes())?;

    let status = child.wait_with_output()?.status;
    if !status.success() {
        return Err(io::Error::other(format!(
            "dot failed with error {}",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum GraphVoteAccountMode {
    Disabled,
    LastOnly,
    WithHistory,
}

impl std::fmt::Display for GraphVoteAccountMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => write!(f, "{}", Self::DISABLED),
            Self::LastOnly => write!(f, "{}", Self::LAST_ONLY),
            Self::WithHistory => write!(f, "{}", Self::WITH_HISTORY),
        }
    }
}

impl GraphVoteAccountMode {
    const DISABLED: &'static str = "disabled";
    const LAST_ONLY: &'static str = "last-only";
    const WITH_HISTORY: &'static str = "with-history";
    const ALL_MODE_STRINGS: &'static [&'static str] =
        &[Self::DISABLED, Self::LAST_ONLY, Self::WITH_HISTORY];

    fn is_enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

impl AsRef<str> for GraphVoteAccountMode {
    fn as_ref(&self) -> &str {
        match self {
            Self::Disabled => Self::DISABLED,
            Self::LastOnly => Self::LAST_ONLY,
            Self::WithHistory => Self::WITH_HISTORY,
        }
    }
}

impl Default for GraphVoteAccountMode {
    fn default() -> Self {
        Self::Disabled
    }
}

#[derive(Debug)]
struct GraphVoteAccountModeError;

impl FromStr for GraphVoteAccountMode {
    type Err = GraphVoteAccountModeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            Self::DISABLED => Ok(Self::Disabled),
            Self::LAST_ONLY => Ok(Self::LastOnly),
            Self::WITH_HISTORY => Ok(Self::WithHistory),
            _ => Err(GraphVoteAccountModeError),
        }
    }
}

struct GraphConfig {
    include_all_votes: bool,
    vote_account_mode: GraphVoteAccountMode,
}

#[allow(clippy::cognitive_complexity)]
fn graph_forks(bank_forks: &BankForks, config: &GraphConfig) -> String {
    let frozen_banks = bank_forks.frozen_banks();
    let mut fork_slots: HashSet<_> = bank_forks
        .frozen_banks()
        .map(|(slot, _bank)| slot)
        .collect();
    for (_, bank) in frozen_banks {
        for parent in bank.parents() {
            fork_slots.remove(&parent.slot());
        }
    }

    // Search all forks and collect the last vote made by each validator
    let mut last_votes = HashMap::new();
    for fork_slot in &fork_slots {
        let bank = &bank_forks[*fork_slot];

        let total_stake = bank
            .vote_accounts()
            .iter()
            .map(|(_, (stake, _))| stake)
            .sum();
        for (stake, vote_account) in bank.vote_accounts().values() {
            let vote_state_view = vote_account.vote_state_view();
            if let Some(last_vote) = vote_state_view.last_voted_slot() {
                let entry = last_votes.entry(*vote_state_view.node_pubkey()).or_insert((
                    last_vote,
                    vote_state_view.clone(),
                    *stake,
                    total_stake,
                ));
                if entry.0 < last_vote {
                    *entry = (last_vote, vote_state_view.clone(), *stake, total_stake);
                }
            }
        }
    }

    // Figure the stake distribution at all the nodes containing the last vote from each
    // validator
    let mut slot_stake_and_vote_count = HashMap::new();
    for (last_vote_slot, _, stake, total_stake) in last_votes.values() {
        let entry = slot_stake_and_vote_count
            .entry(last_vote_slot)
            .or_insert((0, 0, *total_stake));
        entry.0 += 1;
        entry.1 += stake;
        assert_eq!(entry.2, *total_stake)
    }

    let mut dot = vec!["digraph {".to_string()];

    // Build a subgraph consisting of all banks and links to their parent banks
    dot.push("  subgraph cluster_banks {".to_string());
    dot.push("    style=invis".to_string());
    let mut styled_slots = HashSet::new();
    let mut all_votes: HashMap<Pubkey, HashMap<Slot, VoteStateView>> = HashMap::new();
    for fork_slot in &fork_slots {
        let mut bank = bank_forks[*fork_slot].clone();

        let mut first = true;
        loop {
            for (_, vote_account) in bank.vote_accounts().values() {
                let vote_state_view = vote_account.vote_state_view();
                if let Some(last_vote) = vote_state_view.last_voted_slot() {
                    let validator_votes =
                        all_votes.entry(*vote_state_view.node_pubkey()).or_default();
                    validator_votes
                        .entry(last_vote)
                        .or_insert_with(|| vote_state_view.clone());
                }
            }

            if !styled_slots.contains(&bank.slot()) {
                dot.push(format!(
                    r#"    "{}"[label="{} (epoch {})\nleader: {}{}{}",style="{}{}"];"#,
                    bank.slot(),
                    bank.slot(),
                    bank.epoch(),
                    bank.collector_id(),
                    if let Some(parent) = bank.parent() {
                        format!(
                            "\ntransactions: {}",
                            bank.transaction_count() - parent.transaction_count(),
                        )
                    } else {
                        "".to_string()
                    },
                    if let Some((votes, stake, total_stake)) =
                        slot_stake_and_vote_count.get(&bank.slot())
                    {
                        format!(
                            "\nvotes: {}, stake: {:.1} SOL ({:.1}%)",
                            votes,
                            lamports_to_sol(*stake),
                            *stake as f64 / *total_stake as f64 * 100.,
                        )
                    } else {
                        "".to_string()
                    },
                    if first { "filled," } else { "" },
                    ""
                ));
                styled_slots.insert(bank.slot());
            }
            first = false;

            match bank.parent() {
                None => {
                    if bank.slot() > 0 {
                        dot.push(format!(r#"    "{}" -> "..." [dir=back]"#, bank.slot(),));
                    }
                    break;
                }
                Some(parent) => {
                    let slot_distance = bank.slot() - parent.slot();
                    let penwidth = if bank.epoch() > parent.epoch() {
                        "5"
                    } else {
                        "1"
                    };
                    let link_label = if slot_distance > 1 {
                        format!("label=\"{} slots\",color=red", slot_distance - 1)
                    } else {
                        "color=blue".to_string()
                    };
                    dot.push(format!(
                        r#"    "{}" -> "{}"[{},dir=back,penwidth={}];"#,
                        bank.slot(),
                        parent.slot(),
                        link_label,
                        penwidth
                    ));

                    bank = parent.clone();
                }
            }
        }
    }
    dot.push("  }".to_string());

    // Strafe the banks with links from validators to the bank they last voted on,
    // while collecting information about the absent votes and stakes
    let mut absent_stake = 0;
    let mut absent_votes = 0;
    let mut lowest_last_vote_slot = u64::MAX;
    let mut lowest_total_stake = 0;
    for (node_pubkey, (last_vote_slot, vote_state_view, stake, total_stake)) in &last_votes {
        all_votes.entry(*node_pubkey).and_modify(|validator_votes| {
            validator_votes.remove(last_vote_slot);
        });

        let maybe_styled_last_vote_slot = styled_slots.get(last_vote_slot);
        if maybe_styled_last_vote_slot.is_none() {
            if *last_vote_slot < lowest_last_vote_slot {
                lowest_last_vote_slot = *last_vote_slot;
                lowest_total_stake = *total_stake;
            }
            absent_votes += 1;
            absent_stake += stake;
        };

        if config.vote_account_mode.is_enabled() {
            let vote_history =
                if matches!(config.vote_account_mode, GraphVoteAccountMode::WithHistory) {
                    format!(
                        "vote history:\n{}",
                        vote_state_view
                            .votes_iter()
                            .map(|vote| format!(
                                "slot {} (conf={})",
                                vote.slot(),
                                vote.confirmation_count()
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                } else {
                    format!(
                        "last vote slot: {}",
                        vote_state_view
                            .last_voted_slot()
                            .map(|vote_slot| vote_slot.to_string())
                            .unwrap_or_else(|| "none".to_string())
                    )
                };
            dot.push(format!(
                r#"  "last vote {}"[shape=box,label="Latest validator vote: {}\nstake: {} SOL\nroot slot: {}\n{}"];"#,
                node_pubkey,
                node_pubkey,
                lamports_to_sol(*stake),
                vote_state_view.root_slot().unwrap_or(0),
                vote_history,
            ));

            dot.push(format!(
                r#"  "last vote {}" -> "{}" [style=dashed,label="latest vote"];"#,
                node_pubkey,
                if let Some(styled_last_vote_slot) = maybe_styled_last_vote_slot {
                    styled_last_vote_slot.to_string()
                } else {
                    "...".to_string()
                },
            ));
        }
    }

    // Annotate the final "..." node with absent vote and stake information
    if absent_votes > 0 {
        dot.push(format!(
            r#"    "..."[label="...\nvotes: {}, stake: {:.1} SOL {:.1}%"];"#,
            absent_votes,
            lamports_to_sol(absent_stake),
            absent_stake as f64 / lowest_total_stake as f64 * 100.,
        ));
    }

    // Add for vote information from all banks.
    if config.include_all_votes {
        for (node_pubkey, validator_votes) in &all_votes {
            for (vote_slot, vote_state_view) in validator_votes {
                dot.push(format!(
                    r#"  "{} vote {}"[shape=box,style=dotted,label="validator vote: {}\nroot slot: {}\nvote history:\n{}"];"#,
                    node_pubkey,
                    vote_slot,
                    node_pubkey,
                    vote_state_view.root_slot().unwrap_or(0),
                    vote_state_view
                        .votes_iter()
                        .map(|vote| format!("slot {} (conf={})", vote.slot(), vote.confirmation_count()))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));

                dot.push(format!(
                    r#"  "{} vote {}" -> "{}" [style=dotted,label="vote"];"#,
                    node_pubkey,
                    vote_slot,
                    if styled_slots.contains(vote_slot) {
                        vote_slot.to_string()
                    } else {
                        "...".to_string()
                    },
                ));
            }
        }
    }

    dot.push("}".to_string());
    dot.join("\n")
}

fn compute_slot_cost(
    blockstore: &Blockstore,
    slot: Slot,
    allow_dead_slots: bool,
) -> Result<(), String> {
    let (entries, _num_shreds, _is_full) = blockstore
        .get_slot_entries_with_shred_info(slot, 0, allow_dead_slots)
        .map_err(|err| format!("Slot: {slot}, Failed to load entries, err {err:?}"))?;

    let num_entries = entries.len();
    let mut num_transactions = 0;
    let mut num_programs = 0;

    let mut program_ids = HashMap::new();
    let mut cost_tracker = CostTracker::default();

    let feature_set = FeatureSet::all_enabled();
    let reserved_account_keys = ReservedAccountKeys::new_all_activated();

    for entry in entries {
        num_transactions += entry.transactions.len();
        entry
            .transactions
            .into_iter()
            .filter_map(|transaction| {
                RuntimeTransaction::try_create(
                    transaction,
                    MessageHash::Compute,
                    None,
                    SimpleAddressLoader::Disabled,
                    &reserved_account_keys.active,
                )
                .map_err(|err| {
                    warn!("Failed to compute cost of transaction: {err:?}");
                })
                .ok()
            })
            .for_each(|transaction| {
                num_programs += transaction.message().instructions().len();

                let tx_cost = CostModel::calculate_cost(&transaction, &feature_set);
                let result = cost_tracker.try_add(&tx_cost);
                if result.is_err() {
                    println!(
                        "Slot: {slot}, CostModel rejected transaction {transaction:?}, reason \
                         {result:?}",
                    );
                }
                for (program_id, _instruction) in transaction.message().program_instructions_iter()
                {
                    *program_ids.entry(*program_id).or_insert(0) += 1;
                }
            });
    }

    println!(
        "Slot: {slot}, Entries: {num_entries}, Transactions: {num_transactions}, Programs \
         {num_programs}",
    );
    println!("  Programs: {program_ids:?}");

    Ok(())
}

/// Finds the accounts needed to replay slots `snapshot_slot` to `ending_slot`.
/// Removes all other accounts from accounts_db, and updates the accounts hash
/// and capitalization. This is used by the --minimize option in create-snapshot
/// Returns true if the minimized snapshot may be incomplete.
fn minimize_bank_for_snapshot(
    blockstore: &Blockstore,
    bank: &Bank,
    snapshot_slot: Slot,
    ending_slot: Slot,
) -> bool {
    let ((transaction_account_set, possibly_incomplete), transaction_accounts_measure) = measure_time!(
        blockstore.get_accounts_used_in_range(bank, snapshot_slot, ending_slot),
        "get transaction accounts"
    );
    let total_accounts_len = transaction_account_set.len();
    info!("Added {total_accounts_len} accounts from transactions. {transaction_accounts_measure}");

    SnapshotMinimizer::minimize(bank, snapshot_slot, transaction_account_set);
    possibly_incomplete
}

fn assert_capitalization(bank: &Bank) {
    let calculated = bank.calculate_capitalization_for_tests();
    let expected = bank.capitalization();
    assert_eq!(
        calculated, expected,
        "Capitalization mismatch: calculated: {calculated} != expected: {expected}",
    );
}

fn load_banking_trace_events_or_exit(ledger_path: &Path) -> BankingTraceEvents {
    let file_paths = read_banking_trace_event_file_paths_or_exit(banking_trace_path(ledger_path));

    info!("Using: banking trace event files: {file_paths:?}");
    match BankingTraceEvents::load(&file_paths) {
        Ok(banking_trace_events) => banking_trace_events,
        Err(error) => {
            eprintln!("Failed to load banking trace events: {error:?}");
            exit(1)
        }
    }
}

fn read_banking_trace_event_file_paths_or_exit(banking_trace_path: PathBuf) -> Vec<PathBuf> {
    info!("Using: banking trace events dir: {banking_trace_path:?}");

    let entries = match read_dir(&banking_trace_path) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("Error: failed to open banking_trace_path: {error:?}");
            exit(1);
        }
    };

    let mut entry_names = entries
        .flat_map(|entry| entry.ok().map(|entry| entry.file_name()))
        .collect::<HashSet<OsString>>();

    let mut event_file_paths = vec![];

    if entry_names.is_empty() {
        warn!("banking_trace_path dir is empty.");
        return event_file_paths;
    }

    for index in 0.. {
        let event_file_name: OsString = BankingSimulator::event_file_name(index).into();
        if entry_names.remove(&event_file_name) {
            event_file_paths.push(banking_trace_path.join(event_file_name));
        } else {
            break;
        }
    }

    if event_file_paths.is_empty() {
        warn!("Error: no event files found");
    }

    if !entry_names.is_empty() {
        let full_names = entry_names
            .into_iter()
            .map(|name| banking_trace_path.join(name))
            .collect::<Vec<_>>();
        warn!(
            "Some files in {banking_trace_path:?} is ignored due to gapped events file rotation \
             or unrecognized names: {full_names:?}"
        );
    }

    // Reverse to load in the chronicle order (note that this isn't strictly needed)
    event_file_paths.reverse();
    event_file_paths
}

struct SlotRecorderConfig {
    transaction_recorder: Option<JoinHandle<()>>,
    transaction_status_sender: Option<TransactionStatusSender>,
    slot_details: Arc<Mutex<Vec<SlotDetails>>>,
    file: File,
}

fn setup_slot_recording(
    arg_matches: &ArgMatches,
) -> (Option<ProcessSlotCallback>, Option<SlotRecorderConfig>) {
    let record_slots = arg_matches.get_count("record_slots") > 0;
    let verify_slots = arg_matches.get_count("verify_slots") > 0;
    match (record_slots, verify_slots) {
        (false, false) => (None, None),
        (true, true) => {
            // .default_value() does not work with .conflicts_with() in clap 2.33
            // .conflicts_with("verify_slots")
            // https://github.com/clap-rs/clap/issues/1605#issuecomment-722326915
            // So open-code the conflicts_with() here
            eprintln!(
                "error: The argument '--verify-slots <FILENAME>' cannot be used with \
                 '--record-slots <FILENAME>'"
            );
            exit(1);
        }
        (true, false) => {
            let filename = Path::new(arg_matches.get_one::<std::ffi::OsString>("record_slots").unwrap());
            let file = File::create(filename).unwrap_or_else(|err| {
                eprintln!("Unable to write to file: {}: {:#}", filename.display(), err);
                exit(1);
            });

            let mut include_bank_hash_components = false;
            let mut include_tx = false;
            if let Some(args) = arg_matches.get_many::<String>("record_slots_config") {
                for arg in args {
                    match arg.as_str() {
                        "tx" => include_tx = true,
                        "accounts" => include_bank_hash_components = true,
                        _ => unreachable!(),
                    }
                }
            }

            let slot_details = Arc::new(Mutex::new(Vec::new()));
            let (transaction_status_sender, transaction_recorder) = if include_tx {
                let (sender, receiver) = crossbeam_channel::unbounded();

                let slots = Arc::clone(&slot_details);
                let transaction_recorder = Some(std::thread::spawn(move || {
                    record_transactions(receiver, slots);
                }));

                (
                    Some(TransactionStatusSender { sender }),
                    transaction_recorder,
                )
            } else {
                (None, None)
            };

            let slot_callback = Arc::new({
                let slots = Arc::clone(&slot_details);
                move |bank: &Bank| {
                    let mut details = bank_hash_details::SlotDetails::new_from_bank(
                        bank,
                        include_bank_hash_components,
                    )
                    .unwrap();
                    let mut slots = slots.lock().unwrap();

                    if let Some(recorded_slot) = slots.iter_mut().find(|f| f.slot == details.slot) {
                        // copy all fields except transactions
                        swap(&mut recorded_slot.transactions, &mut details.transactions);

                        *recorded_slot = details;
                    } else {
                        slots.push(details);
                    }
                }
            });

            (
                Some(slot_callback as ProcessSlotCallback),
                Some(SlotRecorderConfig {
                    transaction_recorder,
                    transaction_status_sender,
                    slot_details,
                    file,
                }),
            )
        }
        (false, true) => {
            let filename = Path::new(arg_matches.get_one::<std::ffi::OsString>("verify_slots").unwrap());
            let file = File::open(filename).unwrap_or_else(|err| {
                eprintln!("Unable to read file: {}: {err:#}", filename.display());
                exit(1);
            });
            let reader = std::io::BufReader::new(file);
            let details: bank_hash_details::BankHashDetails = serde_json::from_reader(reader)
                .unwrap_or_else(|err| {
                    eprintln!("Error loading slots file: {err:#}");
                    exit(1);
                });

            let slots = Arc::new(Mutex::new(details.bank_hash_details));
            let slot_callback = Arc::new(move |bank: &Bank| {
                if slots.lock().unwrap().is_empty() {
                    error!(
                        "Expected slot: not found got slot: {} hash: {}",
                        bank.slot(),
                        bank.hash()
                    );
                } else {
                    let bank_hash_details::SlotDetails {
                        slot: expected_slot,
                        bank_hash: expected_hash,
                        ..
                    } = slots.lock().unwrap().remove(0);
                    if bank.slot() != expected_slot || bank.hash().to_string() != expected_hash {
                        error!(
                            "Expected slot: {expected_slot} hash: {expected_hash} got slot: {} \
                             hash: {}",
                            bank.slot(),
                            bank.hash()
                        );
                    } else {
                        info!("Expected slot: {expected_slot} hash: {expected_hash} correct");
                    }
                }
            });

            (Some(slot_callback as ProcessSlotCallback), None)
        }
    }
}

fn record_transactions(
    recv: crossbeam_channel::Receiver<TransactionStatusMessage>,
    slots: Arc<Mutex<Vec<SlotDetails>>>,
) {
    for tsm in recv {
        if let TransactionStatusMessage::Batch(batch) = tsm {
            assert_eq!(batch.transactions.len(), batch.commit_results.len());

            let transactions: Vec<_> = batch
                .transactions
                .iter()
                .zip(batch.commit_results)
                .zip(batch.transaction_indexes)
                .map(|((tx, commit_result), index)| {
                    let message = tx.message();

                    let accounts: Vec<String> = message
                        .account_keys()
                        .iter()
                        .map(|acc| acc.to_string())
                        .collect();

                    let instructions = message
                        .instructions()
                        .iter()
                        .map(|ix| {
                            parse_ui_instruction(
                                ix,
                                &message.account_keys(),
                                Some(TRANSACTION_LEVEL_STACK_HEIGHT as u32),
                            )
                        })
                        .collect();

                    let is_simple_vote_tx = tx.is_simple_vote_transaction();
                    let commit_details = commit_result.ok().map(|committed_tx| committed_tx.into());

                    TransactionDetails {
                        signature: tx.signature().to_string(),
                        accounts,
                        instructions,
                        is_simple_vote_tx,
                        commit_details,
                        index,
                    }
                })
                .collect();

            let mut slots = slots.lock().unwrap();

            if let Some(recorded_slot) = slots.iter_mut().find(|f| f.slot == batch.slot) {
                recorded_slot.transactions.extend(transactions);
            } else {
                slots.push(SlotDetails {
                    slot: batch.slot,
                    transactions,
                    ..Default::default()
                });
            }
        }
    }

    for slot in slots.lock().unwrap().iter_mut() {
        slot.transactions.sort_by(|a, b| a.index.cmp(&b.index));
    }
}

#[cfg(not(any(target_env = "msvc", target_os = "freebsd")))]
use jemallocator::Jemalloc;

#[cfg(not(any(target_env = "msvc", target_os = "freebsd")))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[allow(clippy::cognitive_complexity)]
fn main() {
    // Ignore SIGUSR1 to prevent long-running calls being killed by logrotate
    // in warehouse deployments
    #[cfg(unix)]
    {
        // `register()` is unsafe because the action is called in a signal handler
        // with the usual caveats. So long as this action body stays empty, we'll
        // be fine
        unsafe { signal_hook::low_level::register(signal_hook::consts::SIGUSR1, || {}) }.unwrap();
    }

    solana_logger::setup_with_default_filter();

    let load_genesis_config_arg = load_genesis_arg();
    let accounts_db_config_args = accounts_db_args();
    let snapshot_config_args = snapshot_args();

    let halt_at_slot_arg = Arg::new("halt_at_slot")
        .long("halt-at-slot")
        .value_name("SLOT")
        .value_parser(clap::value_parser!(u64))
        
        .help("Halt processing at the given slot");
    let os_memory_stats_reporting_arg = Arg::new("os_memory_stats_reporting")
        .long("os-memory-stats-reporting")
        .help("Enable reporting of OS memory statistics.");
    let verify_index_arg = Arg::new("verify_accounts_index")
        .long("verify-accounts-index")
        .action(ArgAction::SetTrue)
        .help("For debugging and tests on accounts index.");
    let limit_load_slot_count_from_snapshot_arg =
        Arg::new("limit_load_slot_count_from_snapshot")
            .long("limit-load-slot-count-from-snapshot")
            .value_name("SLOT")
            .value_parser(clap::value_parser!(u64))
            
            .help(
                "For debugging and profiling with large snapshots, artificially limit how many \
                 slots are loaded from a snapshot.",
            );
    let hard_forks_arg = Arg::new("hard_forks")
        .long("hard-fork")
        .value_name("SLOT")
        .value_parser(clap::value_parser!(u64))
        .action(ArgAction::Append)
        
        .help("Add a hard fork at this slot");
    let allow_dead_slots_arg = Arg::new("allow_dead_slots")
        .long("allow-dead-slots")
        .action(ArgAction::SetTrue)
        .help("Output dead slots as well");
    let hashes_per_tick = Arg::new("hashes_per_tick")
        .long("hashes-per-tick")
        .value_name("NUM_HASHES|\"sleep\"")
        
        .help(
            "How many PoH hashes to roll before emitting the next tick. If \"sleep\", for \
             development sleep for the target tick duration instead of hashing",
        );
    let snapshot_version_arg = Arg::new("snapshot_version")
        .long("snapshot-version")
        .value_name("SNAPSHOT_VERSION")
        .value_parser(clap::value_parser!(String))
        .default_value(Box::leak(Box::new(SnapshotVersion::default().to_string())).as_str())
        .help("Output snapshot version");
    let debug_key_arg = Arg::new("debug_key")
        .long("debug-key")
        .value_parser(clap::value_parser!(String))
        .value_name("ADDRESS")
        .action(ArgAction::Append)
        
        .help("Log when transactions are processed that reference the given key(s).");

    let geyser_plugin_args = Arg::new("geyser_plugin_config")
        .long("geyser-plugin-config")
        .value_name("FILE")
        
        .action(ArgAction::Append)
        .help("Specify the configuration file for the Geyser plugin.");

    let log_messages_bytes_limit_arg = Arg::new("log_messages_bytes_limit")
        .long("log-messages-bytes-limit")
        
        .value_parser(clap::value_parser!(usize))
        .value_name("BYTES")
        .help("Maximum number of bytes written to the program log before truncation");

    let accounts_data_encoding_arg = Arg::new("encoding")
        .long("encoding")
        
        .value_parser(["base64", "base64+zstd", "jsonParsed"])
        .default_value("base64")
        .help("Print account data in specified format when printing account contents.");

    let rent = Rent::default();
    let default_bootstrap_validator_lamports = sol_to_lamports(500.0)
        .max(VoteStateV3::get_rent_exempt_reserve(&rent))
        .to_string();
    let default_bootstrap_validator_stake_lamports = sol_to_lamports(0.5)
        .max(rent.minimum_balance(StakeStateV2::size_of()))
        .to_string();
    let default_graph_vote_account_mode = GraphVoteAccountMode::default();

    let mut measure_total_execution_time = Measure::start("ledger tool");

    let cli_matches = ClapCommand::new(crate_name!())
        .about(crate_description!())
        .version(Box::leak(Box::new(solana_version::version!().to_string())).as_str())
        .help_template("\
{before-help}{name} {version}
{author-with-newline}{about-with-newline}
{usage-heading} {usage}

{all-args}{after-help}")
        .subcommand_required(true)
        .arg(
            Arg::new("ledger_path")
                .short('l')
                .long("ledger")
                .value_name("DIR")
                
                .global(true)
                .default_value("ledger")
                .help("Use DIR as ledger location"),
        )
        .arg(
            Arg::new("wal_recovery_mode")
                .long("wal-recovery-mode")
                .value_name("MODE")
                
                .global(true)
                .value_parser([
                    "tolerate_corrupted_tail_records",
                    "absolute_consistency",
                    "point_in_time",
                    "skip_any_corrupted_record",
                ])
                .help("Mode to recovery the ledger db write ahead log"),
        )
        .arg(
            Arg::new("force_update_to_open")
                .long("force-update-to-open")
                .action(ArgAction::SetTrue)
                .global(true)
                .help(
                    "Allow commands that would otherwise not alter the blockstore to make \
                     necessary updates in order to open it",
                ),
        )
        .arg(
            Arg::new("ignore_ulimit_nofile_error")
                .long("ignore-ulimit-nofile-error")
                .action(ArgAction::SetTrue)
                .global(true)
                .help(
                    "Allow opening the blockstore to succeed even if the desired open file \
                     descriptor limit cannot be configured. Use with caution as some commands may \
                     run fine with a reduced file descriptor limit while others will not",
                ),
        )
        .arg(
            Arg::new("block_verification_method")
                .long("block-verification-method")
                .value_name("METHOD")
                
                .value_parser(clap::value_parser!(String))
                .default_value(Box::leak(Box::new(BlockVerificationMethod::default().to_string())).as_str())
                .global(true)
                .help(BlockVerificationMethod::cli_message()),
        )
        .arg(
            Arg::new("unified_scheduler_handler_threads")
                .long("unified-scheduler-handler-threads")
                .value_name("COUNT")
                
                .value_parser(clap::value_parser!(u32))
                .global(true)
                .help(DefaultSchedulerPool::cli_message()),
        )
        .arg(
            Arg::new("output_format")
                .long("output")
                .value_name("FORMAT")
                .global(true)
                
                .value_parser(["json", "json-compact"])
                .help(
                    "Return information in specified output format, currently only available for \
                     bigtable and program subcommands",
                ),
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .global(true)
                .action(ArgAction::Append)
                .action(ArgAction::SetTrue)
                .help("Show additional information where supported"),
        )
        .bigtable_subcommand()
        .blockstore_subcommand()
        // All of the blockstore commands are added under the blockstore command.
        // For the sake of legacy support, also directly add the blockstore commands here so that
        // these subcommands can continue to be called from the top level of the binary.
        .subcommands(blockstore_subcommands(true))
        .subcommand(
            ClapCommand::new("genesis")
                .about("Prints the ledger's genesis config")
                .arg(&load_genesis_config_arg)
                .arg(
                    Arg::new("accounts")
                        .long("accounts")
                        .action(ArgAction::SetTrue)
                        .help("Print the ledger's genesis accounts"),
                )
                .arg(
                    Arg::new("no_account_data")
                        .long("no-account-data")
                        .action(ArgAction::SetTrue)
                        .requires("accounts")
                        .help("Do not print account data when printing account contents."),
                )
                .arg(&accounts_data_encoding_arg),
        )
        .subcommand(
            ClapCommand::new("genesis-hash")
                .about("Prints the ledger's genesis hash")
                .arg(&load_genesis_config_arg),
        )
        .subcommand(
            ClapCommand::new("modify-genesis")
                .about("Modifies genesis parameters")
                .arg(&load_genesis_config_arg)
                .arg(&hashes_per_tick)
                .arg(
                    Arg::new("cluster_type")
                        .long("cluster-type")
                        .value_parser(clap::value_parser!(String))
                        
                        .help("Selects the features that will be enabled for the cluster"),
                )
                .arg(
                    Arg::new("output_directory")
                        .index(1)
                        .value_name("DIR")
                        
                        .help("Output directory for the modified genesis config"),
                ),
        )
        .subcommand(
            ClapCommand::new("shred-version")
                .about("Prints the ledger's shred hash")
                .arg(&load_genesis_config_arg)
                .args(&accounts_db_config_args)
                .args(&snapshot_config_args)
                .arg(&hard_forks_arg),
        )
        .subcommand(
            ClapCommand::new("bank-hash")
                .about("Prints the hash of the working bank after reading the ledger")
                .arg(&load_genesis_config_arg)
                .args(&accounts_db_config_args)
                .args(&snapshot_config_args)
                .arg(&halt_at_slot_arg),
        )
        .subcommand(
            ClapCommand::new("verify")
                .about("Verify the ledger")
                .arg(&load_genesis_config_arg)
                .args(&accounts_db_config_args)
                .args(&snapshot_config_args)
                .arg(&halt_at_slot_arg)
                .arg(&limit_load_slot_count_from_snapshot_arg)
                .arg(&verify_index_arg)
                .arg(&hard_forks_arg)
                .arg(&os_memory_stats_reporting_arg)
                .arg(&allow_dead_slots_arg)
                .arg(&debug_key_arg)
                .arg(&geyser_plugin_args)
                .arg(&log_messages_bytes_limit_arg)
                .arg(
                    Arg::new("skip_poh_verify")
                        .long("skip-poh-verify")
                        .action(ArgAction::SetTrue)
                        .help(
                            "Deprecated, please use --skip-verification. Skip ledger PoH and \
                             transaction verification.",
                        ),
                )
                .arg(
                    Arg::new("skip_verification")
                        .long("skip-verification")
                        .action(ArgAction::SetTrue)
                        .help("Skip ledger PoH and transaction verification."),
                )
                .arg(
                    Arg::new("enable_rpc_transaction_history")
                        .long("enable-rpc-transaction-history")
                        .action(ArgAction::SetTrue)
                        .help("Store transaction info for processed slots into local ledger"),
                )
                .arg(
                    Arg::new("enable_extended_tx_metadata_storage")
                        .long("enable-extended-tx-metadata-storage")
                        .requires("enable_rpc_transaction_history")
                        .action(ArgAction::SetTrue)
                        .help(
                            "Include CPI inner instructions, logs, and return data in the \
                             historical transaction info stored",
                        ),
                )
                .arg(
                    Arg::new("run_final_hash_calc")
                        .long("run-final-accounts-hash-calculation")
                        .action(ArgAction::SetTrue)
                        .help(
                            "After 'verify' completes, run a final accounts hash calculation. \
                             Final hash calculation could race with accounts background service \
                             tasks and assert.",
                        ),
                )
                .arg(
                    Arg::new("print_accounts_stats")
                        .long("print-accounts-stats")
                        .action(ArgAction::SetTrue)
                        .help(
                            "After verifying the ledger, print some information about the account \
                             stores",
                        ),
                )
                .arg(
                    Arg::new("print_bank_hash")
                        .long("print-bank-hash")
                        .action(ArgAction::SetTrue)
                        .help("After verifying the ledger, print the working bank's hash"),
                )
                .arg(
                    Arg::new("write_bank_file")
                        .long("write-bank-file")
                        .action(ArgAction::SetTrue)
                        .help(
                            "After verifying the ledger, write a file that contains the \
                             information that went into computing the completed bank's bank hash. \
                             The file will be written within <LEDGER_DIR>/bank_hash_details/",
                        ),
                )
                .arg(
                    Arg::new("record_slots")
                        .long("record-slots")
                        .default_value("slots.json")
                        .value_name("FILENAME")
                        .help("Record slots to a file"),
                )
                .arg(
                    Arg::new("verify_slots")
                        .long("verify-slots")
                        .default_value("slots.json")
                        .value_name("FILENAME")
                        .help("Verify slots match contents of file"),
                )
                .arg(
                    Arg::new("record_slots_config")
                        .long("record-slots-config")
                        .action(ArgAction::Append)
                        
                        .value_parser(["accounts", "tx"])
                        .requires("record_slots")
                        .conflicts_with_all(&[
                            "enable_rpc_transaction_history",
                            "geyser_plugin_config",
                        ])
                        .help(
                            "In addition to the bank hash, optionally include accounts and/or \
                             transactions details for the slot",
                        ),
                )
                .arg(
                    Arg::new("abort_on_invalid_block")
                        .long("abort-on-invalid-block")
                        .action(ArgAction::SetTrue)
                        .help(
                            "Exits with failed status early as soon as any bad block is detected",
                        ),
                )
                .arg(
                    Arg::new("no_block_cost_limits")
                        .long("no-block-cost-limits")
                        .action(ArgAction::SetTrue)
                        .help("Disable block cost limits effectively by setting them to the max"),
                )
                .arg(
                    Arg::new("enable_hash_overrides")
                        .long("enable-hash-overrides")
                        .action(ArgAction::SetTrue)
                        .help(
                            "Enable override of blockhashes and bank hashes from banking trace \
                             event files to correctly verify blocks produced by the \
                             simulate-block-production subcommand",
                        ),
                ),
        )
        .subcommand(
            ClapCommand::new("graph")
                .about("Create a Graphviz rendering of the ledger")
                .arg(&load_genesis_config_arg)
                .args(&accounts_db_config_args)
                .args(&snapshot_config_args)
                .arg(&halt_at_slot_arg)
                .arg(&hard_forks_arg)
                .arg(
                    Arg::new("include_all_votes")
                        .long("include-all-votes")
                        .help("Include all votes in the graph"),
                )
                .arg(
                    Arg::new("graph_filename")
                        .index(1)
                        .value_name("FILENAME")
                        
                        .help("Output file"),
                )
                .arg(
                    Arg::new("vote_account_mode")
                        .long("vote-account-mode")
                        
                        .value_name("MODE")
                        .default_value(Box::leak(Box::new(default_graph_vote_account_mode.to_string())).as_str())
                        .value_parser(clap::value_parser!(String))
                        .help(
                            "Specify if and how to graph vote accounts. Enabling will incur \
                             significant rendering overhead, especially `with-history`",
                        ),
                ),
        )
        .subcommand(
            ClapCommand::new("create-snapshot")
                .about("Create a new ledger snapshot")
                .arg(&os_memory_stats_reporting_arg)
                .arg(&load_genesis_config_arg)
                .args(&accounts_db_config_args)
                .args(&snapshot_config_args)
                .arg(&hard_forks_arg)
                .arg(&snapshot_version_arg)
                .arg(&geyser_plugin_args)
                .arg(&log_messages_bytes_limit_arg)
                .arg(
                    Arg::new("snapshot_slot")
                        .index(1)
                        .value_name("SLOT")
                        .value_parser(clap::value_parser!(String))
                        
                        .help(
                            "Slot at which to create the snapshot; accepts keyword ROOT for the \
                             highest root",
                        ),
                )
                .arg(
                    Arg::new("output_directory")
                        .index(2)
                        .value_name("DIR")
                        
                        .help(
                            "Output directory for the snapshot \
                             [default: --snapshot-archive-path if present else --ledger directory]",
                        ),
                )
                .arg(
                    Arg::new("warp_slot")
                        .required(false)
                        .long("warp-slot")
                        
                        .value_name("WARP_SLOT")
                        .value_parser(clap::value_parser!(u64))
                        .help(
                            "After loading the snapshot slot warp the ledger to WARP_SLOT, which \
                             could be a slot in a galaxy far far away",
                        ),
                )
                .arg(
                    Arg::new("faucet_lamports")
                        .short('t')
                        .long("faucet-lamports")
                        .value_name("LAMPORTS")
                        
                        .requires("faucet_pubkey")
                        .help("Number of lamports to assign to the faucet"),
                )
                .arg(
                    Arg::new("faucet_pubkey")
                        .short('m')
                        .long("faucet-pubkey")
                        .value_name("PUBKEY")
                        
                        .value_parser(clap::value_parser!(String))
                        .requires("faucet_lamports")
                        .help("Path to file containing the faucet's pubkey"),
                )
                .arg(
                    Arg::new("bootstrap_validator")
                        .short('b')
                        .long("bootstrap-validator")
                        .value_name("IDENTITY_PUBKEY VOTE_PUBKEY STAKE_PUBKEY")
                        
                        .value_parser(clap::value_parser!(String))
                        .number_of_values(3)
                        .action(ArgAction::Append)
                        .help("The bootstrap validator's identity, vote and stake pubkeys"),
                )
                .arg(
                    Arg::new("bootstrap_stake_authorized_pubkey")
                        .long("bootstrap-stake-authorized-pubkey")
                        .value_name("BOOTSTRAP STAKE AUTHORIZED PUBKEY")
                        
                        .value_parser(clap::value_parser!(String))
                        .help(
                            "Path to file containing the pubkey authorized to manage the \
                             bootstrap validator's stake
                             [default: --bootstrap-validator IDENTITY_PUBKEY]",
                        ),
                )
                .arg(
                    Arg::new("bootstrap_validator_lamports")
                        .long("bootstrap-validator-lamports")
                        .value_name("LAMPORTS")
                        
                        .default_value(Box::leak(Box::new(default_bootstrap_validator_lamports.to_string())).as_str())
                        .help("Number of lamports to assign to the bootstrap validator"),
                )
                .arg(
                    Arg::new("bootstrap_validator_stake_lamports")
                        .long("bootstrap-validator-stake-lamports")
                        .value_name("LAMPORTS")
                        
                        .default_value(Box::leak(Box::new(default_bootstrap_validator_stake_lamports.to_string())).as_str())
                        .help(
                            "Number of lamports to assign to the bootstrap validator's stake \
                             account",
                        ),
                )
                .arg(
                    Arg::new("rent_burn_percentage")
                        .long("rent-burn-percentage")
                        .value_name("NUMBER")
                        
                        .help("Adjust percentage of collected rent to burn")
                        .value_parser(clap::value_parser!(u8)),
                )
                .arg(&hashes_per_tick)
                .arg(
                    Arg::new("accounts_to_remove")
                        .required(false)
                        .long("remove-account")
                        
                        .value_name("PUBKEY")
                        .value_parser(clap::value_parser!(String))
                        .action(ArgAction::Append)
                        .help("List of accounts to remove while creating the snapshot"),
                )
                .arg(
                    Arg::new("feature_gates_to_deactivate")
                        .required(false)
                        .long("deactivate-feature-gate")
                        
                        .value_name("PUBKEY")
                        .value_parser(clap::value_parser!(String))
                        .action(ArgAction::Append)
                        .help("List of feature gates to deactivate while creating the snapshot"),
                )
                .arg(
                    Arg::new("vote_accounts_to_destake")
                        .required(false)
                        .long("destake-vote-account")
                        
                        .value_name("PUBKEY")
                        .value_parser(clap::value_parser!(String))
                        .action(ArgAction::Append)
                        .help("List of validator vote accounts to destake"),
                )
                .arg(
                    Arg::new("remove_stake_accounts")
                        .required(false)
                        .long("remove-stake-accounts")
                        .action(ArgAction::SetTrue)
                        .help("Remove all existing stake accounts from the new snapshot"),
                )
                .arg(
                    Arg::new("incremental")
                        .long("incremental")
                        .action(ArgAction::SetTrue)
                        .help(
                            "Create an incremental snapshot instead of a full snapshot. This \
                             requires that the ledger is loaded from a full snapshot, which will \
                             be used as the base for the incremental snapshot.",
                        )
                        .conflicts_with("no_snapshot"),
                )
                .arg(
                    Arg::new("minimized")
                        .long("minimized")
                        .action(ArgAction::SetTrue)
                        .help(
                            "Create a minimized snapshot instead of a full snapshot. This \
                             snapshot will only include information needed to replay the ledger \
                             from the snapshot slot to the ending slot.",
                        )
                        .conflicts_with("incremental")
                        .requires("ending_slot"),
                )
                .arg(
                    Arg::new("ending_slot")
                        .long("ending-slot")
                        
                        .value_name("ENDING_SLOT")
                        .help("Ending slot for minimized snapshot creation"),
                )
                .arg(
                    Arg::new("snapshot_archive_format")
                        .long("snapshot-archive-format")
                        .value_parser(clap::value_parser!(String))
                        .default_value(DEFAULT_ARCHIVE_COMPRESSION)
                        .value_name("ARCHIVE_TYPE")
                        
                        .help("Snapshot archive format to use.")
                        .conflicts_with("no_snapshot"),
                )
                .arg(
                    Arg::new("snapshot_zstd_compression_level")
                        .long("snapshot-zstd-compression-level")
                        .default_value("0")
                        .value_name("LEVEL")
                        
                        .help("The compression level to use when archiving with zstd")
                        .long_help(
                            "The compression level to use when archiving with zstd. Higher \
                             compression levels generally produce higher compression ratio at the \
                             expense of speed and memory. See the zstd manpage for more \
                             information.",
                        ),
                )
                .arg(
                    Arg::new("enable_capitalization_change")
                        .long("enable-capitalization-change")
                        .action(ArgAction::SetTrue)
                        .help("If snapshot creation should succeed with a capitalization delta."),
                ),
        )
        .subcommand(
            ClapCommand::new("simulate-block-production")
                .about("Simulate producing blocks with banking trace event files in the ledger")
                .arg(&load_genesis_config_arg)
                .args(&accounts_db_config_args)
                .args(&snapshot_config_args)
                .arg(
                    Arg::new("block_production_method")
                        .long("block-production-method")
                        .value_name("METHOD")
                        
                        .value_parser(clap::value_parser!(String))
                        .default_value(Box::leak(Box::new(BlockProductionMethod::default().to_string())).as_str())
                        .help(BlockProductionMethod::cli_message()),
                )
                .arg(
                    Arg::new("transaction_struct")
                        .long("transaction-structure")
                        .value_name("STRUCT")
                        
                        .value_parser(clap::value_parser!(String))
                        .default_value(Box::leak(Box::new(TransactionStructure::default().to_string())).as_str())
                        .help(TransactionStructure::cli_message()),
                )
                .arg(
                    Arg::new("first_simulated_slot")
                        .long("first-simulated-slot")
                        .value_name("SLOT")
                        .value_parser(clap::value_parser!(u64))
                        
                        .required(true)
                        .help("Start simulation at the given slot"),
                )
                .arg(
                    Arg::new("no_block_cost_limits")
                        .long("no-block-cost-limits")
                        .action(ArgAction::SetTrue)
                        .help("Disable block cost limits effectively by setting them to the max"),
                ),
        )
        .subcommand(
            ClapCommand::new("accounts")
                .about("Print account stats and contents after processing the ledger")
                .arg(&load_genesis_config_arg)
                .args(&accounts_db_config_args)
                .args(&snapshot_config_args)
                .arg(&halt_at_slot_arg)
                .arg(&hard_forks_arg)
                .arg(&geyser_plugin_args)
                .arg(&log_messages_bytes_limit_arg)
                .arg(&accounts_data_encoding_arg)
                .arg(
                    Arg::new("include_sysvars")
                        .long("include-sysvars")
                        .action(ArgAction::SetTrue)
                        .help("Include sysvars too"),
                )
                .arg(
                    Arg::new("no_account_contents")
                        .long("no-account-contents")
                        .action(ArgAction::SetTrue)
                        .help(
                            "Do not print contents of each account, which is very slow with lots \
                             of accounts.",
                        ),
                )
                .arg(
                    Arg::new("no_account_data")
                        .long("no-account-data")
                        .action(ArgAction::SetTrue)
                        .help("Do not print account data when printing account contents."),
                )
                .arg(
                    Arg::new("account")
                        .long("account")
                        
                        .value_name("PUBKEY")
                        .value_parser(clap::value_parser!(String))
                        .action(ArgAction::Append)
                        .help(
                            "Limit output to accounts corresponding to the specified pubkey(s), \
                             may be specified multiple times",
                        ),
                )
                .arg(
                    Arg::new("program_accounts")
                        .long("program-accounts")
                        
                        .value_name("PUBKEY")
                        .value_parser(clap::value_parser!(String))
                        .conflicts_with("account")
                        .help("Limit output to accounts owned by the provided program pubkey"),
                ),
        )
        .subcommand(
            ClapCommand::new("capitalization")
                .about("Print capitalization (aka, total supply) while checksumming it")
                .arg(&load_genesis_config_arg)
                .args(&accounts_db_config_args)
                .args(&snapshot_config_args)
                .arg(&halt_at_slot_arg)
                .arg(&hard_forks_arg)
                .arg(&geyser_plugin_args)
                .arg(&log_messages_bytes_limit_arg)
                .arg(
                    Arg::new("warp_epoch")
                        .required(false)
                        .long("warp-epoch")
                        
                        .value_name("WARP_EPOCH")
                        .help(
                            "After loading the snapshot warp the ledger to WARP_EPOCH, which \
                             could be an epoch in a galaxy far far away",
                        ),
                )
                .arg(
                    Arg::new("inflation")
                        .required(false)
                        .long("inflation")
                        
                        .value_parser(["pico", "full", "none"])
                        .help("Overwrite inflation when warping"),
                )
                .arg(
                    Arg::new("enable_credits_auto_rewind")
                        .required(false)
                        .long("enable-credits-auto-rewind")
                        .action(ArgAction::SetTrue)
                        .help("Enable credits auto rewind"),
                )
                .arg(
                    Arg::new("recalculate_capitalization")
                        .required(false)
                        .long("recalculate-capitalization")
                        .action(ArgAction::SetTrue)
                        .help(
                            "Recalculate capitalization before warping; circumvents bank's \
                             out-of-sync capitalization",
                        ),
                )
                .arg(
                    Arg::new("csv_filename")
                        .long("csv-filename")
                        .value_name("FILENAME")
                        
                        .help("Output file in the csv format"),
                ),
        )
        .subcommand(
            ClapCommand::new("compute-slot-cost")
                .about(
                    "runs cost_model over the block at the given slots, computes how expensive a \
                     block was based on cost_model",
                )
                .arg(
                    Arg::new("slots")
                        .index(1)
                        .value_name("SLOTS")
                        .value_parser(clap::value_parser!(u64))
                        .action(ArgAction::Append)
                        
                        .help(
                            "Slots that their blocks are computed for cost, default to all slots \
                             in ledger",
                        ),
                )
                .arg(&allow_dead_slots_arg),
        )
        .program_subcommand()
        .get_matches();

    info!("{} {}", crate_name!(), solana_version::version!());

    let ledger_path = PathBuf::from(cli_matches.get_one::<String>("ledger_path").unwrap().clone());
    let verbose_level = cli_matches.get_count("verbose");

    // Name the rayon global thread pool
    rayon::ThreadPoolBuilder::new()
        .thread_name(|i| format!("solRayonGlob{i:02}"))
        .build_global()
        .unwrap();

    match cli_matches.subcommand() {
        Some(("bigtable", arg_matches)) => bigtable_process_command(&ledger_path, arg_matches),
        Some(("blockstore", arg_matches)) => blockstore_process_command(&ledger_path, arg_matches),
        Some(("program", arg_matches)) => program(&ledger_path, arg_matches),
        // This match case provides legacy support for commands that were previously top level
        // subcommands of the binary, but have been moved under the blockstore subcommand.
        Some(("analyze-storage", _))
        | Some(("bounds", _))
        | Some(("copy", _))
        | Some(("dead-slots", _))
        | Some(("duplicate-slots", _))
        | Some(("latest-optimistic-slots", _))
        | Some(("list-roots", _))
        | Some(("parse_full_frozen", _))
        | Some(("print", _))
        | Some(("print-file-metadata", _))
        | Some(("purge", _))
        | Some(("remove-dead-slot", _))
        | Some(("repair-roots", _))
        | Some(("set-dead-slot", _))
        | Some(("shred-meta", _))
        | Some(("slot", _)) => blockstore_process_command(&ledger_path, &cli_matches),
        _ => {
            let ledger_path = canonicalize_ledger_path(&ledger_path);

            match cli_matches.subcommand() {
                Some(("genesis", arg_matches)) => {
                    let output_format =
                        match arg_matches.get_one::<String>("output_format").map(|s| s.as_str()) { Some("json") => OutputFormat::Json, Some("json-compact") => OutputFormat::JsonCompact, _ => OutputFormat::Display };
                    let output_accounts = arg_matches.get_flag("accounts");

                    let genesis_config = open_genesis_config_by(&ledger_path, arg_matches);

                    if output_accounts {
                        let output_config = parse_account_output_config(arg_matches);

                        let accounts: Vec<_> = genesis_config
                            .accounts
                            .into_iter()
                            .map(|(pubkey, account)| {
                                CliAccount::new_with_config(
                                    &pubkey,
                                    &AccountSharedData::from(account),
                                    &output_config,
                                )
                            })
                            .collect();
                        let accounts = CliAccounts { accounts };

                        println!("{}", output_format.formatted_string(&accounts));
                    } else {
                        println!("{genesis_config}");
                    }
                }
                Some(("genesis-hash", arg_matches)) => {
                    println!(
                        "{}",
                        open_genesis_config_by(&ledger_path, arg_matches).hash()
                    );
                }
                Some(("modify-genesis", arg_matches)) => {
                    let mut genesis_config = open_genesis_config_by(&ledger_path, arg_matches);
                    let output_directory =
                        PathBuf::from(arg_matches.get_one::<String>("output_directory").unwrap());

                    if let Some(cluster_type_str) = arg_matches.get_one::<String>("cluster_type") {
                        if let Some(cluster_type) = cluster_type_str.parse().ok() {
                        genesis_config.cluster_type = cluster_type;
                        }
                    }

                    if let Some(hashes_per_tick) = arg_matches.get_one::<String>("hashes_per_tick") {
                        genesis_config.poh_config.hashes_per_tick = match hashes_per_tick.as_str() {
                            // Note: Unlike `solana-genesis`, "auto" is not supported here.
                            "sleep" => None,
                            _ => Some(arg_matches.get_one::<String>("hashes_per_tick").unwrap().parse().unwrap()),
                        }
                    }

                    create_new_ledger(
                        &output_directory,
                        &genesis_config,
                        solana_accounts_db::hardened_unpack::MAX_GENESIS_ARCHIVE_UNPACKED_SIZE,
                        LedgerColumnOptions::default(),
                    )
                    .unwrap_or_else(|err| {
                        eprintln!("Failed to write genesis config: {err:?}");
                        exit(1);
                    });

                    println!("{}", open_genesis_config_by(&output_directory, arg_matches));
                }
                Some(("shred-version", arg_matches)) => {
                    let mut process_options = parse_process_options(&ledger_path, arg_matches);
                    // Respect a user-set --halt-at-slot; otherwise, set Some(0) to avoid
                    // processing any additional banks and just use the snapshot bank
                    if process_options.halt_at_slot.is_none() {
                        process_options.halt_at_slot = Some(0);
                    }
                    let genesis_config = open_genesis_config_by(&ledger_path, arg_matches);
                    let blockstore = open_blockstore(
                        &ledger_path,
                        arg_matches,
                        get_access_type(&process_options),
                    );
                    let LoadAndProcessLedgerOutput { bank_forks, .. } =
                        load_and_process_ledger_or_exit(
                            arg_matches,
                            &genesis_config,
                            Arc::new(blockstore),
                            process_options,
                            None,
                        );

                    println!(
                        "{}",
                        compute_shred_version(
                            &genesis_config.hash(),
                            Some(&bank_forks.read().unwrap().working_bank().hard_forks())
                        )
                    );
                }
                Some(("bank-hash", _)) => {
                    eprintln!(
                        "The bank-hash command has been deprecated, use agave-ledger-tool verify \
                         --print-bank-hash ... instead"
                    );
                }
                Some(("verify", arg_matches)) => {
                    let exit_signal = Arc::new(AtomicBool::new(false));
                    let report_os_memory_stats =
                        arg_matches.get_flag("os_memory_stats_reporting");
                    let system_monitor_service = SystemMonitorService::new(
                        Arc::clone(&exit_signal),
                        SystemMonitorStatsReportConfig {
                            report_os_memory_stats,
                            report_os_network_stats: false,
                            report_os_cpu_stats: false,
                            report_os_disk_stats: false,
                        },
                    );

                    let mut process_options = parse_process_options(&ledger_path, arg_matches);
                    if arg_matches.get_flag("enable_hash_overrides") {
                        let banking_trace_events = load_banking_trace_events_or_exit(&ledger_path);
                        process_options.hash_overrides =
                            Some(banking_trace_events.hash_overrides().clone());
                    }

                    let (slot_callback, slot_recorder_config) = setup_slot_recording(arg_matches);
                    process_options.slot_callback = slot_callback;
                    let transaction_status_sender = slot_recorder_config
                        .as_ref()
                        .and_then(|config| config.transaction_status_sender.clone());

                    let output_format =
                        match arg_matches.get_one::<String>("output_format").map(|s| s.as_str()) { Some("json") => OutputFormat::Json, Some("json-compact") => OutputFormat::JsonCompact, _ => OutputFormat::Display };
                    let print_accounts_stats = arg_matches.get_flag("print_accounts_stats");
                    let print_bank_hash = arg_matches.get_flag("print_bank_hash");
                    let write_bank_file = arg_matches.get_flag("write_bank_file");

                    let genesis_config = open_genesis_config_by(&ledger_path, arg_matches);
                    info!("genesis hash: {}", genesis_config.hash());

                    let blockstore = open_blockstore(
                        &ledger_path,
                        arg_matches,
                        get_access_type(&process_options),
                    );
                    let LoadAndProcessLedgerOutput { bank_forks, .. } =
                        load_and_process_ledger_or_exit(
                            arg_matches,
                            &genesis_config,
                            Arc::new(blockstore),
                            process_options,
                            transaction_status_sender,
                        );

                    let working_bank = bank_forks.read().unwrap().working_bank();
                    if print_accounts_stats {
                        working_bank.print_accounts_stats();
                    }
                    if print_bank_hash {
                        let slot_bank_hash = SlotBankHash {
                            slot: working_bank.slot(),
                            hash: working_bank.hash().to_string(),
                        };
                        println!("{}", output_format.formatted_string(&slot_bank_hash));
                    }
                    if write_bank_file {
                        bank_hash_details::write_bank_hash_details_file(&working_bank)
                            .map_err(|err| {
                                warn!("Unable to write bank hash_details file: {err}");
                            })
                            .ok();
                    }

                    if let Some(mut slot_recorder_config) = slot_recorder_config {
                        // Drop transaction_status_sender to break transaction_recorder
                        // out of its' recieve loop
                        let transaction_status_sender =
                            slot_recorder_config.transaction_status_sender.take();
                        drop(transaction_status_sender);
                        if let Some(transaction_recorder) =
                            slot_recorder_config.transaction_recorder
                        {
                            transaction_recorder.join().unwrap();
                        }

                        let slot_details = slot_recorder_config.slot_details.lock().unwrap();
                        let bank_hashes =
                            bank_hash_details::BankHashDetails::new(slot_details.to_vec());

                        // writing the json file ends up with a syscall for each number, comma, indentation etc.
                        // use BufWriter to speed things up
                        let writer = std::io::BufWriter::new(slot_recorder_config.file);
                        serde_json::to_writer_pretty(writer, &bank_hashes).unwrap();
                    }

                    exit_signal.store(true, Ordering::Relaxed);
                    system_monitor_service.join().unwrap();
                }
                Some(("graph", arg_matches)) => {
                    let output_file = arg_matches.get_one::<String>("graph_filename").unwrap().clone();
                    let graph_config = GraphConfig {
                        include_all_votes: arg_matches.get_flag("include_all_votes"),
                        vote_account_mode: arg_matches.get_one::<String>("vote_account_mode")
                            .unwrap().parse().unwrap(),
                    };

                    let process_options = parse_process_options(&ledger_path, arg_matches);
                    let genesis_config = open_genesis_config_by(&ledger_path, arg_matches);
                    let blockstore = open_blockstore(
                        &ledger_path,
                        arg_matches,
                        get_access_type(&process_options),
                    );
                    let LoadAndProcessLedgerOutput { bank_forks, .. } =
                        load_and_process_ledger_or_exit(
                            arg_matches,
                            &genesis_config,
                            Arc::new(blockstore),
                            process_options,
                            None,
                        );

                    let dot = graph_forks(&bank_forks.read().unwrap(), &graph_config);
                    let extension = Path::new(&output_file).extension();
                    let result = if extension == Some(OsStr::new("pdf")) {
                        render_dot(dot, &output_file, "pdf")
                    } else if extension == Some(OsStr::new("png")) {
                        render_dot(dot, &output_file, "png")
                    } else {
                        File::create(&output_file)
                            .and_then(|mut file| file.write_all(&dot.into_bytes()))
                    };

                    match result {
                        Ok(_) => println!("Wrote {output_file}"),
                        Err(err) => eprintln!("Unable to write {output_file}: {err}"),
                    }
                }
                Some(("create-snapshot", arg_matches)) => {
                    let exit_signal = Arc::new(AtomicBool::new(false));
                    let system_monitor_service = arg_matches
                        .get_flag("os_memory_stats_reporting")
                        .then(|| {
                            SystemMonitorService::new(
                                Arc::clone(&exit_signal),
                                SystemMonitorStatsReportConfig {
                                    report_os_memory_stats: true,
                                    report_os_network_stats: false,
                                    report_os_cpu_stats: false,
                                    report_os_disk_stats: false,
                                },
                            )
                        });

                    let is_incremental = arg_matches.get_flag("incremental");
                    let is_minimized = arg_matches.get_flag("minimized");
                    let output_directory = arg_matches.get_one::<String>("output_directory").map(|s| s.parse::<PathBuf>().unwrap())
                        .unwrap_or_else(|| {
                            let snapshot_archive_path = arg_matches.get_one::<String>("snapshots")
                                .map(|s| s.parse::<String>().unwrap())
                                .map(PathBuf::from);
                            let incremental_snapshot_archive_path =
                                arg_matches.get_one::<String>("incremental_snapshot_archive_path")
                                    .map(|s| s.parse::<String>().unwrap())
                                    .map(PathBuf::from);
                            match (
                                is_incremental,
                                &snapshot_archive_path,
                                &incremental_snapshot_archive_path,
                            ) {
                                (true, _, Some(incremental_snapshot_archive_path)) => {
                                    incremental_snapshot_archive_path.clone()
                                }
                                (_, Some(snapshot_archive_path), _) => {
                                    snapshot_archive_path.clone()
                                }
                                (_, _, _) => ledger_path.clone(),
                            }
                        });
                    let mut warp_slot = arg_matches.get_one::<String>("warp_slot").map(|s| s.parse::<Slot>().unwrap());
                    let remove_stake_accounts = arg_matches.get_flag("remove_stake_accounts");

                    let faucet_pubkey = arg_matches.get_one::<String>("faucet_pubkey").and_then(|s| s.parse().ok());
                    let faucet_lamports =
                        arg_matches.get_one::<String>("faucet_lamports").map(|s| s.parse::<u64>().unwrap()).unwrap_or(0);

                    let rent_burn_percentage = arg_matches.get_one::<String>("rent_burn_percentage").map(|s| s.parse::<u8>().unwrap());
                    let hashes_per_tick = arg_matches.get_one::<String>("hashes_per_tick");

                    let bootstrap_stake_authorized_pubkey =
                        arg_matches.get_one::<String>("bootstrap_stake_authorized_pubkey").and_then(|s| s.parse().ok());
                    let bootstrap_validator_lamports =
                                            arg_matches.get_one::<String>("bootstrap_validator_lamports").unwrap().parse().unwrap();
                let bootstrap_validator_stake_lamports =
                    arg_matches.get_one::<String>("bootstrap_validator_stake_lamports").unwrap().parse().unwrap();
                    let minimum_stake_lamports = rent.minimum_balance(StakeStateV2::size_of());
                    if bootstrap_validator_stake_lamports < minimum_stake_lamports {
                        eprintln!(
                            "Error: insufficient --bootstrap-validator-stake-lamports. Minimum \
                             amount is {minimum_stake_lamports}"
                        );
                        exit(1);
                    }
                    let bootstrap_validator_pubkeys =
                        arg_matches.get_many::<String>("bootstrap_validator").map(|values| values.filter_map(|s| s.parse().ok()).collect::<Vec<_>>());
                    let accounts_to_remove =
                        arg_matches.get_many::<String>("accounts_to_remove").map(|values| values.filter_map(|s| s.parse().ok()).collect::<Vec<_>>()).unwrap_or_default();
                    let feature_gates_to_deactivate =
                        arg_matches.get_many::<String>("feature_gates_to_deactivate").map(|values| values.filter_map(|s| s.parse().ok()).collect::<Vec<_>>()).unwrap_or_default();
                    let vote_accounts_to_destake: HashSet<_> =
                        arg_matches.get_many::<String>("vote_accounts_to_destake").map(|values| values.filter_map(|s| s.parse::<Pubkey>().ok()).collect::<Vec<_>>())
                            .unwrap_or_default()
                            .into_iter()
                            .collect();
                    let snapshot_version = arg_matches.get_one::<String>("snapshot_version").map_or(
                        SnapshotVersion::default(),
                        |s| {
                            s.parse::<SnapshotVersion>().unwrap_or_else(|e| {
                                eprintln!("Error: {e}");
                                exit(1)
                            })
                        },
                    );

                    let snapshot_archive_format = {
                        let archive_format_str =
                            arg_matches.get_one::<String>("snapshot_archive_format").unwrap().clone();
                        let mut archive_format = ArchiveFormat::from_cli_arg(&archive_format_str)
                            .unwrap_or_else(|| {
                                panic!("Archive format not recognized: {archive_format_str}")
                            });
                        if let ArchiveFormat::TarZstd { config } = &mut archive_format {
                            config.compression_level = arg_matches.get_one::<String>("snapshot_zstd_compression_level")
                                .unwrap().parse().unwrap();
                        }
                        archive_format
                    };

                    let genesis_config = open_genesis_config_by(&ledger_path, arg_matches);
                    let mut process_options = parse_process_options(&ledger_path, arg_matches);

                    let blockstore = Arc::new(open_blockstore(
                        &ledger_path,
                        arg_matches,
                        get_access_type(&process_options),
                    ));

                    let snapshot_slot = if Some("ROOT") == arg_matches.get_one::<String>("snapshot_slot").map(|s| s.as_str()) {
                        blockstore
                            .rooted_slot_iterator(0)
                            .expect("Failed to get rooted slot iterator")
                            .last()
                            .expect("Failed to get root")
                    } else {
                        arg_matches.get_one::<String>("snapshot_slot").unwrap().parse().unwrap()
                    };

                    if blockstore
                        .meta(snapshot_slot)
                        .unwrap()
                        .filter(|m| m.is_full())
                        .is_none()
                    {
                        eprintln!(
                            "Error: snapshot slot {snapshot_slot} does not exist in blockstore or \
                             is not full.",
                        );
                        exit(1);
                    }
                    process_options.halt_at_slot = Some(snapshot_slot);

                    let ending_slot = if is_minimized {
                        let ending_slot = arg_matches.get_one::<String>("ending_slot").unwrap().parse().unwrap();
                        if ending_slot <= snapshot_slot {
                            eprintln!(
                                "Error: ending_slot ({ending_slot}) must be greater than \
                                 snapshot_slot ({snapshot_slot})"
                            );
                            exit(1);
                        }

                        Some(ending_slot)
                    } else {
                        None
                    };

                    let enable_capitalization_change =
                        arg_matches.get_flag("enable_capitalization_change");

                    let snapshot_type_str = if is_incremental {
                        "incremental "
                    } else if is_minimized {
                        "minimized "
                    } else {
                        ""
                    };

                    info!(
                        "Creating {}snapshot of slot {} in {}",
                        snapshot_type_str,
                        snapshot_slot,
                        output_directory.display()
                    );

                    let LoadAndProcessLedgerOutput {
                        bank_forks,
                        starting_snapshot_hashes,
                        accounts_background_service,
                    } = load_and_process_ledger_or_exit(
                        arg_matches,
                        &genesis_config,
                        blockstore.clone(),
                        process_options,
                        None,
                    );

                    let mut bank = bank_forks
                        .read()
                        .unwrap()
                        .get(snapshot_slot)
                        .unwrap_or_else(|| {
                            eprintln!("Error: Slot {snapshot_slot} is not available");
                            exit(1);
                        });

                    // Snapshot creation will implicitly perform AccountsDb
                    // flush and clean operations. These operations cannot be
                    // run concurrently, so ensure ABS is stopped to avoid that
                    // possibility.
                    accounts_background_service.join().unwrap();

                    // Similar to waiting for ABS to stop, we also wait for the initial startup
                    // verification to complete. The startup verification runs in the background
                    // and verifies the snapshot's accounts hashes are correct. We only want a
                    // single accounts hash calculation to run at a time, and since snapshot
                    // creation below will calculate the accounts hash, we wait for the startup
                    // verification to complete before proceeding.
                    bank.rc
                        .accounts
                        .accounts_db
                        .verify_accounts_hash_in_bg
                        .join_background_thread();

                    let child_bank_required = rent_burn_percentage.is_some()
                        || hashes_per_tick.is_some()
                        || remove_stake_accounts
                        || !accounts_to_remove.is_empty()
                        || !feature_gates_to_deactivate.is_empty()
                        || !vote_accounts_to_destake.is_empty()
                        || faucet_pubkey.is_some()
                        || bootstrap_validator_pubkeys.is_some();

                    if child_bank_required {
                        let mut child_bank = Bank::new_from_parent(
                            bank.clone(),
                            bank.collector_id(),
                            bank.slot() + 1,
                        );

                        if let Some(rent_burn_percentage) = rent_burn_percentage {
                            child_bank.set_rent_burn_percentage(rent_burn_percentage);
                        }

                        if let Some(hashes_per_tick) = hashes_per_tick {
                                            child_bank.set_hashes_per_tick(match hashes_per_tick.as_str() {
                    // Note: Unlike `solana-genesis`, "auto" is not supported here.
                    "sleep" => None,
                    _ => Some(arg_matches.get_one::<String>("hashes_per_tick").unwrap().parse().unwrap()),
                });
                        }

                        for address in feature_gates_to_deactivate {
                            let mut account =
                                child_bank.get_account(&address).unwrap_or_else(|| {
                                    eprintln!(
                                        "Error: Feature-gate account does not exist, unable to \
                                         deactivate it: {address}"
                                    );
                                    exit(1);
                                });

                            match feature::from_account(&account) {
                                Some(feature) => {
                                    if feature.activated_at.is_none() {
                                        warn!("Feature gate is not yet activated: {address}");
                                    } else {
                                        child_bank.deactivate_feature(&address);
                                    }
                                }
                                None => {
                                    eprintln!("Error: Account is not a `Feature`: {address}");
                                    exit(1);
                                }
                            }

                            account.set_lamports(0);
                            child_bank.store_account(&address, &account);
                            debug!("Feature gate deactivated: {address}");
                        }

                        bank = Arc::new(child_bank);
                    }

                    if let Some(faucet_pubkey) = faucet_pubkey {
                        bank.store_account(
                            &faucet_pubkey,
                            &AccountSharedData::new(faucet_lamports, 0, &system_program::id()),
                        );
                    }

                    if remove_stake_accounts {
                        for (address, mut account) in bank
                            .get_program_accounts(
                                &stake::program::id(),
                                &ScanConfig::new(ScanOrder::Sorted),
                            )
                            .unwrap()
                            .into_iter()
                        {
                            account.set_lamports(0);
                            bank.store_account(&address, &account);
                        }
                    }

                    for address in accounts_to_remove {
                        let mut account = bank.get_account(&address).unwrap_or_else(|| {
                            eprintln!(
                                "Error: Account does not exist, unable to remove it: {address}"
                            );
                            exit(1);
                        });

                        account.set_lamports(0);
                        bank.store_account(&address, &account);
                        debug!("Account removed: {address}");
                    }

                    if !vote_accounts_to_destake.is_empty() {
                        for (address, mut account) in bank
                            .get_program_accounts(
                                &stake::program::id(),
                                &ScanConfig::new(ScanOrder::Sorted),
                            )
                            .unwrap()
                            .into_iter()
                        {
                            if let Ok(StakeStateV2::Stake(meta, stake, _)) = account.state() {
                                if vote_accounts_to_destake.contains(&stake.delegation.voter_pubkey)
                                {
                                    if verbose_level > 0 {
                                        warn!(
                                            "Undelegating stake account {} from {}",
                                            address, stake.delegation.voter_pubkey,
                                        );
                                    }
                                    account.set_state(&StakeStateV2::Initialized(meta)).unwrap();
                                    bank.store_account(&address, &account);
                                }
                            }
                        }
                    }

                    if let Some(bootstrap_validator_pubkeys) = bootstrap_validator_pubkeys {
                        assert_eq!(bootstrap_validator_pubkeys.len() % 3, 0);

                        // Ensure there are no duplicated pubkeys in the --bootstrap-validator list
                        {
                            let mut v = bootstrap_validator_pubkeys.clone();
                            v.sort();
                            v.dedup();
                            if v.len() != bootstrap_validator_pubkeys.len() {
                                eprintln!(
                                    "Error: --bootstrap-validator pubkeys cannot be duplicated"
                                );
                                exit(1);
                            }
                        }

                        // Delete existing vote accounts
                        for (address, mut account) in bank
                            .get_program_accounts(
                                &solana_vote_program::id(),
                                &ScanConfig::new(ScanOrder::Sorted),
                            )
                            .unwrap()
                            .into_iter()
                        {
                            account.set_lamports(0);
                            bank.store_account(&address, &account);
                        }

                        // Add a new identity/vote/stake account for each of the provided bootstrap
                        // validators
                        let mut bootstrap_validator_pubkeys_iter =
                            bootstrap_validator_pubkeys.iter();
                        loop {
                            let Some(identity_pubkey) = bootstrap_validator_pubkeys_iter.next()
                            else {
                                break;
                            };
                            let vote_pubkey = bootstrap_validator_pubkeys_iter.next().unwrap();
                            let stake_pubkey = bootstrap_validator_pubkeys_iter.next().unwrap();

                            bank.store_account(
                                identity_pubkey,
                                &AccountSharedData::new(
                                    bootstrap_validator_lamports,
                                    0,
                                    &system_program::id(),
                                ),
                            );

                            let vote_account = vote_state::create_account_with_authorized(
                                identity_pubkey,
                                identity_pubkey,
                                identity_pubkey,
                                100,
                                VoteStateV3::get_rent_exempt_reserve(&rent).max(1),
                            );

                            bank.store_account(
                                stake_pubkey,
                                &stake_state::create_account(
                                    bootstrap_stake_authorized_pubkey
                                        .as_ref()
                                        .unwrap_or(identity_pubkey),
                                    vote_pubkey,
                                    &vote_account,
                                    &rent,
                                    bootstrap_validator_stake_lamports,
                                ),
                            );
                            bank.store_account(vote_pubkey, &vote_account);
                        }

                        // Warp ahead at least two epochs to ensure that the leader schedule will be
                        // updated to reflect the new bootstrap validator(s)
                        let minimum_warp_slot =
                            genesis_config.epoch_schedule.get_first_slot_in_epoch(
                                genesis_config.epoch_schedule.get_epoch(snapshot_slot) + 2,
                            );

                        if let Some(warp_slot) = warp_slot {
                            if warp_slot < minimum_warp_slot {
                                eprintln!(
                                    "Error: --warp-slot too close.  Must be >= {minimum_warp_slot}"
                                );
                                exit(1);
                            }
                        } else {
                            warn!("Warping to slot {minimum_warp_slot}");
                            warp_slot = Some(minimum_warp_slot);
                        }
                    }

                    if child_bank_required {
                        bank.fill_bank_with_ticks_for_tests();
                    }

                    let pre_capitalization = bank.capitalization();
                    let post_capitalization = bank.calculate_capitalization_for_tests();
                    bank.set_capitalization_for_tests(post_capitalization);

                    let capitalization_message = if pre_capitalization != post_capitalization {
                        let amount = if pre_capitalization > post_capitalization {
                            format!("-{}", pre_capitalization - post_capitalization)
                        } else {
                            (post_capitalization - pre_capitalization).to_string()
                        };
                        let msg = format!("Capitalization change: {amount} lamports");
                        warn!("{msg}");
                        if !enable_capitalization_change {
                            eprintln!(
                                "{msg}\nBut `--enable-capitalization-change flag not provided"
                            );
                            exit(1);
                        }
                        Some(msg)
                    } else {
                        None
                    };

                    let bank = if let Some(warp_slot) = warp_slot {
                        // Need to flush the write cache in order to use
                        // Storages to calculate the accounts hash, and need to
                        // root `bank` before flushing the cache. Use squash to
                        // root all unrooted parents as well and avoid panicking
                        // during snapshot creation if we try to add roots out
                        // of order.
                        bank.squash();
                        bank.force_flush_accounts_cache();
                        Arc::new(Bank::warp_from_parent(
                            bank.clone(),
                            bank.collector_id(),
                            warp_slot,
                        ))
                    } else {
                        bank
                    };

                    let minimize_snapshot_possibly_incomplete = if is_minimized {
                        minimize_bank_for_snapshot(
                            &blockstore,
                            &bank,
                            snapshot_slot,
                            ending_slot.unwrap(),
                        )
                    } else {
                        false
                    };

                    println!(
                        "Creating a version {} {}snapshot of slot {}",
                        snapshot_version,
                        snapshot_type_str,
                        bank.slot(),
                    );

                    if is_incremental {
                        if starting_snapshot_hashes.is_none() {
                            eprintln!(
                                "Unable to create incremental snapshot without a base full \
                                 snapshot"
                            );
                            exit(1);
                        }
                        let full_snapshot_slot = starting_snapshot_hashes.unwrap().full.0 .0;
                        if bank.slot() <= full_snapshot_slot {
                            eprintln!(
                                "Unable to create incremental snapshot: Slot must be greater than \
                                 full snapshot slot. slot: {}, full snapshot slot: {}",
                                bank.slot(),
                                full_snapshot_slot,
                            );
                            exit(1);
                        }

                        let incremental_snapshot_archive_info =
                            snapshot_bank_utils::bank_to_incremental_snapshot_archive(
                                ledger_path,
                                &bank,
                                full_snapshot_slot,
                                Some(snapshot_version),
                                output_directory.clone(),
                                output_directory,
                                snapshot_archive_format,
                            )
                            .unwrap_or_else(|err| {
                                eprintln!("Unable to create incremental snapshot: {err}");
                                exit(1);
                            });

                        println!(
                            "Successfully created incremental snapshot for slot {}, hash {}, base \
                             slot: {}: {}",
                            bank.slot(),
                            bank.hash(),
                            full_snapshot_slot,
                            incremental_snapshot_archive_info.path().display(),
                        );
                    } else {
                        let full_snapshot_archive_info =
                            snapshot_bank_utils::bank_to_full_snapshot_archive(
                                ledger_path,
                                &bank,
                                Some(snapshot_version),
                                output_directory.clone(),
                                output_directory,
                                snapshot_archive_format,
                            )
                            .unwrap_or_else(|err| {
                                eprintln!("Unable to create snapshot: {err}");
                                exit(1);
                            });

                        println!(
                            "Successfully created snapshot for slot {}, hash {}: {}",
                            bank.slot(),
                            bank.hash(),
                            full_snapshot_archive_info.path().display(),
                        );

                        if is_minimized {
                            let starting_epoch = bank.epoch_schedule().get_epoch(snapshot_slot);
                            let ending_epoch =
                                bank.epoch_schedule().get_epoch(ending_slot.unwrap());
                            if starting_epoch != ending_epoch {
                                warn!(
                                    "Minimized snapshot range crosses epoch boundary ({} to {}). \
                                     Bank hashes after {} will not match replays from a full \
                                     snapshot",
                                    starting_epoch,
                                    ending_epoch,
                                    bank.epoch_schedule().get_last_slot_in_epoch(starting_epoch)
                                );
                            }

                            if minimize_snapshot_possibly_incomplete {
                                warn!(
                                    "Minimized snapshot may be incomplete due to missing accounts \
                                     from CPI'd address lookup table extensions. This may lead to \
                                     mismatched bank hashes while replaying."
                                );
                            }
                        }
                    }

                    if let Some(msg) = capitalization_message {
                        println!("{msg}");
                    }
                    println!(
                        "Shred version: {}",
                        compute_shred_version(&genesis_config.hash(), Some(&bank.hard_forks()))
                    );

                    if let Some(system_monitor_service) = system_monitor_service {
                        exit_signal.store(true, Ordering::Relaxed);
                        system_monitor_service.join().unwrap();
                    }
                }
                Some(("simulate-block-production", arg_matches)) => {
                    let mut process_options = parse_process_options(&ledger_path, arg_matches);

                    let banking_trace_events = load_banking_trace_events_or_exit(&ledger_path);
                    process_options.hash_overrides =
                        Some(banking_trace_events.hash_overrides().clone());

                    let slot = arg_matches.get_one::<String>("first_simulated_slot").unwrap().parse::<Slot>().unwrap();
                    let simulator = BankingSimulator::new(banking_trace_events, slot);
                    let Some(parent_slot) = simulator.parent_slot() else {
                        eprintln!(
                            "Couldn't determine parent_slot of first_simulated_slot: {slot} due \
                             to missing banking_trace_event data."
                        );
                        exit(1);
                    };
                    process_options.halt_at_slot = Some(parent_slot);

                    // PrimaryForMaintenance needed over Secondary to purge any
                    // existing simulated shreds from previous runs
                    let blockstore = Arc::new(open_blockstore(
                        &ledger_path,
                        arg_matches,
                        AccessType::PrimaryForMaintenance,
                    ));
                    let genesis_config = open_genesis_config_by(&ledger_path, arg_matches);
                    let LoadAndProcessLedgerOutput { bank_forks, .. } =
                        load_and_process_ledger_or_exit(
                            arg_matches,
                            &genesis_config,
                            blockstore.clone(),
                            process_options,
                            None, // transaction status sender
                        );

                                    let block_production_method = arg_matches.get_one::<String>("block_production_method")
                    .unwrap().parse().unwrap();
                    let transaction_struct =
                        arg_matches.get_one::<String>("transaction_struct").unwrap().parse().unwrap();

                    info!(
                        "Using: block-production-method: {block_production_method} \
                         transaction-structure: {transaction_struct}"
                    );

                    match simulator.start(
                        genesis_config,
                        bank_forks,
                        blockstore,
                        block_production_method,
                        transaction_struct,
                    ) {
                        Ok(()) => println!("Ok"),
                        Err(error) => {
                            eprintln!("{error:?}");
                            exit(1);
                        }
                    };
                }
                Some(("accounts", arg_matches)) => {
                    let process_options = parse_process_options(&ledger_path, arg_matches);
                    let genesis_config = open_genesis_config_by(&ledger_path, arg_matches);
                    let blockstore = open_blockstore(
                        &ledger_path,
                        arg_matches,
                        get_access_type(&process_options),
                    );
                    let LoadAndProcessLedgerOutput { bank_forks, .. } =
                        load_and_process_ledger_or_exit(
                            arg_matches,
                            &genesis_config,
                            Arc::new(blockstore),
                            process_options,
                            None,
                        );
                    let bank = bank_forks.read().unwrap().working_bank();

                    let include_sysvars = arg_matches.get_flag("include_sysvars");
                    let output_config = if arg_matches.get_flag("no_account_contents") {
                        None
                    } else {
                        Some(parse_account_output_config(arg_matches))
                    };

                    let mode = if let Some(pubkeys) = arg_matches.get_many::<String>("account").map(|values| values.filter_map(|s| s.parse().ok()).collect::<Vec<_>>()) {
                        info!("Scanning individual accounts: {pubkeys:?}");
                        AccountsOutputMode::Individual(pubkeys)
                    } else if let Some(pubkey) = arg_matches.get_one::<String>("program_accounts").and_then(|s| s.parse().ok()) {
                        info!("Scanning program accounts for {pubkey}");
                        AccountsOutputMode::Program(pubkey)
                    } else {
                        info!("Scanning all accounts");
                        AccountsOutputMode::All
                    };
                    let config = AccountsOutputConfig {
                        mode,
                        output_config,
                        include_sysvars,
                    };
                    let output_format =
                        match arg_matches.get_one::<String>("output_format").map(|s| s.as_str()) { Some("json") => OutputFormat::Json, Some("json-compact") => OutputFormat::JsonCompact, _ => OutputFormat::Display };

                    let accounts_streamer =
                        AccountsOutputStreamer::new(bank, output_format, config);
                    let (_, scan_time) = measure_time!(
                        accounts_streamer
                            .output()
                            .map_err(|err| error!("Error while outputting accounts: {err}")),
                        "accounts scan"
                    );
                    info!("{scan_time}");
                }
                Some(("capitalization", arg_matches)) => {
                    let process_options = parse_process_options(&ledger_path, arg_matches);
                    let genesis_config = open_genesis_config_by(&ledger_path, arg_matches);
                    let blockstore = open_blockstore(
                        &ledger_path,
                        arg_matches,
                        get_access_type(&process_options),
                    );
                    let LoadAndProcessLedgerOutput { bank_forks, .. } =
                        load_and_process_ledger_or_exit(
                            arg_matches,
                            &genesis_config,
                            Arc::new(blockstore),
                            process_options,
                            None,
                        );
                    let bank_forks = bank_forks.read().unwrap();
                    let slot = bank_forks.working_bank().slot();
                    let bank = bank_forks.get(slot).unwrap_or_else(|| {
                        eprintln!("Error: Slot {slot} is not available");
                        exit(1);
                    });

                    if arg_matches.get_flag("recalculate_capitalization") {
                        println!("Recalculating capitalization");
                        let old_capitalization = bank.capitalization();
                        let new_capitalization = bank.calculate_capitalization_for_tests();
                        bank.set_capitalization_for_tests(new_capitalization);
                        if old_capitalization == new_capitalization {
                            eprintln!("Capitalization was identical: {}", Sol(old_capitalization));
                        }
                    }

                    if arg_matches.get_flag("warp_epoch") {
                        let base_bank = bank;

                        let raw_warp_epoch = arg_matches.get_one::<String>("warp_epoch").unwrap().parse::<String>().unwrap();
                        let warp_epoch = if raw_warp_epoch.starts_with('+') {
                            base_bank.epoch() + arg_matches.get_one::<String>("warp_epoch").unwrap().parse::<Epoch>().unwrap()
                        } else {
                            arg_matches.get_one::<String>("warp_epoch").unwrap().parse::<Epoch>().unwrap()
                        };
                        if warp_epoch < base_bank.epoch() {
                            eprintln!(
                                "Error: can't warp epoch backwards: {} => {}",
                                base_bank.epoch(),
                                warp_epoch
                            );
                            exit(1);
                        }

                        if let Some(raw_inflation) = arg_matches.get_one::<String>("inflation") {
                            let inflation = match raw_inflation.as_str() {
                                "pico" => Inflation::pico(),
                                "full" => Inflation::full(),
                                "none" => Inflation::new_disabled(),
                                _ => unreachable!(),
                            };
                            println!(
                                "Forcing to: {:?} (was: {:?})",
                                inflation,
                                base_bank.inflation()
                            );
                            base_bank.set_inflation(inflation);
                        }

                        let next_epoch = base_bank
                            .epoch_schedule()
                            .get_first_slot_in_epoch(warp_epoch);

                        let feature_account_balance = std::cmp::max(
                            genesis_config.rent.minimum_balance(Feature::size_of()),
                            1,
                        );
                        if arg_matches.get_flag("enable_credits_auto_rewind") {
                            base_bank.unfreeze_for_ledger_tool();
                            let mut force_enabled_count = 0;
                            if base_bank
                                .get_account(&feature_set::credits_auto_rewind::id())
                                .is_none()
                            {
                                base_bank.store_account(
                                    &feature_set::credits_auto_rewind::id(),
                                    &feature::create_account(
                                        &Feature { activated_at: None },
                                        feature_account_balance,
                                    ),
                                );
                                force_enabled_count += 1;
                            }
                            if force_enabled_count == 0 {
                                warn!("Already credits_auto_rewind is activated (or scheduled)");
                            }
                            let mut store_failed_count = 0;
                            if force_enabled_count >= 1 {
                                if base_bank
                                    .get_account(&feature_set::deprecate_rewards_sysvar::id())
                                    .is_some()
                                {
                                    // steal some lamports from the pretty old feature not to affect
                                    // capitalizaion, which doesn't affect inflation behavior!
                                    base_bank.store_account(
                                        &feature_set::deprecate_rewards_sysvar::id(),
                                        &AccountSharedData::default(),
                                    );
                                    force_enabled_count -= 1;
                                } else {
                                    store_failed_count += 1;
                                }
                            }
                            assert_eq!(force_enabled_count, store_failed_count);
                            if store_failed_count >= 1 {
                                // we have no choice; maybe locally created blank cluster with
                                // not-Development cluster type.
                                let old_cap = base_bank.capitalization();
                                let new_cap = base_bank.calculate_capitalization_for_tests();
                                base_bank.set_capitalization_for_tests(new_cap);
                                warn!(
                                    "Skewing capitalization a bit to enable credits_auto_rewind \
                                     as requested: increasing {feature_account_balance} from \
                                     {old_cap} to {new_cap}",
                                );
                                assert_eq!(
                                    old_cap + feature_account_balance * store_failed_count,
                                    new_cap
                                );
                            }
                        }

                        #[derive(Default, Debug)]
                        struct PointDetail {
                            epoch: Epoch,
                            points: u128,
                            stake: u128,
                            credits: u128,
                        }

                        #[derive(Default, Debug)]
                        struct CalculationDetail {
                            epochs: usize,
                            voter: Pubkey,
                            voter_owner: Pubkey,
                            current_effective_stake: u64,
                            total_stake: u64,
                            rent_exempt_reserve: u64,
                            points: Vec<PointDetail>,
                            base_rewards: u64,
                            commission: u8,
                            vote_rewards: u64,
                            stake_rewards: u64,
                            activation_epoch: Epoch,
                            deactivation_epoch: Option<Epoch>,
                            point_value: Option<PointValue>,
                            old_credits_observed: Option<u64>,
                            new_credits_observed: Option<u64>,
                            skipped_reasons: String,
                        }
                        let stake_calculation_details: DashMap<Pubkey, CalculationDetail> =
                            DashMap::new();
                        let last_point_value = Arc::new(RwLock::new(None));
                        let tracer = |event: &RewardCalculationEvent| {
                            // Currently RewardCalculationEvent enum has only Staking variant
                            // because only staking tracing is supported!
                            #[allow(irrefutable_let_patterns)]
                            if let RewardCalculationEvent::Staking(pubkey, event) = event {
                                let mut detail =
                                    stake_calculation_details.entry(**pubkey).or_default();
                                match event {
                                InflationPointCalculationEvent::CalculatedPoints(
                                    epoch,
                                    stake,
                                    credits,
                                    points,
                                ) => {
                                    if *points > 0 {
                                        detail.epochs += 1;
                                        detail.points.push(PointDetail {
                                            epoch: *epoch,
                                            points: *points,
                                            stake: *stake,
                                            credits: *credits,
                                        });
                                    }
                                }
                                InflationPointCalculationEvent::SplitRewards(
                                    all,
                                    voter,
                                    staker,
                                    point_value,
                                ) => {
                                    detail.base_rewards = *all;
                                    detail.vote_rewards = *voter;
                                    detail.stake_rewards = *staker;
                                    detail.point_value = Some(point_value.clone());
                                    // we have duplicate copies of `PointValue`s for possible
                                    // miscalculation; do some minimum sanity check
                                    let mut last_point_value = last_point_value.write().unwrap();
                                    if let Some(last_point_value) = last_point_value.as_ref() {
                                        assert_eq!(last_point_value, point_value);
                                    } else {
                                        *last_point_value = Some(point_value.clone());
                                    }
                                }
                                InflationPointCalculationEvent::EffectiveStakeAtRewardedEpoch(
                                    stake,
                                ) => {
                                    detail.current_effective_stake = *stake;
                                }
                                InflationPointCalculationEvent::Commission(commission) => {
                                    detail.commission = *commission;
                                }
                                InflationPointCalculationEvent::RentExemptReserve(reserve) => {
                                    detail.rent_exempt_reserve = *reserve;
                                }
                                InflationPointCalculationEvent::CreditsObserved(
                                    old_credits_observed,
                                    new_credits_observed,
                                ) => {
                                    detail.old_credits_observed = Some(*old_credits_observed);
                                    detail.new_credits_observed = *new_credits_observed;
                                }
                                InflationPointCalculationEvent::Delegation(delegation, owner) => {
                                    detail.voter = delegation.voter_pubkey;
                                    detail.voter_owner = *owner;
                                    detail.total_stake = delegation.stake;
                                    detail.activation_epoch = delegation.activation_epoch;
                                    if delegation.deactivation_epoch < Epoch::MAX {
                                        detail.deactivation_epoch =
                                            Some(delegation.deactivation_epoch);
                                    }
                                }
                                InflationPointCalculationEvent::Skipped(skipped_reason) => {
                                    if detail.skipped_reasons.is_empty() {
                                        detail.skipped_reasons = format!("{skipped_reason:?}");
                                    } else {
                                        use std::fmt::Write;
                                        let _ = write!(
                                            &mut detail.skipped_reasons,
                                            "/{skipped_reason:?}"
                                        );
                                    }
                                }
                            }
                            }
                        };
                        let warped_bank = Bank::new_from_parent_with_tracer(
                            base_bank.clone(),
                            base_bank.collector_id(),
                            next_epoch,
                            tracer,
                        );
                        warped_bank.freeze();
                        let mut csv_writer = if arg_matches.get_flag("csv_filename") {
                            let csv_filename =
                                arg_matches.get_one::<String>("csv_filename").unwrap().clone();
                            let file = File::create(csv_filename).unwrap();
                            Some(csv::WriterBuilder::new().from_writer(file))
                        } else {
                            None
                        };

                        println!("Slot: {} => {}", base_bank.slot(), warped_bank.slot());
                        println!("Epoch: {} => {}", base_bank.epoch(), warped_bank.epoch());
                        assert_capitalization(&base_bank);
                        assert_capitalization(&warped_bank);
                        let interest_per_epoch = ((warped_bank.capitalization() as f64)
                            / (base_bank.capitalization() as f64)
                            * 100_f64)
                            - 100_f64;
                        let interest_per_year = interest_per_epoch
                            / warped_bank.epoch_duration_in_years(base_bank.epoch());
                        println!(
                            "Capitalization: {} => {} (+{} {}%; annualized {}%)",
                            Sol(base_bank.capitalization()),
                            Sol(warped_bank.capitalization()),
                            Sol(warped_bank.capitalization() - base_bank.capitalization()),
                            interest_per_epoch,
                            interest_per_year,
                        );

                        let mut overall_delta = 0;

                        let modified_accounts =
                            warped_bank.get_all_accounts_modified_since_parent();
                        let mut rewarded_accounts = modified_accounts
                            .iter()
                            .map(|(pubkey, account)| {
                                (
                                    pubkey,
                                    account,
                                    base_bank
                                        .get_account(pubkey)
                                        .map(|a| a.lamports())
                                        .unwrap_or_default(),
                                )
                            })
                            .collect::<Vec<_>>();
                        rewarded_accounts.sort_unstable_by_key(
                            |(pubkey, account, base_lamports)| {
                                (
                                    *account.owner(),
                                    *base_lamports,
                                    account.lamports() - base_lamports,
                                    *pubkey,
                                )
                            },
                        );

                        let mut unchanged_accounts = stake_calculation_details
                            .iter()
                            .map(|entry| *entry.key())
                            .collect::<HashSet<_>>()
                            .difference(
                                &rewarded_accounts
                                    .iter()
                                    .map(|(pubkey, ..)| **pubkey)
                                    .collect(),
                            )
                            .map(|pubkey| (*pubkey, warped_bank.get_account(pubkey).unwrap()))
                            .collect::<Vec<_>>();
                        unchanged_accounts.sort_unstable_by_key(|(pubkey, account)| {
                            (*account.owner(), account.lamports(), *pubkey)
                        });
                        let unchanged_accounts = unchanged_accounts.into_iter();

                        let rewarded_accounts = rewarded_accounts
                            .into_iter()
                            .map(|(pubkey, account, ..)| (*pubkey, account.clone()));

                        let all_accounts = unchanged_accounts.chain(rewarded_accounts);
                        for (pubkey, warped_account) in all_accounts {
                            // Don't output sysvars; it's always updated but not related to
                            // inflation.
                            if solana_sdk_ids::sysvar::check_id(warped_account.owner()) {
                                continue;
                            }

                            if let Some(base_account) = base_bank.get_account(&pubkey) {
                                let delta = warped_account.lamports() - base_account.lamports();
                                let detail_ref = stake_calculation_details.get(&pubkey);
                                let detail: Option<&CalculationDetail> =
                                    detail_ref.as_ref().map(|detail_ref| detail_ref.value());
                                println!(
                                    "{:<45}({}): {} => {} (+{} {:>4.9}%) {:?}",
                                    format!("{pubkey}"), // format! is needed to pad/justify correctly.
                                    base_account.owner(),
                                    Sol(base_account.lamports()),
                                    Sol(warped_account.lamports()),
                                    Sol(delta),
                                    ((warped_account.lamports() as f64)
                                        / (base_account.lamports() as f64)
                                        * 100_f64)
                                        - 100_f64,
                                    detail,
                                );
                                if let Some(ref mut csv_writer) = csv_writer {
                                    #[derive(Serialize)]
                                    struct InflationRecord {
                                        cluster_type: String,
                                        rewarded_epoch: Epoch,
                                        account: String,
                                        owner: String,
                                        old_balance: u64,
                                        new_balance: u64,
                                        data_size: usize,
                                        delegation: String,
                                        delegation_owner: String,
                                        effective_stake: String,
                                        delegated_stake: String,
                                        rent_exempt_reserve: String,
                                        activation_epoch: String,
                                        deactivation_epoch: String,
                                        earned_epochs: String,
                                        epoch: String,
                                        epoch_credits: String,
                                        epoch_points: String,
                                        epoch_stake: String,
                                        old_credits_observed: String,
                                        new_credits_observed: String,
                                        base_rewards: String,
                                        stake_rewards: String,
                                        vote_rewards: String,
                                        commission: String,
                                        cluster_rewards: String,
                                        cluster_points: String,
                                        old_capitalization: u64,
                                        new_capitalization: u64,
                                    }
                                    fn format_or_na<T: std::fmt::Display>(
                                        data: Option<T>,
                                    ) -> String {
                                        data.map(|data| format!("{data}"))
                                            .unwrap_or_else(|| "N/A".to_owned())
                                    }
                                    let mut point_details = detail
                                        .map(|d| d.points.iter().map(Some).collect::<Vec<_>>())
                                        .unwrap_or_default();

                                    // ensure to print even if there is no calculation/point detail
                                    if point_details.is_empty() {
                                        point_details.push(None);
                                    }

                                    for point_detail in point_details {
                                        let (cluster_rewards, cluster_points) = last_point_value
                                            .read()
                                            .unwrap()
                                            .clone()
                                            .map_or((None, None), |pv| {
                                                (Some(pv.rewards), Some(pv.points))
                                            });
                                        let record = InflationRecord {
                                            cluster_type: format!("{:?}", base_bank.cluster_type()),
                                            rewarded_epoch: base_bank.epoch(),
                                            account: format!("{pubkey}"),
                                            owner: format!("{}", base_account.owner()),
                                            old_balance: base_account.lamports(),
                                            new_balance: warped_account.lamports(),
                                            data_size: base_account.data().len(),
                                            delegation: format_or_na(detail.map(|d| d.voter)),
                                            delegation_owner: format_or_na(
                                                detail.map(|d| d.voter_owner),
                                            ),
                                            effective_stake: format_or_na(
                                                detail.map(|d| d.current_effective_stake),
                                            ),
                                            delegated_stake: format_or_na(
                                                detail.map(|d| d.total_stake),
                                            ),
                                            rent_exempt_reserve: format_or_na(
                                                detail.map(|d| d.rent_exempt_reserve),
                                            ),
                                            activation_epoch: format_or_na(detail.map(|d| {
                                                if d.activation_epoch < Epoch::MAX {
                                                    d.activation_epoch
                                                } else {
                                                    // bootstraped
                                                    0
                                                }
                                            })),
                                            deactivation_epoch: format_or_na(
                                                detail.and_then(|d| d.deactivation_epoch),
                                            ),
                                            earned_epochs: format_or_na(detail.map(|d| d.epochs)),
                                            epoch: format_or_na(point_detail.map(|d| d.epoch)),
                                            epoch_credits: format_or_na(
                                                point_detail.map(|d| d.credits),
                                            ),
                                            epoch_points: format_or_na(
                                                point_detail.map(|d| d.points),
                                            ),
                                            epoch_stake: format_or_na(
                                                point_detail.map(|d| d.stake),
                                            ),
                                            old_credits_observed: format_or_na(
                                                detail.and_then(|d| d.old_credits_observed),
                                            ),
                                            new_credits_observed: format_or_na(
                                                detail.and_then(|d| d.new_credits_observed),
                                            ),
                                            base_rewards: format_or_na(
                                                detail.map(|d| d.base_rewards),
                                            ),
                                            stake_rewards: format_or_na(
                                                detail.map(|d| d.stake_rewards),
                                            ),
                                            vote_rewards: format_or_na(
                                                detail.map(|d| d.vote_rewards),
                                            ),
                                            commission: format_or_na(detail.map(|d| d.commission)),
                                            cluster_rewards: format_or_na(cluster_rewards),
                                            cluster_points: format_or_na(cluster_points),
                                            old_capitalization: base_bank.capitalization(),
                                            new_capitalization: warped_bank.capitalization(),
                                        };
                                        csv_writer.serialize(&record).unwrap();
                                    }
                                }
                                overall_delta += delta;
                            } else {
                                error!("new account!?: {pubkey}");
                            }
                        }
                        if overall_delta > 0 {
                            println!("Sum of lamports changes: {}", Sol(overall_delta));
                        }
                    } else {
                        if arg_matches.get_flag("recalculate_capitalization") {
                            eprintln!("Capitalization isn't verified because it's recalculated");
                        }
                        if arg_matches.get_flag("inflation") {
                            eprintln!(
                                "Forcing inflation isn't meaningful because bank isn't warping"
                            );
                        }

                        assert_capitalization(&bank);
                        println!("Inflation: {:?}", bank.inflation());
                        println!("Capitalization: {}", Sol(bank.capitalization()));
                    }
                }
                Some(("compute-slot-cost", arg_matches)) => {
                    let blockstore =
                        open_blockstore(&ledger_path, arg_matches, AccessType::Secondary);

                    let mut slots: Vec<u64> = vec![];
                    if !arg_matches.get_flag("slots") {
                        if let Ok(metas) = blockstore.slot_meta_iterator(0) {
                            slots = metas.map(|(slot, _)| slot).collect();
                        }
                    } else {
                        slots = arg_matches.get_many::<String>("slots").unwrap_or_else(|| std::process::exit(1)).map(|s| s.parse::<Slot>().unwrap()).collect::<Vec<_>>();
                    }
                    let allow_dead_slots = arg_matches.get_flag("allow_dead_slots");

                    for slot in slots {
                        if let Err(err) = compute_slot_cost(&blockstore, slot, allow_dead_slots) {
                            eprintln!("{err}");
                        }
                    }
                }
                Some(("", _)) => {
                    eprintln!("Usage: {} [OPTIONS] <SUBCOMMAND>", crate_name!());
                    exit(1);
                }
                _ => unreachable!(),
            };
        }
    };
    measure_total_execution_time.stop();
    info!("{measure_total_execution_time}");
}
