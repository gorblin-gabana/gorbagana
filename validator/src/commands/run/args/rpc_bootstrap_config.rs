use {
    crate::{
        bootstrap::RpcBootstrapConfig,
        commands::{FromClapArgMatches, Result},
    },
    clap::{ArgMatches},
};

#[cfg(test)]
impl Default for RpcBootstrapConfig {
    fn default() -> Self {
        Self {
            no_genesis_fetch: false,
            no_snapshot_fetch: false,
            check_vote_account: None,
            only_known_rpc: false,
            max_genesis_archive_unpacked_size: 10485760,
            incremental_snapshot_fetch: true,
        }
    }
}

impl FromClapArgMatches for RpcBootstrapConfig {
    fn from_clap_arg_match(matches: &ArgMatches) -> Result<Self> {
        let no_genesis_fetch = matches.get_flag("no_genesis_fetch");

        let no_snapshot_fetch = matches.get_flag("no_snapshot_fetch");

        let check_vote_account = matches
            .get_one::<String>("check_vote_account")
            .map(|url| url.to_string());

        let only_known_rpc = matches.get_flag("only_known_rpc");

        let max_genesis_archive_unpacked_size = matches
            .get_one::<String>("max_genesis_archive_unpacked_size")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or_else(|| {
                eprintln!("max_genesis_archive_unpacked_size is required");
                std::process::exit(1);
            });

        let no_incremental_snapshots = matches.get_flag("no_incremental_snapshots");

        Ok(Self {
            no_genesis_fetch,
            no_snapshot_fetch,
            check_vote_account,
            only_known_rpc,
            max_genesis_archive_unpacked_size,
            incremental_snapshot_fetch: !no_incremental_snapshots,
        })
    }
}
