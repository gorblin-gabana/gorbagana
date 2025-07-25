use {
    crate::{input_validators, offline::BLOCKHASH_ARG, ArgConstant},
    clap::{Command, Arg},
};

pub const NONCE_ARG: ArgConstant<'static> = ArgConstant {
    name: "nonce",
    long: "nonce",
    help: "Provide the nonce account to use when creating a nonced \n\
           transaction. Nonced transactions are useful when a transaction \n\
           requires a lengthy signing process. Learn more about nonced \n\
           transactions at https://docs.solanalabs.com/cli/examples/durable-nonce",
};

pub const NONCE_AUTHORITY_ARG: ArgConstant<'static> = ArgConstant {
    name: "nonce_authority",
    long: "nonce-authority",
    help: "Provide the nonce authority keypair to use when signing a nonced transaction",
};

fn nonce_arg<'help>() -> Arg<'help> {
    Arg::new(NONCE_ARG.name)
        .long(NONCE_ARG.long)
        .value_name("PUBKEY")
        .requires(BLOCKHASH_ARG.name)
        .validator(|s| input_validators::is_valid_pubkey_simple(s))
        .help(NONCE_ARG.help)
}

pub fn nonce_authority_arg<'help>() -> Arg<'help> {
    Arg::new(NONCE_AUTHORITY_ARG.name)
        .long(NONCE_AUTHORITY_ARG.long)
        .value_name("KEYPAIR")
        .validator(|s| input_validators::is_valid_signer_simple(s))
        .help(NONCE_AUTHORITY_ARG.help)
}

pub trait NonceArgs {
    fn nonce_args(self, global: bool) -> Self;
}

impl NonceArgs for Command<'_> {
    fn nonce_args(self, global: bool) -> Self {
        self.arg(nonce_arg().global(global)).arg(
            nonce_authority_arg()
                .requires(NONCE_ARG.name)
                .global(global),
        )
    }
}
