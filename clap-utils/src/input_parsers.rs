use {
    crate::keypair::{
        keypair_from_seed_phrase, pubkey_from_path, resolve_signer_from_path, signer_from_path,
        ASK_KEYWORD, SKIP_SEED_PHRASE_VALIDATION_ARG,
    },
    chrono::DateTime,
    clap::ArgMatches,
    solana_clock::UnixTimestamp,
    solana_cluster_type::ClusterType,
    solana_commitment_config::CommitmentConfig,
    solana_keypair::{read_keypair_file, Keypair},
    solana_native_token::LAMPORTS_PER_SOL,
    solana_pubkey::Pubkey,
    solana_remote_wallet::remote_wallet::RemoteWalletManager,
    solana_signature::Signature,
    solana_signer::Signer,
    std::{io, num::ParseIntError, rc::Rc, str::FromStr},
};

// Sentinel value used to indicate to write to screen instead of file
pub const STDOUT_OUTFILE_TOKEN: &str = "-";

// Return parsed values from matches at `name`
pub fn values_of<T>(matches: &ArgMatches, name: &str) -> Option<Vec<T>>
where
    T: std::str::FromStr,
    <T as std::str::FromStr>::Err: std::fmt::Debug,
{
    matches
        .values_of(name)
        .map(|xs| xs.map(|x| x.parse::<T>().unwrap()).collect())
}

// Return a parsed value from matches at `name`
pub fn value_of<T>(matches: &ArgMatches, name: &str) -> Option<T>
where
    T: std::str::FromStr,
    <T as std::str::FromStr>::Err: std::fmt::Debug,
{
    if let Some(value) = matches.value_of(name) {
        value.parse::<T>().ok()
    } else {
        None
    }
}

pub fn unix_timestamp_from_rfc3339_datetime(
    matches: &ArgMatches,
    name: &str,
) -> Option<UnixTimestamp> {
    matches.value_of(name).and_then(|value| {
        DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|date_time| date_time.timestamp())
    })
}

// Return the keypair for an argument with filename `name` or None if not present.
pub fn keypair_of(matches: &ArgMatches, name: &str) -> Option<Keypair> {
    if let Some(value) = matches.value_of(name) {
        if value == ASK_KEYWORD {
            let skip_validation = matches.is_present(SKIP_SEED_PHRASE_VALIDATION_ARG.name);
            keypair_from_seed_phrase(name, skip_validation, true, None, true).ok()
        } else {
            read_keypair_file(value).ok()
        }
    } else {
        None
    }
}

pub fn keypairs_of(matches: &ArgMatches, name: &str) -> Option<Vec<Keypair>> {
    matches.values_of(name).map(|values| {
        values
            .filter_map(|value| {
                if value == ASK_KEYWORD {
                    let skip_validation = matches.is_present(SKIP_SEED_PHRASE_VALIDATION_ARG.name);
                    keypair_from_seed_phrase(name, skip_validation, true, None, true).ok()
                } else {
                    read_keypair_file(value).ok()
                }
            })
            .collect()
    })
}

// Return a pubkey for an argument that can itself be parsed into a pubkey,
// or is a filename that can be read as a keypair
pub fn pubkey_of(matches: &ArgMatches, name: &str) -> Option<Pubkey> {
    value_of(matches, name).or_else(|| keypair_of(matches, name).map(|keypair| keypair.pubkey()))
}

pub fn pubkeys_of(matches: &ArgMatches, name: &str) -> Option<Vec<Pubkey>> {
    matches.values_of(name).map(|values| {
        values
            .map(|value| {
                value.parse::<Pubkey>().unwrap_or_else(|_| {
                    read_keypair_file(value)
                        .expect("read_keypair_file failed")
                        .pubkey()
                })
            })
            .collect()
    })
}

