use {
    crate::{input_validators, ArgConstant},
    clap::Arg,
};

pub const FEE_PAYER_ARG: ArgConstant<'static> = ArgConstant {
    name: "fee_payer",
    long: "fee-payer",
    help: "Specify the fee-payer account. This may be a keypair file, the ASK keyword \n\
           or the pubkey of an offline signer, provided an appropriate --signer argument \n\
           is also passed. Defaults to the client keypair.",
};

pub fn fee_payer_arg<'help>() -> Arg<'help> {
    Arg::new(FEE_PAYER_ARG.name)
        .long(FEE_PAYER_ARG.long)
        .value_name("KEYPAIR")
        .validator(|s| input_validators::is_valid_signer_simple(s))
        .help(FEE_PAYER_ARG.help)
}
