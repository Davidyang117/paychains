pub use paychains_program::entrypoint::*;

#[macro_export]
#[deprecated(
    since = "1.4.3",
    note = "use paychains_program::entrypoint::entrypoint instead"
)]
macro_rules! entrypoint {
    ($process_instruction:ident) => {
        #[cfg(all(not(feature = "custom-heap"), not(test)))]
        #[global_allocator]
        static A: $crate::entrypoint::BumpAllocator = $crate::entrypoint::BumpAllocator {
            start: $crate::entrypoint::HEAP_START_ADDRESS,
            len: $crate::entrypoint::HEAP_LENGTH,
        };

        /// # Safety
        #[no_mangle]
        pub unsafe extern "C" fn entrypoint(input: *mut u8) -> u64 {
            let (program_id, accounts, instruction_data) =
                unsafe { $crate::entrypoint::deserialize(input) };
            match $process_instruction(&program_id, &accounts, &instruction_data) {
                Ok(()) => $crate::entrypoint::SUCCESS,
                Err(error) => error.into(),
            }
        }
    };
}
