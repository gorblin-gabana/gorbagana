use {
    crate::{input_validators, ArgConstant},
    clap::{Command, Arg},
};

pub const BLOCKHASH_ARG: ArgConstant<'static> = ArgConstant {
    name: "blockhash",
    long: "blockhash",
    help: "Use the supplied blockhash",
};

pub const SIGN_ONLY_ARG: ArgConstant<'static> = ArgConstant {
    name: "sign_only",
    long: "sign-only",
    help: "Sign the transaction offline",
};

pub const SIGNER_ARG: ArgConstant<'static> = ArgConstant {
    name: "signer",
    long: "signer",
    help: "Provide a public-key/signature pair for the transaction",
};

pub const DUMP_TRANSACTION_MESSAGE: ArgConstant<'static> = ArgConstant {
    name: "dump_transaction_message",
    long: "dump-transaction-message",
    help: "Display the base64 encoded binary transaction message in sign-only mode",
};

pub fn blockhash_arg<'help>() -> Arg<'help> {
    Arg::new(BLOCKHASH_ARG.name)
        .long(BLOCKHASH_ARG.long)
        .value_name("BLOCKHASH")
        .validator(|s| input_validators::is_hash_simple(s))
        .help(BLOCKHASH_ARG.help)
}

pub fn sign_only_arg<'help>() -> Arg<'help> {
    Arg::new(SIGN_ONLY_ARG.name)
        .long(SIGN_ONLY_ARG.long)
        .requires(BLOCKHASH_ARG.name)
        .help(SIGN_ONLY_ARG.help)
}

fn signer_arg<'help>() -> Arg<'help> {
    Arg::new(SIGNER_ARG.name)
        .long(SIGNER_ARG.long)
        .value_name("PUBKEY=SIGNATURE")
        .validator(|s| input_validators::is_pubkey_sig_simple(s))
        .requires(BLOCKHASH_ARG.name)
        .multiple(true)
        .number_of_values(1)
        .help(SIGNER_ARG.help)
}

pub fn dump_transaction_message<'help>() -> Arg<'help> {
    Arg::new(DUMP_TRANSACTION_MESSAGE.name)
        .long(DUMP_TRANSACTION_MESSAGE.long)
        .requires(SIGN_ONLY_ARG.name)
        .help(DUMP_TRANSACTION_MESSAGE.help)
}

pub trait ArgsConfig {
    fn blockhash_arg<'help>(&self, arg: Arg<'help>) -> Arg<'help> {
        arg
    }
    fn sign_only_arg<'help>(&self, arg: Arg<'help>) -> Arg<'help> {
        arg
    }
    fn signer_arg<'help>(&self, arg: Arg<'help>) -> Arg<'help> {
        arg
    }
    fn dump_transaction_message_arg<'help>(&self, arg: Arg<'help>) -> Arg<'help> {
        arg
    }
}

pub trait OfflineArgs {
    fn offline_args(self) -> Self;
    fn offline_args_config(self, config: &dyn ArgsConfig) -> Self;
}

impl OfflineArgs for Command<'_> {
    fn offline_args_config(self, config: &dyn ArgsConfig) -> Self {
        self.arg(config.blockhash_arg(blockhash_arg()))
            .arg(config.sign_only_arg(sign_only_arg()))
            .arg(config.signer_arg(signer_arg()))
            .arg(config.dump_transaction_message_arg(dump_transaction_message()))
    }
    fn offline_args(self) -> Self {
        struct NullArgsConfig {}
        impl ArgsConfig for NullArgsConfig {}
        self.offline_args_config(&NullArgsConfig {})
    }
}