// Return pubkey/signature pairs for a string of the form pubkey=signature
pub fn pubkeys_sigs_of(matches: &ArgMatches, name: &str) -> Option<Vec<(Pubkey, Signature)>> {
    matches.values_of(name).map(|values| {
        values
            .map(|pubkey_signer_string| {
                let mut signer = pubkey_signer_string.split('=');
                let key = Pubkey::from_str(signer.next().unwrap()).unwrap();
                let sig = Signature::from_str(signer.next().unwrap()).unwrap();
                (key, sig)
            })
            .collect()
    })
}

// Return a signer from matches at `name`
#[allow(clippy::type_complexity)]
pub fn signer_of(
    matches: &ArgMatches,
    name: &str,
    wallet_manager: &mut Option<Rc<RemoteWalletManager>>,
) -> Result<(Option<Box<dyn Signer>>, Option<Pubkey>), Box<dyn std::error::Error>> {
    if let Some(location) = matches.value_of(name) {
        let signer = signer_from_path(matches, location, name, wallet_manager)?;
        let signer_pubkey = signer.pubkey();
        Ok((Some(signer), Some(signer_pubkey)))
    } else {
        Ok((None, None))
    }
}

pub fn pubkey_of_signer(
    matches: &ArgMatches,
    name: &str,
    wallet_manager: &mut Option<Rc<RemoteWalletManager>>,
) -> Result<Option<Pubkey>, Box<dyn std::error::Error>> {
    if let Some(location) = matches.value_of(name) {
        Ok(Some(pubkey_from_path(
            matches,
            location,
            name,
            wallet_manager,
        )?))
    } else {
        Ok(None)
    }
}

pub fn pubkeys_of_multiple_signers(
    matches: &ArgMatches,
    name: &str,
    wallet_manager: &mut Option<Rc<RemoteWalletManager>>,
) -> Result<Option<Vec<Pubkey>>, Box<dyn std::error::Error>> {
    if let Some(pubkey_matches) = matches.values_of(name) {
        let mut pubkeys: Vec<Pubkey> = vec![];
        for signer in pubkey_matches {
            pubkeys.push(pubkey_from_path(matches, signer, name, wallet_manager)?);
        }
        Ok(Some(pubkeys))
    } else {
        Ok(None)
    }
}

pub fn resolve_signer(
    matches: &ArgMatches,
    name: &str,
    wallet_manager: &mut Option<Rc<RemoteWalletManager>>,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    resolve_signer_from_path(
        matches,
        matches.value_of(name).unwrap(),
        name,
        wallet_manager,
    )
}

/// Convert a SOL amount string to lamports.
///
/// Accepts plain or decimal strings ("50", "0.03", ".5", "1.").
/// Any decimal places beyond 9 are truncated.
pub fn lamports_of_sol(matches: &ArgMatches, name: &str) -> Option<u64> {
    matches.value_of(name).and_then(|value| {
        if value == "." {
            None
        } else {
            let (sol, lamports) = value.split_once('.').unwrap_or((value, ""));
            let sol = if sol.is_empty() {
                0
            } else {
                sol.parse::<u64>().ok()?
            };
            let lamports = if lamports.is_empty() {
                0
            } else {
                format!("{:0<9}", lamports)[..9].parse().ok()?
            };
            Some(
                LAMPORTS_PER_SOL
                    .saturating_mul(sol)
                    .saturating_add(lamports),
            )
        }
    })
}

pub fn cluster_type_of(matches: &ArgMatches, name: &str) -> Option<ClusterType> {
    value_of(matches, name)
}

pub fn commitment_of(matches: &ArgMatches, name: &str) -> Option<CommitmentConfig> {
    matches
        .value_of(name)
        .map(|value| CommitmentConfig::from_str(value).unwrap_or_default())
}

