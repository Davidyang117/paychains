//! Example Rust-based BPF upgraded program

extern crate paychains_program;
use paychains_program::{
    account_info::AccountInfo, entrypoint, entrypoint::ProgramResult, msg, pubkey::Pubkey,
    sysvar::clock,
};

entrypoint!(process_instruction);
fn process_instruction(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    _instruction_data: &[u8],
) -> ProgramResult {
    msg!("Upgraded program");
    assert_eq!(accounts.len(), 1);
    assert_eq!(*accounts[0].key, clock::id());
    Err(43.into())
}
