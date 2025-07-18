#![allow(clippy::arithmetic_side_effects)]
#[cfg(not(any(target_env = "msvc", target_os = "freebsd")))]
use jemallocator::Jemalloc;
use {
    gorb_validator::{
        cli::{app, warn_for_deprecated_arguments, DefaultArgs},
        commands,
    },
    log::error,
    solana_streamer::socket::SocketAddrSpace,
    std::{path::PathBuf, process::exit},
};

#[cfg(not(any(target_env = "msvc", target_os = "freebsd")))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

pub fn main() {
    let default_args = DefaultArgs::new();
    let solana_version = solana_version::version!();
    let cli_app = app(solana_version, &default_args);
    let matches = cli_app.get_matches();
    warn_for_deprecated_arguments(&matches);

    let socket_addr_space = SocketAddrSpace::new(matches.get_flag("allow_private_addr"));
    let ledger_path = PathBuf::from(matches.get_one::<String>("ledger_path").unwrap());

    match matches.subcommand() {
        Some(("init", _)) => commands::run::execute(
            &matches,
            solana_version,
            socket_addr_space,
            &ledger_path,
            commands::run::execute::Operation::Initialize,
        )
        .inspect_err(|err| error!("Failed to initialize validator: {err}"))
        .map_err(commands::Error::Dynamic),
        None | Some(("run", _)) => commands::run::execute(
            &matches,
            solana_version,
            socket_addr_space,
            &ledger_path,
            commands::run::execute::Operation::Run,
        )
        .inspect_err(|err| error!("Failed to start validator: {err}"))
        .map_err(commands::Error::Dynamic),
        Some(("authorized-voter", authorized_voter_subcommand_matches)) => {
            commands::authorized_voter::execute(authorized_voter_subcommand_matches, &ledger_path)
        }
        Some(("plugin", plugin_subcommand_matches)) => {
            commands::plugin::execute(plugin_subcommand_matches, &ledger_path)
        }
        Some(("contact-info", subcommand_matches)) => {
            commands::contact_info::execute(subcommand_matches, &ledger_path)
        }
        Some(("exit", subcommand_matches)) => {
            commands::exit::execute(subcommand_matches, &ledger_path)
        }
        Some(("monitor", _)) => commands::monitor::execute(&matches, &ledger_path),
        Some(("staked-nodes-overrides", subcommand_matches)) => {
            commands::staked_nodes_overrides::execute(subcommand_matches, &ledger_path)
        }
        Some(("set-identity", subcommand_matches)) => {
            commands::set_identity::execute(subcommand_matches, &ledger_path)
        }
        Some(("set-log-filter", subcommand_matches)) => {
            commands::set_log_filter::execute(subcommand_matches, &ledger_path)
        }
        Some(("wait-for-restart-window", subcommand_matches)) => {
            commands::wait_for_restart_window::execute(subcommand_matches, &ledger_path)
        }
        Some(("repair-shred-from-peer", subcommand_matches)) => {
            commands::repair_shred_from_peer::execute(subcommand_matches, &ledger_path)
        }
        Some(("repair-whitelist", repair_whitelist_subcommand_matches)) => {
            commands::repair_whitelist::execute(repair_whitelist_subcommand_matches, &ledger_path)
        }
        Some(("set-public-address", subcommand_matches)) => {
            commands::set_public_address::execute(subcommand_matches, &ledger_path)
        }
        _ => unreachable!(),
    }
    .unwrap_or_else(|err| {
        println!("Validator command failed: {err}");
        exit(1);
    })
}
