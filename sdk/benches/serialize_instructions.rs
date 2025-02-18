#![feature(test)]

extern crate test;
use {
    bincode::{deserialize, serialize},
    paychains_sdk::{
        instruction::{AccountMeta, Instruction},
        message::{Message, SanitizedMessage},
        pubkey::{self, Pubkey},
        sysvar::instructions,
    },
    std::convert::TryFrom,
    test::Bencher,
};

fn make_instructions() -> Vec<Instruction> {
    let meta = AccountMeta::new(pubkey::new_rand(), false);
    let inst = Instruction::new_with_bincode(pubkey::new_rand(), &[0; 10], vec![meta; 4]);
    vec![inst; 4]
}

#[bench]
fn bench_bincode_instruction_serialize(b: &mut Bencher) {
    let instructions = make_instructions();
    b.iter(|| {
        test::black_box(serialize(&instructions).unwrap());
    });
}

#[bench]
fn bench_manual_instruction_serialize(b: &mut Bencher) {
    let instructions = make_instructions();
    let message =
        SanitizedMessage::try_from(Message::new(&instructions, Some(&Pubkey::new_unique())))
            .unwrap();
    b.iter(|| {
        test::black_box(message.serialize_instructions());
    });
}

#[bench]
fn bench_bincode_instruction_deserialize(b: &mut Bencher) {
    let instructions = make_instructions();
    let serialized = serialize(&instructions).unwrap();
    b.iter(|| {
        test::black_box(deserialize::<Vec<Instruction>>(&serialized).unwrap());
    });
}

#[bench]
fn bench_manual_instruction_deserialize(b: &mut Bencher) {
    let instructions = make_instructions();
    let message =
        SanitizedMessage::try_from(Message::new(&instructions, Some(&Pubkey::new_unique())))
            .unwrap();
    let serialized = message.serialize_instructions();
    b.iter(|| {
        for i in 0..instructions.len() {
            #[allow(deprecated)]
            test::black_box(instructions::load_instruction_at(i, &serialized).unwrap());
        }
    });
}

#[bench]
fn bench_manual_instruction_deserialize_single(b: &mut Bencher) {
    let instructions = make_instructions();
    let message =
        SanitizedMessage::try_from(Message::new(&instructions, Some(&Pubkey::new_unique())))
            .unwrap();
    let serialized = message.serialize_instructions();
    b.iter(|| {
        #[allow(deprecated)]
        test::black_box(instructions::load_instruction_at(3, &serialized).unwrap());
    });
}
