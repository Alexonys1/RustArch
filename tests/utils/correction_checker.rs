use std::fs::File;
use std::path::Path;
use std::io::{BufReader, Read};

use rustarch::archiver::{walk_directory_or_file, Crc32, DEFAULT_CHUNK_SIZE_IN_BYTES};


/// Пустые папки не учитываются. TODO: Потом как-нибудь допишу.
pub fn crc32_from_dir(dir_path: &Path) -> u32 {
    let walk_result = walk_directory_or_file(dir_path).unwrap();
    let files = walk_result.files;
    let _empty_dirs = walk_result.empty_dirs;

    let mut hasher = Crc32::new();
    for file in files {
        hasher.update(
            &crc32_from_file_without_finalize(&file.absolute_path).to_ne_bytes()
        );
    }

    hasher.finalize()
}


pub fn crc32_from_file_without_finalize(filepath: &Path) -> u32 {
    let mut file_reader = BufReader::new(File::open(filepath).unwrap());
    let mut hasher = Crc32::new();

    let mut buffer: Vec<u8> = vec![0; DEFAULT_CHUNK_SIZE_IN_BYTES];
    loop {
        let n = file_reader.read(&mut buffer).unwrap();
        if n == 0 { break; }
        hasher.update(&buffer[..n]);
    }

    hasher.into()
}
