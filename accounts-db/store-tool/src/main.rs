use {
    ahash::HashSet,
    clap::{
        Arg, ArgAction, ArgMatches, Command,
    },
    rayon::prelude::*,
    solana_account::ReadableAccount,
    solana_accounts_db::accounts_file::{AccountsFile, StorageAccess},
    solana_pubkey::Pubkey,
    solana_system_interface::MAX_PERMITTED_DATA_LENGTH,
    std::{
        fs, io,
        mem::ManuallyDrop,
        num::Saturating,
        path::{Path, PathBuf},
    },
};

const CMD_INSPECT: &str = "inspect";
const CMD_SEARCH: &str = "search";

fn main() {
    let matches = Command::new("agave-store-tool")
        .about("Tool for account storage files")
        .version("3.0.0")
        .subcommand_required(true)
        .subcommand(
            Command::new(CMD_INSPECT)
                .about("Inspects an account storage file and display each account's information")
                .arg(
                    Arg::new("path")
                        .index(1)
                        .required(true)
                        .value_name("PATH")
                        .help("Account storage file to inspect"),
                )
                .arg(
                    Arg::new("verbose")
                        .short('v')
                        .long("verbose")
                        .action(ArgAction::SetTrue)
                        .help("Show additional account information"),
                ),
        )
        .subcommand(
            Command::new(CMD_SEARCH)
                .about("Searches for accounts")
                .arg(
                    Arg::new("path")
                        .index(1)
                        .required(true)
                        .value_name("PATH")
                        .help("Account storage directory to search"),
                )
                .arg(
                    Arg::new("addresses")
                        .index(2)
                        .required(true)
                        .value_name("PUBKEYS")
                        .value_delimiter(',')
                        .help("Search for the entries of one or more pubkeys, delimited by commas"),
                )
                .arg(
                    Arg::new("verbose")
                        .short('v')
                        .long("verbose")
                        .action(ArgAction::SetTrue)
                        .help("Show additional account information"),
                ),
        )
        .get_matches();

    let subcommand = matches.subcommand();
    let subcommand_str = subcommand.map(|(name, _)| name).unwrap_or("").to_string();
    match subcommand {
        Some((CMD_INSPECT, subcommand_matches)) => cmd_inspect(&matches, subcommand_matches),
        Some((CMD_SEARCH, subcommand_matches)) => cmd_search(&matches, subcommand_matches),
        _ => unreachable!(),
    }
    .unwrap_or_else(|err| {
        eprintln!("Error: '{subcommand_str}' failed: {err}");
        std::process::exit(1);
    });
}

fn cmd_inspect(
    _app_matches: &ArgMatches,
    subcommand_matches: &ArgMatches,
) -> Result<(), String> {
    let path = subcommand_matches.get_one::<String>("path").unwrap().to_string();
    let verbose = subcommand_matches.get_flag("verbose");
    do_inspect(path, verbose)
}

fn cmd_search(
    _app_matches: &ArgMatches,
    subcommand_matches: &ArgMatches,
) -> Result<(), String> {
    let path = subcommand_matches.get_one::<String>("path").unwrap().to_string();
    let addresses: Vec<Pubkey> = subcommand_matches.get_many::<String>("addresses").unwrap().map(|s| s.parse().unwrap()).collect();
    let addresses = HashSet::from_iter(addresses);
    let verbose = subcommand_matches.get_flag("verbose");
    do_search(path, addresses, verbose)
}

