//! Реализация CRC32 (полином IEEE 802.3).

use crate::algorithms::{PipelineSettings, CipherId};
use crate::archiver::Artifact;


pub struct Crc32 {
    state: u32,
}


impl Crc32 {
    pub const fn new() -> Self {
        Self { state: 0xFFFF_FFFF }
    }

    /// Добавляет очередной кусок данных в подсчёт. Кол-во вызовов не ограничено.
    pub fn update(&mut self, data: &[u8]) {
        for &byte in data {
            let idx: u32 = (self.state ^ byte as u32) & 0xFF;
            let entry: u32 = table_entry(idx);
            self.state = entry ^ (self.state >> 8);
        }
    }

    pub const fn finalize(self) -> u32 {
        self.state ^ 0xFFFF_FFFF
    }
}


impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}


/// Не финализирует crc32, просто возвращая self.state!
impl Into<u32> for Crc32 {
    fn into(self) -> u32 {
        self.state
    }
}


/// Если NoCipher, то crc32 всегда равен u32::MAX. Это я сделал для того, чтобы crc32 не считался для файлов без шифрации.
pub fn crc32_of_artifact_and_rewind(artifact: &mut Artifact, pipeline_settings: PipelineSettings) -> std::io::Result<u32> {
    if pipeline_settings.cipher == CipherId::NoCipher {
        artifact.rewind_reading();
        return Ok(u32::MAX)  // Просто заглушка
    }

    let mut crc32 = Crc32::new();
    let chunk_size: usize = artifact.chunk_size.get();
    let mut buffer = vec![0; chunk_size]; // Нужно именно передать заполненный вектор,
    // а не Vec::with_capacity(chunk_size). Иначе будет запись в неинициализированную память!

    loop {
        let readed_bytes: usize = artifact.read_chunk_to(&mut buffer)?;
        if readed_bytes == 0 { break; }
        crc32.update(&buffer[..readed_bytes]);
    }

    artifact.rewind_reading();

    Ok(crc32.finalize())
}


fn table_entry(mut byte: u32) -> u32 {
    const POLY: u32 = 0xEDB88320;

    for _ in 0..8 {
        byte = if byte & 1 == 1 {
            (byte >> 1) ^ POLY
        } else {
            byte >> 1
        };
    }
    byte
}
