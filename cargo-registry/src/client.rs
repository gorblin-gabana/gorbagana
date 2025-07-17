use {
    clap::{crate_description, crate_name, Command as App, Arg, ArgMatches, ArgAction},
    solana_clap_utils::{
        hidden_unless_forced,
        input_validators::is_url_or_moniker,
        keypair::{DefaultSigner, SignerIndex},
    },
    solana_cli::cli::{CliConfig, DEFAULT_CONFIRM_TX_TIMEOUT_SECONDS, DEFAULT_RPC_TIMEOUT_SECONDS},
    solana_cli_config::{Config, ConfigInput},
    solana_commitment_config::CommitmentConfig,
    solana_keypair::{read_keypair_file, Keypair},
    solana_rpc_client::rpc_client::RpcClient,
    solana_rpc_client_api::config::RpcSendTransactionConfig,
    std::{error, sync::Arc, time::Duration},
};

pub(crate) struct Client {
    pub rpc_client: Arc<RpcClient>,
    pub port: u16,
    pub server_url: String,
    websocket_url: String,
    commitment: CommitmentConfig,
    cli_signers: Vec<Keypair>,
    pub authority_signer_index: SignerIndex,
    send_transaction_config: RpcSendTransactionConfig,
}

impl Client {
    pub fn get_cli_config(&'_ self) -> CliConfig<'_> {
        CliConfig {
            websocket_url: self.websocket_url.clone(),
            commitment: self.commitment,
            signers: vec![&self.cli_signers[0], &self.cli_signers[1]],
            send_transaction_config: self.send_transaction_config,
            ..CliConfig::default()
        }
    }

    fn get_keypair(
        matches: &ArgMatches,
        config_path: &str,
        name: &str,
    ) -> Result<Keypair, Box<dyn error::Error>> {
        let (_, default_signer_path) = ConfigInput::compute_keypair_path_setting(
            matches.get_one::<String>(name).map(|s| s.as_str()).unwrap_or(""),
            config_path,
        );

        let default_signer = DefaultSigner::new(name, default_signer_path);

        read_keypair_file(default_signer.path)
    }

    pub(crate) fn new() -> Result<Client, Box<dyn error::Error>> {
        let matches = App::new(crate_name!())
            .about(crate_description!())
            .version("3.0.0")
            .arg(
                Arg::new("skip_preflight")
                    .long("skip-preflight")
                    .global(true)
                    .action(ArgAction::SetTrue)
                    .help("Skip the preflight check when sending transactions"),
            )
            .arg(
                Arg::new("config_file")
                    .short('C')
                    .long("config")
                    .value_name("FILEPATH")
                    
                    .global(true)
                    .help("Configuration file to use"),
            )
            .arg(
                Arg::new("json_rpc_url")
                    .short('u')
                    .long("url")
                    .value_name("URL_OR_MONIKER")
                    
                    .global(true)
                    .value_parser(|s: &str| {
                        is_url_or_moniker(s.to_string()).map(|_| s.to_string())
                    })
                    .help(
                        "URL for Solana's JSON RPC or moniker (or their first letter): \
                       [mainnet-beta, testnet, devnet, localhost]",
                    ),
            )
            .arg(
                Arg::new("keypair")
                    .short('k')
                    .long("keypair")
                    .value_name("KEYPAIR")
                    .global(true)
                    
                    .help("Filepath or URL to a keypair"),
            )
            .arg(
                Arg::new("authority")
                    .short('a')
                    .long("authority")
                    .value_name("KEYPAIR")
                    .global(true)
                    
                    .help("Authority's keypair used to manage the registry"),
            )
            .arg(
                Arg::new("port")
                    .short('p')
                    .long("port")
                    .value_name("PORT")
                    .default_value("3030")
                    .global(true)
                    
                    .help("Cargo registry's local TCP port. The server will bind to this port and wait for requests."),
            )
            .arg(
                Arg::new("server_url")
                    .short('s')
                    .long("server-url")
                    .value_name("URL_OR_MONIKER")
                    
                    .global(true)
                    .value_parser(|s: &str| {
                        is_url_or_moniker(s.to_string()).map(|_| s.to_string())
                    })
                    .help(
                        "URL where the registry service will be hosted. Default: http://0.0.0.0:<port>",
                    ),
            )
            .arg(
                Arg::new("commitment")
                    .long("commitment")
                    
                    .value_parser(["processed", "confirmed", "finalized"])
                    .value_name("COMMITMENT_LEVEL")
                    .hide_possible_values(true)
                    .global(true)
                    .help("Return information at the selected commitment level [possible values: processed, confirmed, finalized]"),
            )
            .arg(
                Arg::new("rpc_timeout")
                    .long("rpc-timeout")
                    .value_name("SECONDS")
                    
                    .default_value(DEFAULT_RPC_TIMEOUT_SECONDS)
                    .global(true)
                    .hide(hidden_unless_forced())
                    .help("Timeout value for RPC requests"),
            )
            .arg(
                Arg::new("confirm_transaction_initial_timeout")
                    .long("confirm-timeout")
                    .value_name("SECONDS")
                    
                    .default_value(DEFAULT_CONFIRM_TX_TIMEOUT_SECONDS)
                    .global(true)
                    .hide(hidden_unless_forced())
                    .help("Timeout value for initial transaction status"),
            )
            .get_matches();

        let cli_config = if let Some(config_file) = matches.get_one::<String>("config_file") {
            Config::load(config_file).unwrap_or_default()
        } else {
            Config::default()
        };

        let (_, json_rpc_url) = ConfigInput::compute_json_rpc_url_setting(
            matches.get_one::<String>("json_rpc_url").map(|s| s.as_str()).unwrap_or(""),
            &cli_config.json_rpc_url,
        );

        let (_, websocket_url) = ConfigInput::compute_websocket_url_setting(
            matches.get_one::<String>("websocket_url").map(|s| s.as_str()).unwrap_or(""),
            &cli_config.websocket_url,
            matches.get_one::<String>("json_rpc_url").map(|s| s.as_str()).unwrap_or(""),
            &cli_config.json_rpc_url,
        );

        let (_, commitment) = ConfigInput::compute_commitment_config(
            matches.get_one::<String>("commitment").map(|s| s.as_str()).unwrap_or(""),
            &cli_config.commitment,
        );

        let rpc_timeout = *matches.get_one::<u64>("rpc_timeout").unwrap();
        let rpc_timeout = Duration::from_secs(rpc_timeout);

        let confirm_transaction_initial_timeout =
            *matches.get_one::<u64>("confirm_transaction_initial_timeout").unwrap();
        let confirm_transaction_initial_timeout =
            Duration::from_secs(confirm_transaction_initial_timeout);

        let payer_keypair = Self::get_keypair(&matches, &cli_config.keypair_path, "keypair")?;
        let authority_keypair = Self::get_keypair(&matches, &cli_config.keypair_path, "authority")?;

        let port = *matches.get_one::<u16>("port").unwrap();

        let server_url =
            matches.get_one::<String>("server_url").cloned().unwrap_or(format!("http://0.0.0.0:{port}"));

        let skip_preflight = matches.get_flag("skip_preflight");

        Ok(Client {
            rpc_client: Arc::new(RpcClient::new_with_timeouts_and_commitment(
                json_rpc_url.to_string(),
                rpc_timeout,
                commitment,
                confirm_transaction_initial_timeout,
            )),
            port,
            server_url,
            websocket_url,
            commitment,
            cli_signers: vec![payer_keypair, authority_keypair],
            authority_signer_index: 1,
            send_transaction_config: RpcSendTransactionConfig {
                skip_preflight,
                preflight_commitment: Some(commitment.commitment),
                ..RpcSendTransactionConfig::default()
            },
        })
    }
}
