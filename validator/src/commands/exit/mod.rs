#[cfg(target_os = "linux")]
use std::{io, thread, time::Duration};
use {
    crate::{
        admin_rpc_service,
        commands::{monitor, wait_for_restart_window, Error, FromClapArgMatches, Result},
    },
    clap::{Arg, ArgMatches, Command, ArgAction},
    solana_clap_utils::input_validators::{is_parsable, is_valid_percentage},
    std::path::Path,
};

const COMMAND: &str = "exit";

const DEFAULT_MIN_IDLE_TIME: &str = "10";
const DEFAULT_MAX_DELINQUENT_STAKE: &str = "5";

#[derive(Clone, Debug, PartialEq)]
pub enum PostExitAction {
    // Run the agave-validator monitor command indefinitely
    Monitor,
    // Block until the exiting validator process has terminated
    Wait,
}

#[derive(Debug, PartialEq)]
pub struct ExitArgs {
    pub force: bool,
    pub post_exit_action: Option<PostExitAction>,
    pub min_idle_time: usize,
    pub max_delinquent_stake: u8,
    pub skip_new_snapshot_check: bool,
    pub skip_health_check: bool,
}

impl FromClapArgMatches for ExitArgs {
    fn from_clap_arg_match(matches: &ArgMatches) -> Result<Self> {
        let post_exit_action = if matches.get_flag("monitor") {
            Some(PostExitAction::Monitor)
        } else if matches.get_flag("wait_for_exit") {
            Some(PostExitAction::Wait)
        } else {
            None
        };

        Ok(ExitArgs {
            force: matches.get_flag("force"),
            post_exit_action,
            min_idle_time: matches
                .get_one::<String>("min_idle_time")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or_else(|| {
                    eprintln!("min_idle_time is required");
                    std::process::exit(1);
                }),
            max_delinquent_stake: matches
                .get_one::<String>("max_delinquent_stake")
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or_else(|| {
                    eprintln!("max_delinquent_stake is required");
                    std::process::exit(1);
                }),
            skip_new_snapshot_check: matches.get_flag("skip_new_snapshot_check"),
            skip_health_check: matches.get_flag("skip_health_check"),
        })
    }
}

pub fn command() -> Command {
    Command::new(COMMAND)
        .about("Send an exit request to the validator")
        .arg(
            Arg::new("force")
                .short('f')
                .long("force")
                .action(ArgAction::SetTrue)
                .help(
                    "Request the validator exit immediately instead of waiting for a restart window",
                ),
        )
        .arg(
            Arg::new("monitor")
                .short('m')
                .long("monitor")
                .action(ArgAction::SetTrue)
                .help("Monitor the validator after sending the exit request"),
        )
        .arg(
            Arg::new("wait_for_exit")
                .long("wait-for-exit")
                .action(ArgAction::SetTrue)
                .conflicts_with("monitor")
                .help("Wait for the validator to terminate after sending the exit request"),
        )
        .arg(
            Arg::new("min_idle_time")
                .long("min-idle-time")
                .value_parser(clap::value_parser!(usize))
                .value_name("MINUTES")
                .default_value(DEFAULT_MIN_IDLE_TIME)
                .help(
                    "Minimum time that the validator should not be leader before restarting",
                ),
        )
        .arg(
            Arg::new("max_delinquent_stake")
                .long("max-delinquent-stake")
                .value_parser(clap::value_parser!(u8))
                .default_value(DEFAULT_MAX_DELINQUENT_STAKE)
                .value_name("PERCENT")
                .help("The maximum delinquent stake % permitted for an exit"),
        )
        .arg(
            Arg::new("skip_new_snapshot_check")
                .long("skip-new-snapshot-check")
                .action(ArgAction::SetTrue)
                .help("Skip check for a new snapshot"),
        )
        .arg(
            Arg::new("skip_health_check")
                .long("skip-health-check")
                .action(ArgAction::SetTrue)
                .help("Skip health check"),
        )
}

pub fn execute(matches: &ArgMatches, ledger_path: &Path) -> Result<()> {
    let exit_args = ExitArgs::from_clap_arg_match(matches)?;

    if !exit_args.force {
        wait_for_restart_window::wait_for_restart_window(
            ledger_path,
            None,
            exit_args.min_idle_time,
            exit_args.max_delinquent_stake,
            exit_args.skip_new_snapshot_check,
            exit_args.skip_health_check,
        )?;
    }

    // Grab the pid from the process before initiating exit as the running
    // validator will be unable to respond after exit has returned.
    //
    // Additionally, only check the pid() RPC call result if it will be used.
    // In an upgrade scenario, it is possible that a binary that calls pid()
    // will be initating exit against a process that doesn't support pid().
    // Since PostExitAction::Wait case is opt-in (via --wait-for-exit), the
    // result is checked ONLY in that case to provide a friendlier upgrade
    // path for users who are NOT using --wait-for-exit
    const WAIT_FOR_EXIT_UNSUPPORTED_ERROR: &str =
        "remote process exit cannot be waited on. `--wait-for-exit` is not supported by the remote process";
    let post_exit_action = exit_args.post_exit_action.clone();
    let validator_pid = admin_rpc_service::runtime().block_on(async move {
        let admin_client = admin_rpc_service::connect(ledger_path).await?;
        let validator_pid = match post_exit_action {
            Some(PostExitAction::Wait) => admin_client
                .pid()
                .await
                .map_err(|_err| Error::Dynamic(WAIT_FOR_EXIT_UNSUPPORTED_ERROR.into()))?,
            _ => 0,
        };
        admin_client.exit().await?;

        Ok::<u32, Error>(validator_pid)
    })?;

    println!("Exit request sent");

    match exit_args.post_exit_action {
        None => Ok(()),
        Some(PostExitAction::Monitor) => monitor::execute(matches, ledger_path),
        Some(PostExitAction::Wait) => poll_until_pid_terminates(validator_pid),
    }?;

    Ok(())
}

