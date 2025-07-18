//! A command-line executable for monitoring a cluster's gossip plane.

use {
    clap::{Arg, ArgAction, ArgMatches, Command},
    log::{error, info, warn},
    solana_clap_utils::{
        hidden_unless_forced,
        input_validators::{is_keypair_or_ask_keyword, is_port, is_pubkey},
    },
    solana_gossip::{contact_info::ContactInfo, gossip_service::discover},
    solana_pubkey::Pubkey,
    solana_streamer::socket::SocketAddrSpace,
    std::{
        error,
        net::{IpAddr, Ipv4Addr, SocketAddr},
        process::exit,
        time::Duration,
    },
};

fn parse_matches() -> ArgMatches {
    let shred_version_arg = Arg::new("shred_version")
        .long("shred-version")
        .value_name("VERSION")
        .default_value("0")
        .help("Filter gossip nodes by this shred version");

    let gossip_port_arg = clap::Arg::new("gossip_port")
        .long("gossip-port")
        .value_name("PORT")
        .value_parser(|s: &str| is_port(s.to_string()))
        .help("Gossip port number for the node");

    let gossip_host_arg = clap::Arg::new("gossip_host")
        .long("gossip-host")
        .value_name("HOST")
        .value_parser(|s: &str| solana_net_utils::is_host(s.to_string()))
        .help("DEPRECATED: --gossip-host is no longer supported. Use --bind-address instead.");

    let bind_address_arg = clap::Arg::new("bind_address")
        .long("bind-address")
        .value_name("HOST")
        .value_parser(|s: &str| solana_net_utils::is_host(s.to_string()))
        .help("IP address to bind the node to for gossip (replaces --gossip-host)");

    Command::new(env!("CARGO_PKG_NAME"))
        .about(env!("CARGO_PKG_DESCRIPTION"))
        .version("3.0.0")
        .subcommand_required(true)
        .arg(
            Arg::new("allow_private_addr")
                .long("allow-private-addr")
                .action(clap::ArgAction::SetTrue)
                .help("Allow contacting private ip addresses")
                .hide(hidden_unless_forced()),
        )
        .subcommand(
            Command::new("rpc-url")
                .about("Get an RPC URL for the cluster")
                .arg(
                    Arg::new("entrypoint")
                        .short('n')
                        .long("entrypoint")
                        .value_name("HOST:PORT")
                        .required(true)
                        .value_parser(|s: &str| solana_net_utils::is_host_port(s.to_string()))
                        .help("Rendezvous with the cluster at this entry point"),
                )
                .arg(
                    Arg::new("all")
                        .long("all")
                        .action(ArgAction::SetTrue)
                        .help("Return all RPC URLs"),
                )
                .arg(
                    Arg::new("any")
                        .long("any")
                        .action(ArgAction::SetTrue)
                        .conflicts_with("all")
                        .help("Return any RPC URL"),
                )
                .arg(
                    Arg::new("timeout")
                        .long("timeout")
                        .value_name("SECONDS")
                        .default_value("15")
                        .help("Timeout in seconds"),
                )
                .arg(&shred_version_arg)
                .arg(&gossip_port_arg)
                .arg(&gossip_host_arg)
                .arg(&bind_address_arg)
                .disable_version_flag(true),
        )
        .subcommand(
            Command::new("spy")
                .about("Monitor the gossip entrypoint")
                .disable_version_flag(true)
                .arg(
                    Arg::new("entrypoint")
                        .short('n')
                        .long("entrypoint")
                        .value_name("HOST:PORT")
                        .value_parser(|s: &str| solana_net_utils::is_host_port(s.to_string()))
                        .help("Rendezvous with the cluster at this entrypoint"),
                )
                .arg(
                    Arg::new("identity")
                        .short('i')
                        .long("identity")
                        .value_name("PATH")
                        .value_parser(|s: &str| is_keypair_or_ask_keyword(s.to_string()))
                        .help("Identity keypair [default: ephemeral keypair]"),
                )
                .arg(
                    Arg::new("num_nodes")
                        .short('N')
                        .long("num-nodes")
                        .value_name("NUM")
                        .conflicts_with("num_nodes_exactly")
                        .help("Wait for at least NUM nodes to be visible"),
                )
                .arg(
                    Arg::new("num_nodes_exactly")
                        .short('E')
                        .long("num-nodes-exactly")
                        .value_name("NUM")
                        .help("Wait for exactly NUM nodes to be visible"),
                )
                .arg(
                    Arg::new("node_pubkey")
                        .short('p')
                        .long("pubkey")
                        .value_name("PUBKEY")
                        .value_parser(|s: &str| is_pubkey(s.to_string()))
                        .action(ArgAction::Append)
                        .help("Public key of a specific node to wait for"),
                )
                .arg(&shred_version_arg)
                .arg(&gossip_port_arg)
                .arg(&gossip_host_arg)
                .arg(&bind_address_arg)
                .arg(
                    Arg::new("timeout")
                        .long("timeout")
                        .value_name("SECONDS")
                        .help("Maximum time to wait in seconds [default: wait forever]"),
                ),
        )
        .get_matches()
}

