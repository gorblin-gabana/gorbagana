use {crate::ArgConstant, clap::Arg};

pub const MEMO_ARG: ArgConstant<'static> = ArgConstant {
    name: "memo",
    long: "--with-memo",
    help: "Specify a memo string to include in the transaction.",
};

pub fn memo_arg() -> Arg {
    Arg::new(MEMO_ARG.name)
        .long(MEMO_ARG.long)
        .value_name("MEMO")
        .help(MEMO_ARG.help)
}
