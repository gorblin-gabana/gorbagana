use {
    crate::{input_validators, ArgConstant},
    clap::Arg,
};

pub const COMPUTE_UNIT_PRICE_ARG: ArgConstant<'static> = ArgConstant {
    name: "compute_unit_price",
    long: "--with-compute-unit-price",
    help: "Set compute unit price for transaction, in increments of 0.000001 lamports per compute unit.",
};

pub const COMPUTE_UNIT_LIMIT_ARG: ArgConstant<'static> = ArgConstant {
    name: "compute_unit_limit",
    long: "--with-compute-unit-limit",
    help: "Set compute unit limit for transaction.",
};

pub fn compute_unit_price_arg<'help>() -> Arg<'help> {
    Arg::new(COMPUTE_UNIT_PRICE_ARG.name)
        .long(COMPUTE_UNIT_PRICE_ARG.long)
        .value_name("COMPUTE-UNIT-PRICE")
        .validator(|s| input_validators::is_parsable_u64(s))
        .help(COMPUTE_UNIT_PRICE_ARG.help)
}

pub fn compute_unit_limit_arg<'help>() -> Arg<'help> {
    Arg::new(COMPUTE_UNIT_LIMIT_ARG.name)
        .long(COMPUTE_UNIT_LIMIT_ARG.long)
        .value_name("COMPUTE-UNIT-LIMIT")
        .validator(|s| input_validators::is_parsable_u32(s))
        .help(COMPUTE_UNIT_LIMIT_ARG.help)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ComputeUnitLimit {
    /// Do not include a compute unit limit instruction, which will give the
    /// transaction a compute unit limit of:
    /// `min(1_400_000, 200_000 * (num_top_level_instructions - num_compute_budget_instructions))`
    Default,
    /// Use a static predefined limit
    Static(u32),
    /// Simulate the transaction to find out the compute unit usage
    Simulated,
}
