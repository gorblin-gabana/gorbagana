//! Arguments for controlling the number of threads allocated for various tasks

use {
    clap::{Arg, ArgMatches},
    solana_accounts_db::{accounts_db, accounts_index},
    solana_clap_utils::{hidden_unless_forced, input_validators::is_within_range},
    solana_rayon_threadlimit::{get_max_thread_count, get_thread_count},
    std::{num::NonZeroUsize, ops::RangeInclusive},
};

// Need this struct to provide &str whose lifetime matches that of the CLAP Arg's
pub struct DefaultThreadArgs {
    pub accounts_db_clean_threads: String,
    pub accounts_db_foreground_threads: String,
    pub accounts_db_hash_threads: String,
    pub accounts_index_flush_threads: String,
    pub ip_echo_server_threads: String,
    pub rayon_global_threads: String,
    pub replay_forks_threads: String,
    pub replay_transactions_threads: String,
    pub rocksdb_compaction_threads: String,
    pub rocksdb_flush_threads: String,
    pub tpu_transaction_forward_receive_threads: String,
    pub tpu_transaction_receive_threads: String,
    pub tpu_vote_transaction_receive_threads: String,
    pub tvu_receive_threads: String,
    pub tvu_retransmit_threads: String,
    pub tvu_sigverify_threads: String,
}

impl Default for DefaultThreadArgs {
    fn default() -> Self {
        Self {
            accounts_db_clean_threads: AccountsDbCleanThreadsArg::bounded_default().to_string(),
            accounts_db_foreground_threads: AccountsDbForegroundThreadsArg::bounded_default()
                .to_string(),
            accounts_db_hash_threads: AccountsDbHashThreadsArg::bounded_default().to_string(),
            accounts_index_flush_threads: AccountsIndexFlushThreadsArg::bounded_default()
                .to_string(),
            ip_echo_server_threads: IpEchoServerThreadsArg::bounded_default().to_string(),
            rayon_global_threads: RayonGlobalThreadsArg::bounded_default().to_string(),
            replay_forks_threads: ReplayForksThreadsArg::bounded_default().to_string(),
            replay_transactions_threads: ReplayTransactionsThreadsArg::bounded_default()
                .to_string(),
            rocksdb_compaction_threads: RocksdbCompactionThreadsArg::bounded_default().to_string(),
            rocksdb_flush_threads: RocksdbFlushThreadsArg::bounded_default().to_string(),
            tpu_transaction_forward_receive_threads:
                TpuTransactionForwardReceiveThreadArgs::bounded_default().to_string(),
            tpu_transaction_receive_threads: TpuTransactionReceiveThreads::bounded_default()
                .to_string(),
            tpu_vote_transaction_receive_threads:
                TpuVoteTransactionReceiveThreads::bounded_default().to_string(),
            tvu_receive_threads: TvuReceiveThreadsArg::bounded_default().to_string(),
            tvu_retransmit_threads: TvuRetransmitThreadsArg::bounded_default().to_string(),
            tvu_sigverify_threads: TvuShredSigverifyThreadsArg::bounded_default().to_string(),
        }
    }
}

pub fn thread_args<'a>(defaults: &DefaultThreadArgs) -> Vec<Arg> {
    vec![
        new_thread_arg::<AccountsDbCleanThreadsArg>(Box::leak(Box::new(defaults.accounts_db_clean_threads.clone()))),
        new_thread_arg::<AccountsDbForegroundThreadsArg>(Box::leak(Box::new(defaults.accounts_db_foreground_threads.clone()))),
        new_thread_arg::<AccountsDbHashThreadsArg>(Box::leak(Box::new(defaults.accounts_db_hash_threads.clone()))),
        new_thread_arg::<AccountsIndexFlushThreadsArg>(Box::leak(Box::new(defaults.accounts_index_flush_threads.clone()))),
        new_thread_arg::<IpEchoServerThreadsArg>(Box::leak(Box::new(defaults.ip_echo_server_threads.clone()))),
        new_thread_arg::<RayonGlobalThreadsArg>(Box::leak(Box::new(defaults.rayon_global_threads.clone()))),
        new_thread_arg::<ReplayForksThreadsArg>(Box::leak(Box::new(defaults.replay_forks_threads.clone()))),
        new_thread_arg::<ReplayTransactionsThreadsArg>(Box::leak(Box::new(defaults.replay_transactions_threads.clone()))),
        new_thread_arg::<RocksdbCompactionThreadsArg>(Box::leak(Box::new(defaults.rocksdb_compaction_threads.clone()))),
        new_thread_arg::<RocksdbFlushThreadsArg>(Box::leak(Box::new(defaults.rocksdb_flush_threads.clone()))),
        new_thread_arg::<TpuTransactionForwardReceiveThreadArgs>(
            Box::leak(Box::new(defaults.tpu_transaction_forward_receive_threads.clone())),
        ),
        new_thread_arg::<TpuTransactionReceiveThreads>(Box::leak(Box::new(defaults.tpu_transaction_receive_threads.clone()))),
        new_thread_arg::<TpuVoteTransactionReceiveThreads>(
            Box::leak(Box::new(defaults.tpu_vote_transaction_receive_threads.clone())),
        ),
        new_thread_arg::<TvuReceiveThreadsArg>(Box::leak(Box::new(defaults.tvu_receive_threads.clone()))),
        new_thread_arg::<TvuRetransmitThreadsArg>(Box::leak(Box::new(defaults.tvu_retransmit_threads.clone()))),
        new_thread_arg::<TvuShredSigverifyThreadsArg>(Box::leak(Box::new(defaults.tvu_sigverify_threads.clone()))),
    ]
}