fn parse_bind_address(matches: &ArgMatches, entrypoint_addr: Option<SocketAddr>) -> IpAddr {
    if let Some(bind_address) = matches.get_one::<String>("bind_address") {
        solana_net_utils::parse_host(bind_address).unwrap_or_else(|e| {
            eprintln!("failed to parse bind-address: {e}");
            exit(1);
        })
    } else if let Some(gossip_host) = matches.get_one::<String>("gossip_host") {
        warn!("--gossip-host is deprecated. Use --bind-address instead.");
        solana_net_utils::parse_host(gossip_host).unwrap_or_else(|e| {
            eprintln!("failed to parse gossip-host: {e}");
            exit(1);
        })
    } else if let Some(entrypoint_addr) = entrypoint_addr {
        solana_net_utils::get_public_ip_addr_with_binding(
            &entrypoint_addr,
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        )
        .unwrap_or_else(|err| {
            eprintln!("Failed to contact cluster entrypoint {entrypoint_addr}: {err}");
            exit(1);
        })
    } else {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    }
}

fn process_spy_results(
    timeout: Option<u64>,
    validators: Vec<ContactInfo>,
    num_nodes: Option<usize>,
    num_nodes_exactly: Option<usize>,
    pubkeys: Option<&[Pubkey]>,
) {
    if timeout.is_some() {
        if let Some(num) = num_nodes {
            if validators.len() < num {
                let add = if num_nodes_exactly.is_some() {
                    ""
                } else {
                    " or more"
                };
                eprintln!("Error: Insufficient validators discovered.  Expecting {num}{add}",);
                exit(1);
            }
        }
        if let Some(nodes) = pubkeys {
            for node in nodes {
                if !validators.iter().any(|x| x.pubkey() == node) {
                    eprintln!("Error: Could not find node {node:?}");
                    exit(1);
                }
            }
        }
    }
    if let Some(num_nodes_exactly) = num_nodes_exactly {
        if validators.len() > num_nodes_exactly {
            eprintln!("Error: Extra nodes discovered.  Expecting exactly {num_nodes_exactly}");
            exit(1);
        }
    }
}

fn get_entrypoint_shred_version(entrypoint: &Option<SocketAddr>) -> Option<u16> {
    let Some(entrypoint) = entrypoint else {
        error!("cannot obtain shred-version without an entrypoint");
        return None;
    };
    match solana_net_utils::get_cluster_shred_version(entrypoint) {
        Err(err) => {
            error!("get_cluster_shred_version failed: {entrypoint}, {err}");
            None
        }
        Ok(0) => {
            error!("entrypoint {entrypoint} returned shred-version zero");
            None
        }
        Ok(shred_version) => {
            info!("obtained shred-version {shred_version} from entrypoint: {entrypoint}");
            Some(shred_version)
        }
    }
}

