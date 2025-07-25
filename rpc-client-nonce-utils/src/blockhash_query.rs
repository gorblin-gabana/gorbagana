#[cfg(feature = "clap")]
use {
    clap::ArgMatches,
    solana_clap_v3_utils::{
        input_parsers::signer::try_pubkey_of,
        nonce::*,
        offline::*,
    },
};
use {
    solana_commitment_config::CommitmentConfig, solana_hash::Hash, solana_pubkey::Pubkey,
    solana_rpc_client::rpc_client::RpcClient,
};

#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    Cluster,
    NonceAccount(Pubkey),
}

impl Source {
    pub fn get_blockhash(
        &self,
        rpc_client: &RpcClient,
        commitment: CommitmentConfig,
    ) -> Result<Hash, Box<dyn std::error::Error>> {
        match self {
            Self::Cluster => {
                let (blockhash, _) = rpc_client.get_latest_blockhash_with_commitment(commitment)?;
                Ok(blockhash)
            }
            Self::NonceAccount(ref pubkey) => {
                let data = crate::get_account_with_commitment(rpc_client, pubkey, commitment)
                    .and_then(|ref a| crate::data_from_account(a))?;
                Ok(data.blockhash())
            }
        }
    }

    pub fn is_blockhash_valid(
        &self,
        rpc_client: &RpcClient,
        blockhash: &Hash,
        commitment: CommitmentConfig,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(match self {
            Self::Cluster => rpc_client.is_blockhash_valid(blockhash, commitment)?,
            Self::NonceAccount(ref pubkey) => {
                let _ = crate::get_account_with_commitment(rpc_client, pubkey, commitment)
                    .and_then(|ref a| crate::data_from_account(a))?;
                true
            }
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BlockhashQuery {
    None(Hash),
    FeeCalculator(Source, Hash),
    All(Source),
}

impl BlockhashQuery {
    pub fn new(blockhash: Option<Hash>, sign_only: bool, nonce_account: Option<Pubkey>) -> Self {
        let source = nonce_account
            .map(Source::NonceAccount)
            .unwrap_or(Source::Cluster);
        match blockhash {
            Some(hash) if sign_only => Self::None(hash),
            Some(hash) if !sign_only => Self::FeeCalculator(source, hash),
            None if !sign_only => Self::All(source),
            _ => panic!("Cannot resolve blockhash"),
        }
    }

    #[cfg(feature = "clap")]
    pub fn new_from_matches(matches: &ArgMatches) -> Self {
        let blockhash = matches.get_one::<String>(BLOCKHASH_ARG.name)
            .map(|s| s.parse().expect("Invalid blockhash"));
        let sign_only = matches.get_flag(SIGN_ONLY_ARG.name);
        let nonce_account = try_pubkey_of(matches, NONCE_ARG.name).unwrap_or(None);
        BlockhashQuery::new(blockhash, sign_only, nonce_account)
    }

    pub fn get_blockhash(
        &self,
        rpc_client: &RpcClient,
        commitment: CommitmentConfig,
    ) -> Result<Hash, Box<dyn std::error::Error>> {
        match self {
            BlockhashQuery::None(hash) => Ok(*hash),
            BlockhashQuery::FeeCalculator(source, hash) => {
                if !source.is_blockhash_valid(rpc_client, hash, commitment)? {
                    return Err(format!("Hash has expired {hash:?}").into());
                }
                Ok(*hash)
            }
            BlockhashQuery::All(source) => source.get_blockhash(rpc_client, commitment),
        }
    }
}

impl Default for BlockhashQuery {
    fn default() -> Self {
        BlockhashQuery::All(Source::Cluster)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "clap")]
    use clap::App;
    use {
        super::*,
        crate::blockhash_query,
        serde_json::{self, json},
        solana_account::Account,
        solana_account_decoder::{encode_ui_account, UiAccountEncoding},
        solana_fee_calculator::FeeCalculator,
        solana_nonce::{self as nonce, state::DurableNonce},
        solana_rpc_client_api::{
            request::RpcRequest,
            response::{Response, RpcBlockhash, RpcResponseContext},
        },
        solana_sha256_hasher::hash,
        std::collections::HashMap,
    };

    #[test]
    fn test_blockhash_query_new_ok() {
        let blockhash = hash(&[1u8]);
        let nonce_pubkey = Pubkey::from([1u8; 32]);

        assert_eq!(
            BlockhashQuery::new(Some(blockhash), true, None),
            BlockhashQuery::None(blockhash),
        );
        assert_eq!(
            BlockhashQuery::new(Some(blockhash), false, None),
            BlockhashQuery::FeeCalculator(blockhash_query::Source::Cluster, blockhash),
        );
        assert_eq!(
            BlockhashQuery::new(None, false, None),
            BlockhashQuery::All(blockhash_query::Source::Cluster)
        );

        assert_eq!(
            BlockhashQuery::new(Some(blockhash), true, Some(nonce_pubkey)),
            BlockhashQuery::None(blockhash),
        );
        assert_eq!(
            BlockhashQuery::new(Some(blockhash), false, Some(nonce_pubkey)),
            BlockhashQuery::FeeCalculator(
                blockhash_query::Source::NonceAccount(nonce_pubkey),
                blockhash
            ),
        );
        assert_eq!(
            BlockhashQuery::new(None, false, Some(nonce_pubkey)),
            BlockhashQuery::All(blockhash_query::Source::NonceAccount(nonce_pubkey)),
        );
    }

    #[test]
    #[should_panic]
    fn test_blockhash_query_new_no_nonce_fail() {
        BlockhashQuery::new(None, true, None);
    }

    #[test]
    #[should_panic]
    fn test_blockhash_query_new_nonce_fail() {
        let nonce_pubkey = Pubkey::from([1u8; 32]);
        BlockhashQuery::new(None, true, Some(nonce_pubkey));
    }

    #[cfg(feature = "clap")]
    #[test]
    fn test_blockhash_query_new_from_matches_ok() {
        let test_commands = App::new("blockhash_query_test")
            .nonce_args(false)
            .offline_args();
        let blockhash = hash(&[1u8]);
        let blockhash_string = blockhash.to_string();

        let matches = test_commands.clone().get_matches_from(vec![
            "blockhash_query_test",
            "--blockhash",
            &blockhash_string,
            "--sign-only",
        ]);
        assert_eq!(
            BlockhashQuery::new_from_matches(&matches),
            BlockhashQuery::None(blockhash),
        );

        let matches = test_commands.clone().get_matches_from(vec![
            "blockhash_query_test",
            "--blockhash",
            &blockhash_string,
        ]);
        assert_eq!(
            BlockhashQuery::new_from_matches(&matches),
            BlockhashQuery::FeeCalculator(blockhash_query::Source::Cluster, blockhash),
        );

        let matches = test_commands
            .clone()
            .get_matches_from(vec!["blockhash_query_test"]);
        assert_eq!(
            BlockhashQuery::new_from_matches(&matches),
            BlockhashQuery::All(blockhash_query::Source::Cluster),
        );

        let nonce_pubkey = Pubkey::from([1u8; 32]);
        let nonce_string = nonce_pubkey.to_string();
        let matches = test_commands.clone().get_matches_from(vec![
            "blockhash_query_test",
            "--blockhash",
            &blockhash_string,
            "--sign-only",
            "--nonce",
            &nonce_string,
        ]);
        assert_eq!(
            BlockhashQuery::new_from_matches(&matches),
            BlockhashQuery::None(blockhash),
        );

        let matches = test_commands.clone().get_matches_from(vec![
            "blockhash_query_test",
            "--blockhash",
            &blockhash_string,
            "--nonce",
            &nonce_string,
        ]);
        assert_eq!(
            BlockhashQuery::new_from_matches(&matches),
            BlockhashQuery::FeeCalculator(
                blockhash_query::Source::NonceAccount(nonce_pubkey),
                blockhash
            ),
        );
    }

    #[cfg(feature = "clap")]
    #[test]
    #[should_panic]
    fn test_blockhash_query_new_from_matches_without_nonce_fail() {
        let test_commands = App::new("blockhash_query_test")
            .arg(blockhash_arg())
            // We can really only hit this case if the arg requirements
            // are broken, so unset the requires() to recreate that condition
            .arg(sign_only_arg().requires(""));

        let matches = test_commands.get_matches_from(vec!["blockhash_query_test", "--sign-only"]);
        BlockhashQuery::new_from_matches(&matches);
    }

    #[cfg(feature = "clap")]
    #[test]
    #[should_panic]
    fn test_blockhash_query_new_from_matches_with_nonce_fail() {
        let test_commands = App::new("blockhash_query_test")
            .arg(blockhash_arg())
            // We can really only hit this case if the arg requirements
            // are broken, so unset the requires() to recreate that condition
            .arg(sign_only_arg().requires(""));
        let nonce_pubkey = Pubkey::from([1u8; 32]);
        let nonce_string = nonce_pubkey.to_string();

        let matches = test_commands.get_matches_from(vec![
            "blockhash_query_test",
            "--sign-only",
            "--nonce",
            &nonce_string,
        ]);
        BlockhashQuery::new_from_matches(&matches);
    }

    #[test]
    #[allow(deprecated)]
    fn test_blockhash_query_get_blockhash() {
        let test_blockhash = hash(&[0u8]);
        let rpc_blockhash = hash(&[1u8]);
        let get_latest_blockhash_response = json!(Response {
            context: RpcResponseContext {
                slot: 1,
                api_version: None
            },
            value: json!(RpcBlockhash {
                blockhash: rpc_blockhash.to_string(),
                last_valid_block_height: 42,
            }),
        });
        let is_blockhash_valid_response = json!(Response {
            context: RpcResponseContext {
                slot: 1,
                api_version: None
            },
            value: true,
        });
        let mut mocks = HashMap::new();
        mocks.insert(
            RpcRequest::GetLatestBlockhash,
            get_latest_blockhash_response.clone(),
        );
        let rpc_client = RpcClient::new_mock_with_mocks("".to_string(), mocks);
        assert_eq!(
            BlockhashQuery::default()
                .get_blockhash(&rpc_client, CommitmentConfig::default())
                .unwrap(),
            rpc_blockhash,
        );
        let mut mocks = HashMap::new();
        mocks.insert(
            RpcRequest::IsBlockhashValid,
            is_blockhash_valid_response.clone(),
        );
        let rpc_client = RpcClient::new_mock_with_mocks("".to_string(), mocks);
        assert_eq!(
            BlockhashQuery::FeeCalculator(Source::Cluster, test_blockhash)
                .get_blockhash(&rpc_client, CommitmentConfig::default())
                .unwrap(),
            test_blockhash,
        );
        let mut mocks = HashMap::new();
        mocks.insert(
            RpcRequest::GetLatestBlockhash,
            get_latest_blockhash_response,
        );
        let rpc_client = RpcClient::new_mock_with_mocks("".to_string(), mocks);
        assert_eq!(
            BlockhashQuery::None(test_blockhash)
                .get_blockhash(&rpc_client, CommitmentConfig::default())
                .unwrap(),
            test_blockhash,
        );
        let rpc_client = RpcClient::new_mock("fails".to_string());
        assert!(BlockhashQuery::default()
            .get_blockhash(&rpc_client, CommitmentConfig::default())
            .is_err());

        let durable_nonce = DurableNonce::from_blockhash(&Hash::new_from_array([2u8; 32]));
        let nonce_blockhash = *durable_nonce.as_hash();
        let nonce_fee_calc = FeeCalculator::new(4242);
        let data = nonce::state::Data {
            authority: Pubkey::from([3u8; 32]),
            durable_nonce,
            fee_calculator: nonce_fee_calc,
        };
        let nonce_account = Account::new_data_with_space(
            42,
            &nonce::versions::Versions::new(nonce::state::State::Initialized(data)),
            nonce::state::State::size(),
            &solana_sdk_ids::system_program::id(),
        )
        .unwrap();
        let nonce_pubkey = Pubkey::from([4u8; 32]);
        let rpc_nonce_account = encode_ui_account(
            &nonce_pubkey,
            &nonce_account,
            UiAccountEncoding::Base64,
            None,
            None,
        );
        let get_account_response = json!(Response {
            context: RpcResponseContext {
                slot: 1,
                api_version: None
            },
            value: json!(Some(rpc_nonce_account)),
        });

        let mut mocks = HashMap::new();
        mocks.insert(RpcRequest::GetAccountInfo, get_account_response.clone());
        let rpc_client = RpcClient::new_mock_with_mocks("".to_string(), mocks);
        assert_eq!(
            BlockhashQuery::All(Source::NonceAccount(nonce_pubkey))
                .get_blockhash(&rpc_client, CommitmentConfig::default())
                .unwrap(),
            nonce_blockhash,
        );
        let mut mocks = HashMap::new();
        mocks.insert(RpcRequest::GetAccountInfo, get_account_response.clone());
        let rpc_client = RpcClient::new_mock_with_mocks("".to_string(), mocks);
        assert_eq!(
            BlockhashQuery::FeeCalculator(Source::NonceAccount(nonce_pubkey), nonce_blockhash)
                .get_blockhash(&rpc_client, CommitmentConfig::default())
                .unwrap(),
            nonce_blockhash,
        );
        let mut mocks = HashMap::new();
        mocks.insert(RpcRequest::GetAccountInfo, get_account_response);
        let rpc_client = RpcClient::new_mock_with_mocks("".to_string(), mocks);
        assert_eq!(
            BlockhashQuery::None(nonce_blockhash)
                .get_blockhash(&rpc_client, CommitmentConfig::default())
                .unwrap(),
            nonce_blockhash,
        );

        let rpc_client = RpcClient::new_mock("fails".to_string());
        assert!(BlockhashQuery::All(Source::NonceAccount(nonce_pubkey))
            .get_blockhash(&rpc_client, CommitmentConfig::default())
            .is_err());
    }
}
