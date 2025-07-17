#![no_std]

pub mod error;
pub mod instruction;
pub mod native_mint;
pub mod state;

pub mod program {
    pinocchio_pubkey::declare_id!("Gorbj8Dp27NkXMQUkeHBSmpf6iQ3yT4b2uVe8kM4s6br");
}