#[cfg(target_os = "linux")]
fn poll_until_pid_terminates(pid: u32) -> Result<()> {
    let pid = i32::try_from(pid)?;

    println!("Waiting for agave-validator process {pid} to terminate");
    loop {
        // From man kill(2)
        //
        // If sig is 0, then no signal is sent, but existence and permission
        // checks are still performed; this can be used to check for the
        // existence of a process ID or process group ID that the caller is
        // permitted to signal.
        let result = unsafe {
            libc::kill(pid, /*sig:*/ 0)
        };
        if result >= 0 {
            // Give the process some time to exit before checking again
            thread::sleep(Duration::from_millis(500));
        } else {
            let errno = io::Error::last_os_error()
                .raw_os_error()
                .ok_or(Error::Dynamic("unable to read raw os error".into()))?;
            match errno {
                libc::ESRCH => {
                    println!("Done, agave-validator process {pid} has terminated");
                    break;
                }
                libc::EINVAL => {
                    // An invalid signal was specified, we only pass sig=0 so
                    // this should not be possible
                    Err(Error::Dynamic(
                        format!("unexpected invalid signal error for kill({pid}, 0)").into(),
                    ))?;
                }
                libc::EPERM => {
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))?;
                }
                unknown => {
                    Err(Error::Dynamic(
                        format!("unexpected errno for kill({pid}, 0): {unknown}").into(),
                    ))?;
                }
            }
        }
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn poll_until_pid_terminates(_pid: u32) -> Result<()> {
    Err(Error::Dynamic(
        "Unable to wait for agave-validator process termination on this platform".into(),
    ))
}

#[cfg(test)]
mod tests {
    use {super::*, crate::commands::tests::verify_args_struct_by_command};

    impl Default for ExitArgs {
        fn default() -> Self {
            ExitArgs {
                min_idle_time: DEFAULT_MIN_IDLE_TIME
                    .parse()
                    .expect("invalid DEFAULT_MIN_IDLE_TIME"),
                max_delinquent_stake: DEFAULT_MAX_DELINQUENT_STAKE
                    .parse()
                    .expect("invalid DEFAULT_MAX_DELINQUENT_STAKE"),
                force: false,
                post_exit_action: None,
                skip_new_snapshot_check: false,
                skip_health_check: false,
            }
        }
    }

    #[test]
    fn verify_args_struct_by_command_exit_default() {
        verify_args_struct_by_command(command(), vec![COMMAND], ExitArgs::default());
    }

    #[test]
    fn verify_args_struct_by_command_exit_with_force() {
        verify_args_struct_by_command(
            command(),
            vec![COMMAND, "--force"],
            ExitArgs {
                force: true,
                ..ExitArgs::default()
            },
        );
    }

    #[test]
    fn verify_args_struct_by_command_exit_with_post_exit_action() {
        verify_args_struct_by_command(
            command(),
            vec![COMMAND, "--monitor"],
            ExitArgs {
                post_exit_action: Some(PostExitAction::Monitor),
                ..ExitArgs::default()
            },
        );

        verify_args_struct_by_command(
            command(),
            vec![COMMAND, "--wait-for-exit"],
            ExitArgs {
                post_exit_action: Some(PostExitAction::Wait),
                ..ExitArgs::default()
            },
        );
    }

    #[test]
    fn verify_args_struct_by_command_exit_with_min_idle_time() {
        verify_args_struct_by_command(
            command(),
            vec![COMMAND, "--min-idle-time", "60"],
            ExitArgs {
                min_idle_time: 60,
                ..ExitArgs::default()
            },
        );
    }

    #[test]
    fn verify_args_struct_by_command_exit_with_max_delinquent_stake() {
        verify_args_struct_by_command(
            command(),
            vec![COMMAND, "--max-delinquent-stake", "10"],
            ExitArgs {
                max_delinquent_stake: 10,
                ..ExitArgs::default()
            },
        );
    }

    #[test]
    fn verify_args_struct_by_command_exit_with_skip_new_snapshot_check() {
        verify_args_struct_by_command(
            command(),
            vec![COMMAND, "--skip-new-snapshot-check"],
            ExitArgs {
                skip_new_snapshot_check: true,
                ..ExitArgs::default()
            },
        );
    }

    #[test]
    fn verify_args_struct_by_command_exit_with_skip_health_check() {
        verify_args_struct_by_command(
            command(),
            vec![COMMAND, "--skip-health-check"],
            ExitArgs {
                skip_health_check: true,
                ..ExitArgs::default()
            },
        );
    }
}