// Parse a cpu range in standard cpuset format, eg:
//
// 0-4,9
// 0-2,7,12-14
pub fn parse_cpu_ranges(data: &str) -> Result<Vec<usize>, io::Error> {
    data.split(',')
        .map(|range| {
            let mut iter = range
                .split('-')
                .map(|s| s.parse::<usize>().map_err(|ParseIntError { .. }| range));
            let start = iter.next().unwrap()?; // str::split always returns at least one element.
            let end = match iter.next() {
                None => start,
                Some(end) => {
                    if iter.next().is_some() {
                        return Err(range);
                    }
                    end?
                }
            };
            Ok(start..=end)
        })
        .try_fold(Vec::new(), |mut cpus, range| {
            let range = range.map_err(|range| io::Error::new(io::ErrorKind::InvalidData, range))?;
            cpus.extend(range);
            Ok(cpus)
        })
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        clap::{App, Arg},
        solana_keypair::write_keypair_file,
        std::fs,
    };

    fn app<'ab, 'v>() -> App<'ab, 'v> {
        App::new("test")
            .arg(
                Arg::with_name("multiple")
                    .long("multiple")
                    .takes_value(true)
                    .multiple(true),
            )
            .arg(Arg::with_name("single").takes_value(true).long("single"))
            .arg(Arg::with_name("unit").takes_value(true).long("unit"))
    }

    fn tmp_file_path(name: &str, pubkey: &Pubkey) -> String {
        use std::env;
        let out_dir = env::var("FARF_DIR").unwrap_or_else(|_| "farf".to_string());

        format!("{out_dir}/tmp/{name}-{pubkey}")
    }

    #[test]
    fn test_values_of() {
        let matches = app().get_matches_from(vec!["test", "--multiple", "50", "--multiple", "39"]);
        assert_eq!(values_of(&matches, "multiple"), Some(vec![50, 39]));
        assert_eq!(values_of::<u64>(&matches, "single"), None);

        let pubkey0 = solana_pubkey::new_rand();
        let pubkey1 = solana_pubkey::new_rand();
        let matches = app().get_matches_from(vec![
            "test",
            "--multiple",
            &pubkey0.to_string(),
            "--multiple",
            &pubkey1.to_string(),
        ]);
        assert_eq!(
            values_of(&matches, "multiple"),
            Some(vec![pubkey0, pubkey1])
        );
    }

    #[test]
    fn test_value_of() {
        let matches = app().get_matches_from(vec!["test", "--single", "50"]);
        assert_eq!(value_of(&matches, "single"), Some(50));
        assert_eq!(value_of::<u64>(&matches, "multiple"), None);

        let pubkey = solana_pubkey::new_rand();
        let matches = app().get_matches_from(vec!["test", "--single", &pubkey.to_string()]);
        assert_eq!(value_of(&matches, "single"), Some(pubkey));
    }

    #[test]
    fn test_keypair_of() {
        let keypair = Keypair::new();
        let outfile = tmp_file_path("test_keypair_of.json", &keypair.pubkey());
        let _ = write_keypair_file(&keypair, &outfile).unwrap();

        let matches = app().get_matches_from(vec!["test", "--single", &outfile]);
        assert_eq!(
            keypair_of(&matches, "single").unwrap().pubkey(),
            keypair.pubkey()
        );
        assert!(keypair_of(&matches, "multiple").is_none());

        let matches = app().get_matches_from(vec!["test", "--single", "random_keypair_file.json"]);
        assert!(keypair_of(&matches, "single").is_none());

        fs::remove_file(&outfile).unwrap();
    }

    #[test]
    fn test_pubkey_of() {
        let keypair = Keypair::new();
        let outfile = tmp_file_path("test_pubkey_of.json", &keypair.pubkey());
        let _ = write_keypair_file(&keypair, &outfile).unwrap();

        let matches = app().get_matches_from(vec!["test", "--single", &outfile]);
        assert_eq!(pubkey_of(&matches, "single"), Some(keypair.pubkey()));
        assert_eq!(pubkey_of(&matches, "multiple"), None);

        let matches =
            app().get_matches_from(vec!["test", "--single", &keypair.pubkey().to_string()]);
        assert_eq!(pubkey_of(&matches, "single"), Some(keypair.pubkey()));

        let matches = app().get_matches_from(vec!["test", "--single", "random_keypair_file.json"]);
        assert_eq!(pubkey_of(&matches, "single"), None);

        fs::remove_file(&outfile).unwrap();
    }

    #[test]
    fn test_pubkeys_of() {
        let keypair = Keypair::new();
        let outfile = tmp_file_path("test_pubkeys_of.json", &keypair.pubkey());
        let _ = write_keypair_file(&keypair, &outfile).unwrap();

        let matches = app().get_matches_from(vec![
            "test",
            "--multiple",
            &keypair.pubkey().to_string(),
            "--multiple",
            &outfile,
        ]);
        assert_eq!(
            pubkeys_of(&matches, "multiple"),
            Some(vec![keypair.pubkey(), keypair.pubkey()])
        );
        fs::remove_file(&outfile).unwrap();
    }

    #[test]
    fn test_pubkeys_sigs_of() {
        let key1 = solana_pubkey::new_rand();
        let key2 = solana_pubkey::new_rand();
        let sig1 = Keypair::new().sign_message(&[0u8]);
        let sig2 = Keypair::new().sign_message(&[1u8]);
        let signer1 = format!("{key1}={sig1}");
        let signer2 = format!("{key2}={sig2}");
        let matches =
            app().get_matches_from(vec!["test", "--multiple", &signer1, "--multiple", &signer2]);
        assert_eq!(
            pubkeys_sigs_of(&matches, "multiple"),
            Some(vec![(key1, sig1), (key2, sig2)])
        );
    }

    #[test]
    #[ignore = "historical reference; shows float behavior fixed in pull #4988"]
    fn test_lamports_of_sol_origin() {
        use solana_native_token::sol_to_lamports;
        pub fn lamports_of_sol(matches: &ArgMatches, name: &str) -> Option<u64> {
            value_of(matches, name).map(sol_to_lamports)
        }

        let matches = app().get_matches_from(vec!["test", "--single", "50"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(50_000_000_000));
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", "1.5"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(1_500_000_000));
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", "0.03"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(30_000_000));
        let matches = app().get_matches_from(vec!["test", "--single", ".03"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(30_000_000));
        let matches = app().get_matches_from(vec!["test", "--single", "1."]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(1_000_000_000));
        let matches = app().get_matches_from(vec!["test", "--single", ".0"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(0));
        let matches = app().get_matches_from(vec!["test", "--single", "."]);
        assert_eq!(lamports_of_sol(&matches, "single"), None);
        // NOT EQ
        let matches = app().get_matches_from(vec!["test", "--single", "1.000000015"]);
        assert_ne!(lamports_of_sol(&matches, "single"), Some(1_000_000_015));
        let matches = app().get_matches_from(vec!["test", "--single", "0.0157"]);
        assert_ne!(lamports_of_sol(&matches, "single"), Some(15_700_000));
        let matches = app().get_matches_from(vec!["test", "--single", "0.5025"]);
        assert_ne!(lamports_of_sol(&matches, "single"), Some(502_500_000));
    }

    #[test]
    fn test_lamports_of_sol() {
        let matches = app().get_matches_from(vec!["test", "--single", "50"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(50_000_000_000));
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", "1.5"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(1_500_000_000));
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", "0.03"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(30_000_000));
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", ".03"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(30_000_000));
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", "1."]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(1_000_000_000));
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", ".0"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(0));
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", "."]);
        assert_eq!(lamports_of_sol(&matches, "single"), None);
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        // EQ
        let matches = app().get_matches_from(vec!["test", "--single", "1.000000015"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(1_000_000_015));
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", "0.0157"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(15_700_000));
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", "0.5025"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(502_500_000));
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        // Truncation of extra decimal places
        let matches = app().get_matches_from(vec!["test", "--single", "0.1234567891"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(123_456_789));
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", "0.1234567899"]);
        assert_eq!(lamports_of_sol(&matches, "single"), Some(123_456_789));
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", "1.000.4567899"]);
        assert_eq!(lamports_of_sol(&matches, "single"), None);
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", "6,998"]);
        assert_eq!(lamports_of_sol(&matches, "single"), None);
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
        let matches = app().get_matches_from(vec!["test", "--single", "6,998.00"]);
        assert_eq!(lamports_of_sol(&matches, "single"), None);
        assert_eq!(lamports_of_sol(&matches, "multiple"), None);
    }
}
