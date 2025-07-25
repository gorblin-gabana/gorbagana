use {
    crate::solana::wen_restart_proto::HeaviestForkRecord,
    anyhow::Result,
    log::*,
    solana_clock::Slot,
    solana_gossip::restart_crds_values::RestartHeaviestFork,
    solana_hash::Hash,
    solana_pubkey::Pubkey,
    solana_runtime::epoch_stakes::VersionedEpochStakes,
    std::{
        collections::{HashMap, HashSet},
        str::FromStr,
    },
};

pub(crate) struct HeaviestForkAggregate {
    my_shred_version: u16,
    my_pubkey: Pubkey,
    // We use the epoch_stakes of the Epoch our heaviest bank is in. Proceed and exit only if
    // enough validator agree with me.
    epoch_stakes: VersionedEpochStakes,
    heaviest_forks: HashMap<Pubkey, RestartHeaviestFork>,
    block_stake_map: HashMap<(Slot, Hash), u64>,
    active_peers: HashSet<Pubkey>,
}

#[derive(Debug, PartialEq)]
pub enum HeaviestForkAggregateResult {
    AlreadyExists,
    DifferentVersionExists(RestartHeaviestFork, RestartHeaviestFork),
    Inserted(HeaviestForkRecord),
    Malformed,
    ZeroStakeIgnored,
}

impl HeaviestForkAggregate {
    pub(crate) fn new(
        my_shred_version: u16,
        epoch_stakes: &VersionedEpochStakes,
        my_heaviest_fork_slot: Slot,
        my_heaviest_fork_hash: Hash,
        my_pubkey: &Pubkey,
    ) -> Self {
        let mut active_peers = HashSet::new();
        active_peers.insert(*my_pubkey);
        let mut block_stake_map = HashMap::new();
        block_stake_map.insert(
            (my_heaviest_fork_slot, my_heaviest_fork_hash),
            epoch_stakes.node_id_to_stake(my_pubkey).unwrap_or(0),
        );
        Self {
            my_shred_version,
            my_pubkey: *my_pubkey,
            epoch_stakes: epoch_stakes.clone(),
            heaviest_forks: HashMap::new(),
            block_stake_map,
            active_peers,
        }
    }

    pub(crate) fn aggregate_from_record(
        &mut self,
        record: &HeaviestForkRecord,
    ) -> Result<HeaviestForkAggregateResult> {
        let from = Pubkey::from_str(&record.from)?;
        let bankhash = Hash::from_str(&record.bankhash)?;
        let restart_heaviest_fork = RestartHeaviestFork {
            from,
            wallclock: record.wallclock,
            last_slot: record.slot,
            last_slot_hash: bankhash,
            observed_stake: record.total_active_stake,
            shred_version: record.shred_version as u16,
        };
        Ok(self.aggregate(restart_heaviest_fork))
    }

    fn is_valid_change(
        current_heaviest_fork: &RestartHeaviestFork,
        new_heaviest_fork: &RestartHeaviestFork,
    ) -> HeaviestForkAggregateResult {
        if current_heaviest_fork.last_slot != new_heaviest_fork.last_slot
            || current_heaviest_fork.last_slot_hash != new_heaviest_fork.last_slot_hash
        {
            return HeaviestForkAggregateResult::DifferentVersionExists(
                current_heaviest_fork.clone(),
                new_heaviest_fork.clone(),
            );
        }
        if current_heaviest_fork == new_heaviest_fork
            || current_heaviest_fork.wallclock > new_heaviest_fork.wallclock
        {
            return HeaviestForkAggregateResult::AlreadyExists;
        }
        HeaviestForkAggregateResult::Inserted(HeaviestForkRecord {
            slot: new_heaviest_fork.last_slot,
            bankhash: new_heaviest_fork.last_slot_hash.to_string(),
            total_active_stake: new_heaviest_fork.observed_stake,
            shred_version: new_heaviest_fork.shred_version as u32,
            wallclock: new_heaviest_fork.wallclock,
            from: new_heaviest_fork.from.to_string(),
        })
    }