fn new_thread_arg<'a, T: ThreadArg>(default: &'static str) -> Arg {
    Arg::new(T::NAME)
        .long(T::LONG_NAME)
        
        .value_name("NUMBER")
        .default_value(default)
        .value_parser(|num: &str| is_within_range(num.to_string(), T::range()))
        .hide(hidden_unless_forced())
        .help(T::HELP)
}

pub struct NumThreadConfig {
    pub accounts_db_clean_threads: NonZeroUsize,
    pub accounts_db_foreground_threads: NonZeroUsize,
    pub accounts_db_hash_threads: NonZeroUsize,
    pub accounts_index_flush_threads: NonZeroUsize,
    pub ip_echo_server_threads: NonZeroUsize,
    pub rayon_global_threads: NonZeroUsize,
    pub replay_forks_threads: NonZeroUsize,
    pub replay_transactions_threads: NonZeroUsize,
    pub rocksdb_compaction_threads: NonZeroUsize,
    pub rocksdb_flush_threads: NonZeroUsize,
    pub tpu_transaction_forward_receive_threads: NonZeroUsize,
    pub tpu_transaction_receive_threads: NonZeroUsize,
    pub tpu_vote_transaction_receive_threads: NonZeroUsize,
    pub tvu_receive_threads: NonZeroUsize,
    pub tvu_retransmit_threads: NonZeroUsize,
    pub tvu_sigverify_threads: NonZeroUsize,
}

pub fn parse_num_threads_args(matches: &ArgMatches) -> NumThreadConfig {
    NumThreadConfig {
        accounts_db_clean_threads: matches
            .get_one::<String>(AccountsDbCleanThreadsArg::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", AccountsDbCleanThreadsArg::NAME);
                std::process::exit(1);
            }),
        accounts_db_foreground_threads: matches
            .get_one::<String>(AccountsDbForegroundThreadsArg::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", AccountsDbForegroundThreadsArg::NAME);
                std::process::exit(1);
            }),
        accounts_db_hash_threads: matches
            .get_one::<String>(AccountsDbHashThreadsArg::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", AccountsDbHashThreadsArg::NAME);
                std::process::exit(1);
            }),
        accounts_index_flush_threads: matches
            .get_one::<String>(AccountsIndexFlushThreadsArg::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", AccountsIndexFlushThreadsArg::NAME);
                std::process::exit(1);
            }),
        ip_echo_server_threads: matches
            .get_one::<String>(IpEchoServerThreadsArg::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", IpEchoServerThreadsArg::NAME);
                std::process::exit(1);
            }),
        rayon_global_threads: matches
            .get_one::<String>(RayonGlobalThreadsArg::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", RayonGlobalThreadsArg::NAME);
                std::process::exit(1);
            }),
        replay_forks_threads: matches
            .get_one::<String>(ReplayForksThreadsArg::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", ReplayForksThreadsArg::NAME);
                std::process::exit(1);
            }),
        replay_transactions_threads: matches
            .get_one::<String>(ReplayTransactionsThreadsArg::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", ReplayTransactionsThreadsArg::NAME);
                std::process::exit(1);
            }),
        rocksdb_compaction_threads: matches
            .get_one::<String>(RocksdbCompactionThreadsArg::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", RocksdbCompactionThreadsArg::NAME);
                std::process::exit(1);
            }),
        rocksdb_flush_threads: matches
            .get_one::<String>(RocksdbFlushThreadsArg::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", RocksdbFlushThreadsArg::NAME);
                std::process::exit(1);
            }),
        tpu_transaction_forward_receive_threads: matches
            .get_one::<String>(TpuTransactionForwardReceiveThreadArgs::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", TpuTransactionForwardReceiveThreadArgs::NAME);
                std::process::exit(1);
            }),
        tpu_transaction_receive_threads: matches
            .get_one::<String>(TpuTransactionReceiveThreads::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", TpuTransactionReceiveThreads::NAME);
                std::process::exit(1);
            }),
        tpu_vote_transaction_receive_threads: matches
            .get_one::<String>(TpuVoteTransactionReceiveThreads::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", TpuVoteTransactionReceiveThreads::NAME);
                std::process::exit(1);
            }),
        tvu_receive_threads: matches
            .get_one::<String>(TvuReceiveThreadsArg::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", TvuReceiveThreadsArg::NAME);
                std::process::exit(1);
            }),
        tvu_retransmit_threads: matches
            .get_one::<String>(TvuRetransmitThreadsArg::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", TvuRetransmitThreadsArg::NAME);
                std::process::exit(1);
            }),
        tvu_sigverify_threads: matches
            .get_one::<String>(TvuShredSigverifyThreadsArg::NAME)
            .and_then(|s| s.parse::<usize>().ok())
            .and_then(NonZeroUsize::new)
            .unwrap_or_else(|| {
                eprintln!("{} is required", TvuShredSigverifyThreadsArg::NAME);
                std::process::exit(1);
            }),
    }
}