fn process_spy(matches: &ArgMatches, socket_addr_space: SocketAddrSpace) -> std::io::Result<()> {
    let num_nodes_exactly = matches
        .get_one::<String>("num_nodes_exactly")
        .map(|num| num.parse().unwrap());
    let num_nodes = matches
        .get_one::<String>("num_nodes")
        .map(|num| num.parse().unwrap())
        .or(num_nodes_exactly);
    let timeout = matches
        .get_one::<String>("timeout")
        .map(|secs| secs.parse().unwrap());
    let pubkeys: Option<Vec<Pubkey>> = matches
        .get_many::<String>("node_pubkey")
        .map(|values| {
            values
                .map(|value| value.parse::<Pubkey>().unwrap())
                .collect()
        });
    let identity_keypair = matches
        .get_one::<String>("identity")
        .map(|value| {
            if value == "ASK" {
                // Handle ask keyword - for now just return None
                None
            } else {
                solana_keypair::read_keypair_file(value).ok()
            }
        })
        .flatten();
    let entrypoint_addr = parse_entrypoint(matches);
    let gossip_addr = get_gossip_address(matches, entrypoint_addr);

    let mut shred_version = matches
        .get_one::<String>("shred_version")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    if shred_version == 0 {
        shred_version = get_entrypoint_shred_version(&entrypoint_addr)
            .expect("need non-zero shred-version to join the cluster");
    }

    let discover_timeout = Duration::from_secs(timeout.unwrap_or(u64::MAX));
    let (_all_peers, validators) = discover(
        identity_keypair,
        entrypoint_addr.as_ref(),
        num_nodes,
        discover_timeout,
        pubkeys.as_deref(), // find_nodes_by_pubkey
        None,               // find_node_by_gossip_addr
        Some(&gossip_addr), // my_gossip_addr
        shred_version,
        socket_addr_space,
    )?;

    process_spy_results(
        timeout,
        validators,
        num_nodes,
        num_nodes_exactly,
        pubkeys.as_deref(),
    );

    Ok(())
}

fn parse_entrypoint(matches: &ArgMatches) -> Option<SocketAddr> {
    matches.get_one::<String>("entrypoint").map(|entrypoint| {
        solana_net_utils::parse_host_port(entrypoint).unwrap_or_else(|e| {
            eprintln!("failed to parse entrypoint address: {e}");
            exit(1);
        })
    })
}

fn process_rpc_url(
    matches: &ArgMatches,
    socket_addr_space: SocketAddrSpace,
) -> std::io::Result<()> {
    let any = matches.get_flag("any");
    let all = matches.get_flag("all");
    let timeout = matches
        .get_one::<String>("timeout")
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let entrypoint_addr = parse_entrypoint(matches);
    let gossip_addr = get_gossip_address(matches, entrypoint_addr);

    let mut shred_version = matches
        .get_one::<String>("shred_version")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    if shred_version == 0 {
        shred_version = get_entrypoint_shred_version(&entrypoint_addr)
            .expect("need non-zero shred-version to join the cluster");
    }

    let (_all_peers, validators) = discover(
        None, // keypair
        entrypoint_addr.as_ref(),
        Some(1), // num_nodes
        Duration::from_secs(timeout),
        None,                     // find_nodes_by_pubkey
        entrypoint_addr.as_ref(), // find_node_by_gossip_addr
        Some(&gossip_addr),       // my_gossip_addr
        shred_version,
        socket_addr_space,
    )?;

    let rpc_addrs: Vec<_> = validators
        .iter()
        .filter(|node| {
            any || all
                || node
                    .gossip()
                    .map(|addr| Some(addr) == entrypoint_addr)
                    .unwrap_or_default()
        })
        .filter_map(ContactInfo::rpc)
        .filter(|addr| socket_addr_space.check(addr))
        .collect();

    if rpc_addrs.is_empty() {
        eprintln!("No RPC URL found");
        exit(1);
    }

    for rpc_addr in rpc_addrs {
        println!("http://{rpc_addr}");
        if any {
            break;
        }
    }

    Ok(())
}

fn get_gossip_address(matches: &ArgMatches, entrypoint_addr: Option<SocketAddr>) -> SocketAddr {
    let bind_address = parse_bind_address(matches, entrypoint_addr);
    SocketAddr::new(
        bind_address,
        matches
            .get_one::<String>("gossip_port")
            .map(|s| s.parse::<u16>().unwrap())
            .unwrap_or_else(|| {
            solana_net_utils::find_available_port_in_range(
                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                (0, 1),
            )
            .expect("unable to find an available gossip port")
        }),
    )
}

fn main() -> Result<(), Box<dyn error::Error>> {
    solana_logger::setup_with_default_filter();

    let matches = parse_matches();
    let socket_addr_space = SocketAddrSpace::new(matches.get_flag("allow_private_addr"));
    match matches.subcommand() {
        Some(("spy", matches)) => {
            process_spy(matches, socket_addr_space)?;
        }
        Some(("rpc-url", matches)) => {
            process_rpc_url(matches, socket_addr_space)?;
        }
        _ => unreachable!(),
    }

    Ok(())
}
