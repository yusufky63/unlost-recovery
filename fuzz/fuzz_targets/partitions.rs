#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| unlost_formats::fuzzing::partitions(data));
