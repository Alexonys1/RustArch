mod utils;

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use utils::*;

use RustArch::cli::{run_pack, run_unpack};
use RustArch::algorithms::{CipherId, CompressionId, FecId, PipelineSettings};


static TEMP_TEST_COUNTER: AtomicU64 = AtomicU64::new(0);


#[test]
fn reed_solomon_pack_and_unpack_round_trip() {
    let id = TEMP_TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "rustarch_pipeline_rs_{}_{}",
        std::process::id(),
        id,
    ));
    let source_dir = root.join("source");
    let source_file = source_dir.join("payload.bin");
    let archive_path = root.join("payload.arch");
    let unpack_dir = root.join("unpacked");
    let source: Vec<u8> = (0..700)
        .map(|index| ((index * 41 + index / 5 + 7) % 256) as u8)
        .collect();

    fs::create_dir_all(&source_dir).unwrap();
    fs::write(&source_file, &source).unwrap();

    let settings = PipelineSettings {
        compression: CompressionId::NoCompression,
        cipher: CipherId::NoCipher,
        fec: FecId::ReedSolomon,
    };

    run_pack(&source_dir, &archive_path, settings, &[]).unwrap();
    run_unpack(&archive_path, &unpack_dir, &[]).unwrap();

    assert_eq!(fs::read(unpack_dir.join("payload.bin")).unwrap(), source);
    fs::remove_dir_all(root).unwrap();
}


#[test]
#[ignore = "Проходит полминуты где-то."]
#[allow(non_snake_case)]
fn TEST_1_Huffman_NoCipher_NoFec_pack_and_unpack() { // TODO: Это отличный повод для фикстуры!!!! Все такие тесты однотипные!
    // ==================> PACK:
    let source_path = TEST1.get_source_path();
    let target_archive_path = TEST1.get_target_archive_path();
    let encode_key = [1, 2, 3];
    let settings = PipelineSettings {
        compression: CompressionId::Huffman,
        cipher: CipherId::NoCipher,
        fec: FecId::NoFec,
    };

    // ===================> UNPACK:
    let unpack_path = TEST1.get_unpack_path();

    // =================> ACTION: (неужели я начал писать тесты?) :D
    let _ = run_pack(&source_path, &target_archive_path, settings, &encode_key);
    let _ = run_unpack(&target_archive_path, &unpack_path, &encode_key);

    // ===================> ASSERT:
    let crc32_of_source = crc32_from_dir(&source_path);
    let crc32_of_unpack = crc32_from_dir(&unpack_path);

    assert_eq!(crc32_of_source, crc32_of_unpack);
}


#[test]
#[ignore = "Проходит полминуты где-то."]
#[allow(non_snake_case)]
fn TEST_2_Huffman_NoCipher_NoFec_pack_and_unpack() { // TODO: Это отличный повод для фикстуры!!!! Все такие тесты однотипные!
    // ==================> PACK:
    let source_path = TEST2.get_source_path();
    let target_archive_path = TEST2.get_target_archive_path();
    let encode_key = [1, 2, 3];
    let settings = PipelineSettings {
        compression: CompressionId::Huffman,
        cipher: CipherId::NoCipher,
        fec: FecId::NoFec,
    };

    // ===================> UNPACK:
    let unpack_path = TEST2.get_unpack_path();

    // =================> ACTION: (неужели я начал писать тесты?) :D
    let _ = run_pack(&source_path, &target_archive_path, settings, &encode_key);
    let _ = run_unpack(&target_archive_path, &unpack_path, &encode_key);

    // ===================> ASSERT:
    let crc32_of_source = crc32_from_dir(&source_path);
    let crc32_of_unpack = crc32_from_dir(&unpack_path);

    assert_eq!(crc32_of_source, crc32_of_unpack);
}