/// Configuration for CLAP arguments that control the number of threads for various functions
trait ThreadArg {
    /// The argument's name
    const NAME: &'static str;
    /// The argument's long name
    const LONG_NAME: &'static str;
    /// The argument's help message
    const HELP: &'static str;

    /// The default number of threads
    fn default() -> usize;
    /// The default number of threads, bounded by Self::max()
    /// This prevents potential CLAP issues on low core count machines where
    /// a fixed value in Self::default() could be greater than Self::max()
    fn bounded_default() -> usize {
        std::cmp::min(Self::default(), Self::max())
    }
    /// The minimum allowed number of threads (inclusive)
    fn min() -> usize {
        1
    }
    /// The maximum allowed number of threads (inclusive)
    fn max() -> usize {
        // By default, no thread pool should scale over the number of the machine's threads
        get_max_thread_count()
    }
    /// The range of allowed number of threads (inclusive on both ends)
    fn range() -> RangeInclusive<usize> {
        RangeInclusive::new(Self::min(), Self::max())
    }
}

struct AccountsDbCleanThreadsArg;
impl ThreadArg for AccountsDbCleanThreadsArg {
    const NAME: &'static str = "accounts_db_clean_threads";
    const LONG_NAME: &'static str = "accounts-db-clean-threads";
    const HELP: &'static str = "Number of threads to use for cleaning AccountsDb";

    fn default() -> usize {
        accounts_db::quarter_thread_count()
    }
}

struct AccountsDbForegroundThreadsArg;
impl ThreadArg for AccountsDbForegroundThreadsArg {
    const NAME: &'static str = "accounts_db_foreground_threads";
    const LONG_NAME: &'static str = "accounts-db-foreground-threads";
    const HELP: &'static str = "Number of threads to use for AccountsDb block processing";

    fn default() -> usize {
        accounts_db::default_num_foreground_threads()
    }
}

struct AccountsDbHashThreadsArg;
impl ThreadArg for AccountsDbHashThreadsArg {
    const NAME: &'static str = "accounts_db_hash_threads";
    const LONG_NAME: &'static str = "accounts-db-hash-threads";
    const HELP: &'static str = "Number of threads to use for background accounts hashing";

    fn default() -> usize {
        accounts_db::default_num_hash_threads().get()
    }
}

struct AccountsIndexFlushThreadsArg;
impl ThreadArg for AccountsIndexFlushThreadsArg {
    const NAME: &'static str = "accounts_index_flush_threads";
    const LONG_NAME: &'static str = "accounts-index-flush-threads";
    const HELP: &'static str = "Number of threads to use for flushing the accounts index";

    fn default() -> usize {
        accounts_index::default_num_flush_threads().get()
    }
}

struct IpEchoServerThreadsArg;
impl ThreadArg for IpEchoServerThreadsArg {
    const NAME: &'static str = "ip_echo_server_threads";
    const LONG_NAME: &'static str = "ip-echo-server-threads";
    const HELP: &'static str = "Number of threads to use for the IP echo server";

    fn default() -> usize {
        solana_net_utils::DEFAULT_IP_ECHO_SERVER_THREADS.get()
    }
    fn min() -> usize {
        solana_net_utils::MINIMUM_IP_ECHO_SERVER_THREADS.get()
    }
}

struct RayonGlobalThreadsArg;
impl ThreadArg for RayonGlobalThreadsArg {
    const NAME: &'static str = "rayon_global_threads";
    const LONG_NAME: &'static str = "rayon-global-threads";
    const HELP: &'static str = "Number of threads to use for the global rayon thread pool";

    fn default() -> usize {
        get_max_thread_count()
    }
}

struct ReplayForksThreadsArg;
impl ThreadArg for ReplayForksThreadsArg {
    const NAME: &'static str = "replay_forks_threads";
    const LONG_NAME: &'static str = "replay-forks-threads";
    const HELP: &'static str = "Number of threads to use for replay of blocks on different forks";

