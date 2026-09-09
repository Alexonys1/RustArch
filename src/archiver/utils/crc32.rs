//! Реализация CRC32 (полином IEEE 802.3, тот же, что в zip/png/ethernet).
//!
//! Нужен архиву для проверки целостности: контрольная сумма считается от
//! ИСХОДНЫХ (несжатых, нешифрованных) данных файла и хранится в записи
//! архива. После полной обратной цепочки (fec-decode -> decrypt ->
//! decompress) распаковщик пересчитывает CRC32 результата и сравнивает их.
//!
//! Считается потоково (`update` можно вызывать многократно кусками),
//! чтобы не требовать держать весь файл в памяти ради контрольной суммы.

use crate::archiver::Artifact;


pub struct Crc32 {
    state: u32,
}

const POLY: u32 = 0xEDB88320;

fn table_entry(mut byte: u32) -> u32 {
    for _ in 0..8 {
        byte = if byte & 1 == 1 {
            (byte >> 1) ^ POLY
        } else {
            byte >> 1
        };
    }
    byte
}

impl Crc32 {
    pub fn new() -> Self {
        Self { state: 0xFFFF_FFFF }
    }

    /// Добавляет очередной кусок данных в подсчёт. Кол-во вызовов не ограничено.
    pub fn update(&mut self, data: &[u8]) {
        for &byte in data {
            let idx = ((self.state ^ byte as u32) & 0xFF) as u32;
            let entry = table_entry(idx);
            self.state = entry ^ (self.state >> 8);
        }
    }

    pub fn finalize(self) -> u32 {
        self.state ^ 0xFFFF_FFFF
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}


pub fn crc32_of_artifact_and_rewind(artifact: &mut Artifact) -> std::io::Result<u32> {
    let mut crc32 = Crc32::new();
    let chunk_size: usize = artifact.chunk_size.get();
    let mut buffer = vec![0; chunk_size]; // Нужно именно передать заполненный вектор,
    // а не Vec::with_capacity(chunk_size). Иначе будет запись в неинициализированную память!

    loop {
        let readed_bytes: usize = artifact.read_chunk(&mut buffer)?;
        if readed_bytes == 0 { break; }
        crc32.update(&buffer[..readed_bytes]);
    }

    artifact.rewind_reading();

    Ok(crc32.finalize())
}
