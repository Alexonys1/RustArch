mod utils;

use utils::*;

use RustArch::cli::{run_pack, run_unpack};
use RustArch::algorithms::{CipherId, CompressionId, FecId, PipelineSettings};


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