    fn default() -> usize {
        // Default to single threaded fork execution
        1
    }
    fn max() -> usize {
        // Choose a value that is small enough to limit the overhead of having a large thread pool
        // while also being large enough to allow replay of all active forks in most scenarios
        4
    }
}

struct ReplayTransactionsThreadsArg;
impl ThreadArg for ReplayTransactionsThreadsArg {
    const NAME: &'static str = "replay_transactions_threads";
    const LONG_NAME: &'static str = "replay-transactions-threads";
    const HELP: &'static str = "Number of threads to use for transaction replay";

    fn default() -> usize {
        get_max_thread_count()
    }
}

struct RocksdbCompactionThreadsArg;
impl ThreadArg for RocksdbCompactionThreadsArg {
    const NAME: &'static str = "rocksdb_compaction_threads";
    const LONG_NAME: &'static str = "rocksdb-compaction-threads";
    const HELP: &'static str = "Number of threads to use for rocksdb (Blockstore) compactions";

    fn default() -> usize {
        solana_ledger::blockstore::default_num_compaction_threads().get()
    }
}

struct RocksdbFlushThreadsArg;
impl ThreadArg for RocksdbFlushThreadsArg {
    const NAME: &'static str = "rocksdb_flush_threads";
    const LONG_NAME: &'static str = "rocksdb-flush-threads";
    const HELP: &'static str = "Number of threads to use for rocksdb (Blockstore) memtable flushes";

    fn default() -> usize {
        solana_ledger::blockstore::default_num_flush_threads().get()
    }
}

struct TpuTransactionForwardReceiveThreadArgs;
impl ThreadArg for TpuTransactionForwardReceiveThreadArgs {
    const NAME: &'static str = "tpu_transaction_forward_receive_threads";
    const LONG_NAME: &'static str = "tpu-transaction-forward-receive-threads";
    const HELP: &'static str =
        "Number of threads to use for receiving transactions on the TPU fowards port";

    fn default() -> usize {
        solana_streamer::quic::default_num_tpu_transaction_forward_receive_threads()
    }
}

struct TpuTransactionReceiveThreads;
impl ThreadArg for TpuTransactionReceiveThreads {
    const NAME: &'static str = "tpu_transaction_receive_threads";
    const LONG_NAME: &'static str = "tpu-transaction-receive-threads";
    const HELP: &'static str =
        "Number of threads to use for receiving transactions on the TPU port";

    fn default() -> usize {
        solana_streamer::quic::default_num_tpu_transaction_receive_threads()
    }
}

struct TpuVoteTransactionReceiveThreads;
impl ThreadArg for TpuVoteTransactionReceiveThreads {
    const NAME: &'static str = "tpu_vote_transaction_receive_threads";
    const LONG_NAME: &'static str = "tpu-vote-transaction-receive-threads";
    const HELP: &'static str =
        "Number of threads to use for receiving transactions on the TPU vote port";

    fn default() -> usize {
        solana_streamer::quic::default_num_tpu_vote_transaction_receive_threads()
    }
}

struct TvuReceiveThreadsArg;
impl ThreadArg for TvuReceiveThreadsArg {
    const NAME: &'static str = "tvu_receive_threads";
    const LONG_NAME: &'static str = "tvu-receive-threads";
    const HELP: &'static str =
        "Number of threads (and sockets) to use for receiving shreds on the TVU port";

    fn default() -> usize {
        solana_gossip::cluster_info::DEFAULT_NUM_TVU_RECEIVE_SOCKETS.get()
    }
    fn min() -> usize {
        solana_gossip::cluster_info::MINIMUM_NUM_TVU_RECEIVE_SOCKETS.get()
    }
}

struct TvuRetransmitThreadsArg;
impl ThreadArg for TvuRetransmitThreadsArg {
    const NAME: &'static str = "tvu_retransmit_threads";
    const LONG_NAME: &'static str = "tvu-retransmit-threads";
    const HELP: &'static str = "Number of threads (and sockets) to use for retransmitting shreds";

    fn default() -> usize {
        solana_gossip::cluster_info::DEFAULT_NUM_TVU_RETRANSMIT_SOCKETS.get()
    }

    fn min() -> usize {
        solana_gossip::cluster_info::MINIMUM_NUM_TVU_RETRANSMIT_SOCKETS.get()
    }
}

struct TvuShredSigverifyThreadsArg;
impl ThreadArg for TvuShredSigverifyThreadsArg {
    const NAME: &'static str = "tvu_shred_sigverify_threads";
    const LONG_NAME: &'static str = "tvu-shred-sigverify-threads";
    const HELP: &'static str =
        "Number of threads to use for performing signature verification of received shreds";

    fn default() -> usize {
        get_thread_count()
    }
}