    pub(crate) fn aggregate(
        &mut self,
        received_heaviest_fork: RestartHeaviestFork,
    ) -> HeaviestForkAggregateResult {
        let from = &received_heaviest_fork.from;
        let sender_stake = self.epoch_stakes.node_id_to_stake(from).unwrap_or(0);
        if sender_stake == 0 {
            warn!("Gossip should not accept zero-stake RestartLastVotedFork from {from:?}");
            return HeaviestForkAggregateResult::ZeroStakeIgnored;
        }
        if from == &self.my_pubkey {
            return HeaviestForkAggregateResult::AlreadyExists;
        }
        if received_heaviest_fork.shred_version != self.my_shred_version {
            warn!(
                "Gossip should not accept RestartLastVotedFork with different shred version {} \
                 from {from:?}",
                received_heaviest_fork.shred_version
            );
            return HeaviestForkAggregateResult::Malformed;
        }
        let result = if let Some(old_heaviest_fork) = self.heaviest_forks.get(from) {
            let result = Self::is_valid_change(old_heaviest_fork, &received_heaviest_fork);
            if let HeaviestForkAggregateResult::Inserted(_) = result {
                // continue following processing
            } else {
                return result;
            }
            result
        } else {
            let entry = self
                .block_stake_map
                .entry((
                    received_heaviest_fork.last_slot,
                    received_heaviest_fork.last_slot_hash,
                ))
                .or_insert(0);
            *entry = entry.saturating_add(sender_stake);
            self.active_peers.insert(*from);
            HeaviestForkAggregateResult::Inserted(HeaviestForkRecord {
                slot: received_heaviest_fork.last_slot,
                bankhash: received_heaviest_fork.last_slot_hash.to_string(),
                total_active_stake: received_heaviest_fork.observed_stake,
                shred_version: received_heaviest_fork.shred_version as u32,
                wallclock: received_heaviest_fork.wallclock,
                from: from.to_string(),
            })
        };
        self.heaviest_forks
            .insert(*from, received_heaviest_fork.clone());
        result
    }

    pub(crate) fn total_active_stake(&self) -> u64 {
        self.active_peers.iter().fold(0, |sum: u64, pubkey| {
            sum.saturating_add(self.epoch_stakes.node_id_to_stake(pubkey).unwrap_or(0))
        })
    }