fn do_inspect(file: impl AsRef<Path>, verbose: bool) -> Result<(), String> {
    let file_size = fs::metadata(&file)
        .map_err(|err| {
            format!(
                "failed to get file metadata '{}': {err}",
                file.as_ref().display(),
            )
        })?
        .len() as usize;

    let (storage, _size) =
        AccountsFile::new_from_file(file.as_ref(), file_size, StorageAccess::default()).map_err(
            |err| {
                format!(
                    "failed to open account storage file '{}': {err}",
                    file.as_ref().display(),
                )
            },
        )?;
    // By default, when the storage is dropped, the backing file will be removed.
    // We do not want to remove the backing file here in the store-tool, so prevent dropping.
    let storage = ManuallyDrop::new(storage);

    let data_size_width = width10(MAX_PERMITTED_DATA_LENGTH);
    let offset_width = width16(storage.capacity());

    let mut num_accounts = Saturating(0usize);
    let mut stored_accounts_size = Saturating(0);
    let mut lamports = Saturating(0);
    storage.scan_accounts_stored_meta(|account| {
        if verbose {
            println!("{account:?}");
        } else {
            println!(
                "{:#0offset_width$x}: {:44}, owner: {:44}, data size: {:data_size_width$}, lamports: {}",
                account.offset(),
                account.pubkey().to_string(),
                account.owner().to_string(),
                account.data_len(),
                account.lamports(),
            );
        }
        num_accounts += 1;
        stored_accounts_size += account.stored_size();
        lamports += account.lamports();
    }).map_err(|err| {
        format!(
            "failed to scan accounts in file '{}': {err}",
            file.as_ref().display(),
        )
    })?;

    println!(
        "number of accounts: {}, stored accounts size: {}, file size: {}, lamports: {}",
        num_accounts,
        stored_accounts_size,
        storage.capacity(),
        lamports,
    );
    Ok(())
}

fn do_search(
    dir: impl AsRef<Path>,
    addresses: HashSet<Pubkey>,
    verbose: bool,
) -> Result<(), String> {
    fn get_files_in(dir: impl AsRef<Path>) -> Result<Vec<PathBuf>, io::Error> {
        let mut files = Vec::new();
        let entries = fs::read_dir(dir)?;
        for entry in entries {
            let path = entry?.path();
            if path.is_file() {
                let path = fs::canonicalize(path)?;
                files.push(path);
            }
        }
        Ok(files)
    }

    let files = get_files_in(&dir).map_err(|err| {
        format!(
            "failed to get files in dir '{}': {err}",
            dir.as_ref().display(),
        )
    })?;
    files.par_iter().for_each(|file| {
        let file_size = match fs::metadata(file) {
            Ok(metadata) => metadata.len() as usize,
            Err(err) => {
                eprintln!(
                    "failed to get storage metadata '{}': {err}",
                    file.display(),
                );
                return;
            }
        };
        let Ok((storage, _size)) = AccountsFile::new_from_file(file, file_size, StorageAccess::default()).inspect_err(|err| {
            eprintln!(
                "failed to open account storage file '{}': {err}",
                file.display(),
            )
        }) else {
            return;
        };
        // By default, when the storage is dropped, the backing file will be removed.
        // We do not want to remove the backing file here in the store-tool, so prevent dropping.
        let storage = ManuallyDrop::new(storage);

        let file_name = Path::new(file.file_name().expect("path is a file"));
        storage.scan_accounts_stored_meta(|account| {
            if addresses.contains(account.pubkey()) {
                if verbose {
                    println!("storage: {}, {account:?}", file_name.display());
                } else {
                    println!(
                        "storage: {}, offset: {}, pubkey: {}, owner: {}, data size: {}, lamports: {}",
                        file_name.display(),
                        account.offset(),
                        account.pubkey(),
                        account.owner(),
                        account.data_len(),
                        account.lamports(),
                    );
                }
            }
        }).unwrap_or_else(|err| eprintln!("failed to scan accounts in file '{}': {err}",
                         file.display()));
    });

    Ok(())
}

/// Returns the number of characters required to print `x` in base-10
fn width10(x: u64) -> usize {
    (x as f64).log10().ceil() as usize
}

/// Returns the number of characters required to print `x` in base-16
fn width16(x: u64) -> usize {
    (x as f64).log(16.0).ceil() as usize
}
