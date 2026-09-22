mod utils;

use std::fs::{self, File};
use std::sync::atomic::{AtomicU64, Ordering};

use utils::*;

use rustarch::cli::{run_pack, run_unpack};
use rustarch::algorithms::{CipherId, CompressionId, FecId, PipelineSettings};
use rustarch::archiver::{
    ArchivedArtifactEntry, read_archive_entries, validate_payload_ranges,
};


static TEMP_TEST_COUNTER: AtomicU64 = AtomicU64::new(0);


fn compression_round_trip(
    compression: CompressionId,
    source: &[u8],
) -> (PipelineSettings, u64, u32)
{
    let id = TEMP_TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "rustarch_compression_{}_{}",
        std::process::id(),
        id,
    ));
    let source_file = root.join("payload.bin");
    let archive_path = root.join("payload.arch");
    let unpack_dir = root.join("unpacked");

    fs::create_dir_all(&root).unwrap();
    fs::write(&source_file, source).unwrap();

    let settings = PipelineSettings {
        compression,
        cipher: CipherId::NoCipher,
        fec: FecId::NoFec,
    };
    run_pack(&source_file, &archive_path, settings, &[]).unwrap();

    let mut archive = File::open(&archive_path).unwrap();
    let (entries, directories) = read_archive_entries(&mut archive).unwrap();
    assert!(directories.is_empty());
    assert_eq!(entries.len(), 1);
    let effective_pipeline = entries[0].pipeline;
    let stored_size = entries[0].stored_size;
    let crc32 = entries[0].crc32;
    drop(archive);

    run_unpack(&archive_path, &unpack_dir, &[]).unwrap();
    assert_eq!(fs::read(unpack_dir.join("payload.bin")).unwrap(), source);
    fs::remove_dir_all(root).unwrap();

    (effective_pipeline, stored_size, crc32)
}


#[test]
fn compression_formats_round_trip_without_duplicated_size_headers() {
    for compression in [
        CompressionId::NoCompression,
        CompressionId::RLE,
        CompressionId::Huffman,
        CompressionId::LZSS,
        CompressionId::Deflate,
    ] {
        let (effective, stored_size, crc32) = compression_round_trip(compression, &[]);
        assert_eq!(effective, PipelineSettings::default());
        assert_eq!(stored_size, 0);
        assert_eq!(crc32, u32::MAX);
        compression_round_trip(compression, &[0x5A]);
    }

    let compressible = vec![0u8; 64 * 1024];

    let (effective, stored_size, _) =
        compression_round_trip(CompressionId::Huffman, &compressible);
    assert_eq!(effective.compression, CompressionId::Huffman);
    assert_eq!(stored_size, 2 + 1 + 8);

    let (effective, _, _) = compression_round_trip(CompressionId::LZSS, &compressible);
    assert_eq!(effective.compression, CompressionId::LZSS);

    let (effective, _, _) = compression_round_trip(CompressionId::Deflate, &compressible);
    assert_eq!(effective.compression, CompressionId::Deflate);
}


#[test]
fn empty_file_bypasses_the_whole_pipeline() {
    let id = TEMP_TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "rustarch_empty_pipeline_{}_{}",
        std::process::id(),
        id,
    ));
    let source_file = root.join("empty.bin");
    let archive_path = root.join("empty.arch");
    let unpack_dir = root.join("unpacked");

    fs::create_dir_all(&root).unwrap();
    fs::write(&source_file, []).unwrap();

    let requested = PipelineSettings {
        compression: CompressionId::Huffman,
        cipher: CipherId::Xor,
        fec: FecId::ReedSolomon,
    };
    run_pack(&source_file, &archive_path, requested, &[1, 2, 3]).unwrap();

    let mut archive = File::open(&archive_path).unwrap();
    let (entries, directories) = read_archive_entries(&mut archive).unwrap();
    assert!(directories.is_empty());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].original_size, 0);
    assert_eq!(entries[0].stored_size, 0);
    assert_eq!(entries[0].pipeline, PipelineSettings::default());
    assert_eq!(entries[0].crc32, u32::MAX);
    drop(archive);

    run_unpack(&archive_path, &unpack_dir, &[]).unwrap();
    assert_eq!(fs::read(unpack_dir.join("empty.bin")).unwrap(), Vec::<u8>::new());
    fs::remove_dir_all(root).unwrap();
}


#[test]
fn mixed_archive_only_bypasses_pipeline_for_empty_files() {
    let id = TEMP_TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "rustarch_mixed_empty_pipeline_{}_{}",
        std::process::id(),
        id,
    ));
    let source_dir = root.join("source");
    let archive_path = root.join("mixed.arch");
    let unpack_dir = root.join("unpacked");
    let payload = vec![0x5A; 4096];
    let key = [1, 2, 3];

    fs::create_dir_all(&source_dir).unwrap();
    fs::write(source_dir.join("empty.bin"), []).unwrap();
    fs::write(source_dir.join("payload.bin"), &payload).unwrap();

    let requested = PipelineSettings {
        compression: CompressionId::Huffman,
        cipher: CipherId::Xor,
        fec: FecId::ReedSolomon,
    };
    run_pack(&source_dir, &archive_path, requested, &key).unwrap();

    let mut archive = File::open(&archive_path).unwrap();
    let (entries, _) = read_archive_entries(&mut archive).unwrap();
    let empty_entry = entries
        .iter()
        .find(|entry| entry.relative_path.ends_with("empty.bin"))
        .unwrap();
    let payload_entry = entries
        .iter()
        .find(|entry| entry.relative_path.ends_with("payload.bin"))
        .unwrap();

    assert_eq!(empty_entry.pipeline, PipelineSettings::default());
    assert_eq!(empty_entry.stored_size, 0);
    assert_eq!(payload_entry.pipeline, requested);
    assert!(payload_entry.stored_size > 0);
    drop(archive);

    run_unpack(&archive_path, &unpack_dir, &key).unwrap();
    assert_eq!(fs::read(unpack_dir.join("empty.bin")).unwrap(), Vec::<u8>::new());
    assert_eq!(fs::read(unpack_dir.join("payload.bin")).unwrap(), payload);
    fs::remove_dir_all(root).unwrap();
}


#[test]
fn empty_entry_validation_rejects_noncanonical_fields() {
    let canonical = ArchivedArtifactEntry {
        relative_path: "empty.bin".into(),
        original_size: 0,
        stored_size: 0,
        payload_offset: 0,
        crc32: u32::MAX,
        pipeline: PipelineSettings::default(),
    };
    validate_payload_ranges(std::slice::from_ref(&canonical), 0).unwrap();

    let mut with_payload = canonical.clone();
    with_payload.stored_size = 1;
    assert!(validate_payload_ranges(&[with_payload], 1).is_err());

    let mut with_pipeline = canonical.clone();
    with_pipeline.pipeline.compression = CompressionId::Huffman;
    assert!(validate_payload_ranges(&[with_pipeline], 0).is_err());

    let mut with_crc = canonical;
    with_crc.crc32 = 0;
    assert!(validate_payload_ranges(&[with_crc], 0).is_err());
}


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