    pub(crate) fn print_block_stake_map(&self) {
        let total_stake = self.epoch_stakes.total_stake();
        for ((slot, hash), stake) in self.block_stake_map.iter() {
            info!(
                "Heaviest Fork Aggregated Slot: {}, Hash: {}, Stake: {}, Percent: {:.2}%",
                slot,
                hash,
                stake,
                *stake as f64 / total_stake as f64 * 100.0,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        crate::{
            heaviest_fork_aggregate::{HeaviestForkAggregate, HeaviestForkAggregateResult},
            solana::wen_restart_proto::HeaviestForkRecord,
        },
        solana_clock::Slot,
        solana_gossip::restart_crds_values::RestartHeaviestFork,
        solana_hash::Hash,
        solana_pubkey::Pubkey,
        solana_runtime::{
            bank::Bank,
            genesis_utils::{
                create_genesis_config_with_vote_accounts, GenesisConfigInfo, ValidatorVoteKeypairs,
            },
        },
        solana_signer::Signer,
        solana_time_utils::timestamp,
    };

    const TOTAL_VALIDATOR_COUNT: u16 = 20;
    const MY_INDEX: usize = 19;
    const SHRED_VERSION: u16 = 52;

    struct TestAggregateInitResult {
        pub heaviest_fork_aggregate: HeaviestForkAggregate,
        pub validator_voting_keypairs: Vec<ValidatorVoteKeypairs>,
        pub heaviest_slot: Slot,
        pub heaviest_hash: Hash,
    }

    fn test_aggregate_init() -> TestAggregateInitResult {
        solana_logger::setup();
        let validator_voting_keypairs: Vec<_> = (0..TOTAL_VALIDATOR_COUNT)
            .map(|_| ValidatorVoteKeypairs::new_rand())
            .collect();
        let GenesisConfigInfo { genesis_config, .. } = create_genesis_config_with_vote_accounts(
            10_000,
            &validator_voting_keypairs,
            vec![100; validator_voting_keypairs.len()],
        );
        let (_, bank_forks) = Bank::new_with_bank_forks_for_tests(&genesis_config);
        let root_bank = bank_forks.read().unwrap().root_bank();
        let heaviest_slot = root_bank.slot().saturating_add(3);
        let heaviest_hash = Hash::new_unique();
        TestAggregateInitResult {
            heaviest_fork_aggregate: HeaviestForkAggregate::new(
                SHRED_VERSION,
                root_bank.epoch_stakes(root_bank.epoch()).unwrap(),
                heaviest_slot,
                heaviest_hash,
                &validator_voting_keypairs[MY_INDEX].node_keypair.pubkey(),
            ),
            validator_voting_keypairs,
            heaviest_slot,
            heaviest_hash,
        }
    }

    #[test]
    fn test_aggregate_from_gossip() {
        let mut test_state = test_aggregate_init();
        let initial_num_active_validators = 3;
        let timestamp1 = timestamp();
        for validator_voting_keypair in test_state
            .validator_voting_keypairs
            .iter()
            .take(initial_num_active_validators)
        {
            let pubkey = validator_voting_keypair.node_keypair.pubkey();
            assert_eq!(
                test_state
                    .heaviest_fork_aggregate
                    .aggregate(RestartHeaviestFork {
                        from: pubkey,
                        wallclock: timestamp1,
                        last_slot: test_state.heaviest_slot,
                        last_slot_hash: test_state.heaviest_hash,
                        observed_stake: 100,
                        shred_version: SHRED_VERSION,
                    },),
                HeaviestForkAggregateResult::Inserted(HeaviestForkRecord {
                    slot: test_state.heaviest_slot,
                    bankhash: test_state.heaviest_hash.to_string(),
                    total_active_stake: 100,
                    shred_version: SHRED_VERSION as u32,
                    wallclock: timestamp1,
                    from: pubkey.to_string(),
                }),
            );
        }
        assert_eq!(
            test_state.heaviest_fork_aggregate.total_active_stake(),
            (initial_num_active_validators + 1) as u64 * 100
        );

        let new_active_validator = test_state.validator_voting_keypairs
            [initial_num_active_validators + 1]
            .node_keypair
            .pubkey();
        let now = timestamp();
        let new_active_validator_last_voted_slots = RestartHeaviestFork {
            from: new_active_validator,
            wallclock: now,
            last_slot: test_state.heaviest_slot,
            last_slot_hash: test_state.heaviest_hash,
            observed_stake: 100,
            shred_version: SHRED_VERSION,
        };
        assert_eq!(
            test_state
                .heaviest_fork_aggregate
                .aggregate(new_active_validator_last_voted_slots),
            HeaviestForkAggregateResult::Inserted(HeaviestForkRecord {
                slot: test_state.heaviest_slot,
                bankhash: test_state.heaviest_hash.to_string(),
                total_active_stake: 100,
                shred_version: SHRED_VERSION as u32,
                wallclock: now,
                from: new_active_validator.to_string(),
            }),
        );
        let expected_total_active_stake = (initial_num_active_validators + 2) as u64 * 100;
        assert_eq!(
            test_state.heaviest_fork_aggregate.total_active_stake(),
            expected_total_active_stake
        );
        let replace_message_validator = test_state.validator_voting_keypairs[2]
            .node_keypair
            .pubkey();
        // If hash changes, it will be ignored.
        let now = timestamp();
        let new_hash = Hash::new_unique();
        let replace_message_validator_last_fork = RestartHeaviestFork {
            from: replace_message_validator,
            wallclock: now,
            last_slot: test_state.heaviest_slot + 1,
            last_slot_hash: new_hash,
            observed_stake: 100,
            shred_version: SHRED_VERSION,
        };
        assert_eq!(
            test_state
                .heaviest_fork_aggregate
                .aggregate(replace_message_validator_last_fork.clone()),
            HeaviestForkAggregateResult::DifferentVersionExists(
                RestartHeaviestFork {
                    from: replace_message_validator,
                    wallclock: timestamp1,
                    last_slot: test_state.heaviest_slot,
                    last_slot_hash: test_state.heaviest_hash,
                    observed_stake: 100,
                    shred_version: SHRED_VERSION,
                },
                replace_message_validator_last_fork,
            ),
        );
        assert_eq!(
            test_state.heaviest_fork_aggregate.total_active_stake(),
            expected_total_active_stake
        );

        // test that zero stake validator is ignored.
        let random_pubkey = Pubkey::new_unique();
        assert_eq!(
            test_state
                .heaviest_fork_aggregate
                .aggregate(RestartHeaviestFork {
                    from: random_pubkey,
                    wallclock: timestamp(),
                    last_slot: test_state.heaviest_slot,
                    last_slot_hash: test_state.heaviest_hash,
                    observed_stake: 100,
                    shred_version: SHRED_VERSION,
                },),
            HeaviestForkAggregateResult::ZeroStakeIgnored,
        );
        assert_eq!(
            test_state.heaviest_fork_aggregate.total_active_stake(),
            expected_total_active_stake
        );

        // If everyone is seeing only 70%, the total active stake seeing supermajority is 0.
        for validator_voting_keypair in test_state.validator_voting_keypairs.iter().take(13) {
            let pubkey = validator_voting_keypair.node_keypair.pubkey();
            let now = timestamp();
            assert_eq!(
                test_state
                    .heaviest_fork_aggregate
                    .aggregate(RestartHeaviestFork {
                        from: pubkey,
                        wallclock: now,
                        last_slot: test_state.heaviest_slot,
                        last_slot_hash: test_state.heaviest_hash,
                        observed_stake: 1400,
                        shred_version: SHRED_VERSION,
                    },),
                HeaviestForkAggregateResult::Inserted(HeaviestForkRecord {
                    slot: test_state.heaviest_slot,
                    bankhash: test_state.heaviest_hash.to_string(),
                    total_active_stake: 1400,
                    shred_version: SHRED_VERSION as u32,
                    wallclock: now,
                    from: pubkey.to_string(),
                }),
            );
        }
        assert_eq!(
            test_state.heaviest_fork_aggregate.total_active_stake(),
            1400
        );

        // test that when 75% of the stake is seeing supermajority,
        // the active percent seeing supermajority is 75%.
        for validator_voting_keypair in test_state.validator_voting_keypairs.iter().take(14) {
            let pubkey = validator_voting_keypair.node_keypair.pubkey();
            let now = timestamp();
            assert_eq!(
                test_state
                    .heaviest_fork_aggregate
                    .aggregate(RestartHeaviestFork {
                        from: pubkey,
                        wallclock: now,
                        last_slot: test_state.heaviest_slot,
                        last_slot_hash: test_state.heaviest_hash,
                        observed_stake: 1500,
                        shred_version: SHRED_VERSION,
                    },),
                HeaviestForkAggregateResult::Inserted(HeaviestForkRecord {
                    slot: test_state.heaviest_slot,
                    bankhash: test_state.heaviest_hash.to_string(),
                    total_active_stake: 1500,
                    shred_version: SHRED_VERSION as u32,
                    wallclock: now,
                    from: pubkey.to_string(),
                }),
            );
        }

        assert_eq!(
            test_state.heaviest_fork_aggregate.total_active_stake(),
            1500
        );

        // test that message from my pubkey is ignored.
        assert_eq!(
            test_state
                .heaviest_fork_aggregate
                .aggregate(RestartHeaviestFork {
                    from: test_state.validator_voting_keypairs[MY_INDEX]
                        .node_keypair
                        .pubkey(),
                    wallclock: timestamp(),
                    last_slot: test_state.heaviest_slot,
                    last_slot_hash: test_state.heaviest_hash,
                    observed_stake: 100,
                    shred_version: SHRED_VERSION,
                },),
            HeaviestForkAggregateResult::AlreadyExists,
        );
    }

    #[test]
    fn test_aggregate_from_record() {
        let mut test_state = test_aggregate_init();
        let time1 = timestamp();
        let from = test_state.validator_voting_keypairs[0]
            .node_keypair
            .pubkey();
        let record = HeaviestForkRecord {
            wallclock: time1,
            slot: test_state.heaviest_slot,
            bankhash: test_state.heaviest_hash.to_string(),
            shred_version: SHRED_VERSION as u32,
            total_active_stake: 100,
            from: from.to_string(),
        };
        assert_eq!(test_state.heaviest_fork_aggregate.total_active_stake(), 100);
        assert_eq!(
            test_state
                .heaviest_fork_aggregate
                .aggregate_from_record(&record)
                .unwrap(),
            HeaviestForkAggregateResult::Inserted(record.clone()),
        );
        assert_eq!(test_state.heaviest_fork_aggregate.total_active_stake(), 200);
        // Now if you get the same result from Gossip again, it should be ignored.
        assert_eq!(
            test_state
                .heaviest_fork_aggregate
                .aggregate(RestartHeaviestFork {
                    from,
                    wallclock: time1,
                    last_slot: test_state.heaviest_slot,
                    last_slot_hash: test_state.heaviest_hash,
                    observed_stake: 100,
                    shred_version: SHRED_VERSION,
                },),
            HeaviestForkAggregateResult::AlreadyExists,
        );

        // If only observed_stake changes, it will be replaced.
        let time2 = timestamp();
        let old_heaviest_fork = RestartHeaviestFork {
            from,
            wallclock: time2,
            last_slot: test_state.heaviest_slot,
            last_slot_hash: test_state.heaviest_hash,
            observed_stake: 200,
            shred_version: SHRED_VERSION,
        };
        assert_eq!(
            test_state
                .heaviest_fork_aggregate
                .aggregate(old_heaviest_fork.clone()),
            HeaviestForkAggregateResult::Inserted(HeaviestForkRecord {
                wallclock: time2,
                slot: test_state.heaviest_slot,
                bankhash: test_state.heaviest_hash.to_string(),
                shred_version: SHRED_VERSION as u32,
                total_active_stake: 200,
                from: from.to_string(),
            }),
        );

        // If slot changes, it will be ignored.
        let new_heaviest_fork = RestartHeaviestFork {
            from: test_state.validator_voting_keypairs[0]
                .node_keypair
                .pubkey(),
            wallclock: timestamp(),
            last_slot: test_state.heaviest_slot + 1,
            last_slot_hash: test_state.heaviest_hash,
            observed_stake: 100,
            shred_version: SHRED_VERSION,
        };
        assert_eq!(
            test_state
                .heaviest_fork_aggregate
                .aggregate(new_heaviest_fork.clone()),
            HeaviestForkAggregateResult::DifferentVersionExists(
                old_heaviest_fork.clone(),
                new_heaviest_fork
            )
        );
        // If hash changes, it will also be ignored.
        let new_heaviest_fork = RestartHeaviestFork {
            from: test_state.validator_voting_keypairs[0]
                .node_keypair
                .pubkey(),
            wallclock: timestamp(),
            last_slot: test_state.heaviest_slot,
            last_slot_hash: Hash::new_unique(),
            observed_stake: 100,
            shred_version: SHRED_VERSION,
        };
        assert_eq!(
            test_state
                .heaviest_fork_aggregate
                .aggregate(new_heaviest_fork.clone()),
            HeaviestForkAggregateResult::DifferentVersionExists(
                old_heaviest_fork,
                new_heaviest_fork
            )
        );

        // percentage doesn't change since it's a replace.
        assert_eq!(test_state.heaviest_fork_aggregate.total_active_stake(), 200);

        // Record from validator with zero stake should be ignored.
        let zero_stake_validator = Pubkey::new_unique();
        assert_eq!(
            test_state
                .heaviest_fork_aggregate
                .aggregate_from_record(&HeaviestForkRecord {
                    wallclock: timestamp(),
                    slot: test_state.heaviest_slot,
                    bankhash: test_state.heaviest_hash.to_string(),
                    shred_version: SHRED_VERSION as u32,
                    total_active_stake: 100,
                    from: zero_stake_validator.to_string(),
                })
                .unwrap(),
            HeaviestForkAggregateResult::ZeroStakeIgnored,
        );
        // percentage doesn't change since the previous aggregate is ignored.
        assert_eq!(test_state.heaviest_fork_aggregate.total_active_stake(), 200);

        // Record from my pubkey should be ignored.
        let my_pubkey = test_state.validator_voting_keypairs[MY_INDEX]
            .node_keypair
            .pubkey();
        assert_eq!(
            test_state
                .heaviest_fork_aggregate
                .aggregate_from_record(&HeaviestForkRecord {
                    wallclock: timestamp(),
                    slot: test_state.heaviest_slot,
                    bankhash: test_state.heaviest_hash.to_string(),
                    shred_version: SHRED_VERSION as u32,
                    total_active_stake: 100,
                    from: my_pubkey.to_string(),
                })
                .unwrap(),
            HeaviestForkAggregateResult::AlreadyExists,
        );
    }

    #[test]
    fn test_aggregate_from_record_failures() {
        let mut test_state = test_aggregate_init();
        let from = test_state.validator_voting_keypairs[0]
            .node_keypair
            .pubkey();
        let mut heaviest_fork_record = HeaviestForkRecord {
            wallclock: timestamp(),
            slot: test_state.heaviest_slot,
            bankhash: test_state.heaviest_hash.to_string(),
            shred_version: SHRED_VERSION as u32,
            total_active_stake: 100,
            from: from.to_string(),
        };
        // First test that this is a valid record.
        assert_eq!(
            test_state
                .heaviest_fork_aggregate
                .aggregate_from_record(&heaviest_fork_record,)
                .unwrap(),
            HeaviestForkAggregateResult::Inserted(heaviest_fork_record.clone()),
        );
        // Then test that it fails if the record is invalid.

        // Invalid pubkey.
        heaviest_fork_record.from = "invalid_pubkey".to_string();
        assert!(test_state
            .heaviest_fork_aggregate
            .aggregate_from_record(&heaviest_fork_record,)
            .is_err());

        // Invalid hash.
        heaviest_fork_record.from = from.to_string();
        heaviest_fork_record.bankhash.clear();
        assert!(test_state
            .heaviest_fork_aggregate
            .aggregate_from_record(&heaviest_fork_record,)
            .is_err());
    }
}
