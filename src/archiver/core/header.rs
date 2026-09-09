//! Бинарный формат файла архива.
//!
//! ```text
//! [0..16)                 заголовок (magic, version, pack_pipeline, entry_count, reserved)
//! [16..X)                 entry_count записей переменной длины
//! [X..конец файла)        payload файла #1 | payload файла #2 | ...
//! ```
//! Все числа - little-endian.
//!
//! Заголовок (16 байт):
//! ```text
//! offset 0    size 4   magic          b"RARC"
//! offset 4    size 2   version        u16 LE
//! offset 6    size 1   compression_id u8   <- ОДИН на весь архив
//! offset 7    size 1   cipher_id      u8   <- ОДИН на весь архив
//! offset 8    size 1   fec_id         u8   <- ОДИН на весь архив
//! offset 9    size 1   flags          зарезервировано, сейчас 0
//! offset 10   size 4   entry_count    u32 LE
//! offset 14   size 2   reserved
//! ```
//! Единый пайплайн на архив позволяет
//! распаковывать записи полностью параллельно и независимо друг от
//! друга, читая пайплайн один раз из заголовка ДО запуска потоков,
//! а не заново для каждой записи.
//!
//! Запись (переменная длина: 2 + path_len + 29 байт):
//! ```text
//! offset 0                size 2            path_len          u16 LE
//! offset 2                size path_len     relative_path     UTF-8, '/'-разделитель
//! offset +path_len        size 8            original_size     u64 LE
//! offset +8               size 8            stored_size       u64 LE
//! offset +8               size 8            payload_offset    u64 LE, абсолютное смещение в архиве
//! offset +8               size 1            entry_flags       бит 0 = IS_DIRECTORY
//! offset +1               size 4            crc32             u32 LE, от ИСХОДНЫХ данных
//! ```
//! Своя частотная таблица Huffman/окно LZ77 (если применимо) - это уже
//! формат самого потока payload'а конкретного алгоритма, а не формата
//! архива: архив видит payload как непрозрачные `stored_size` байт.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

use crate::error::AppError;
use crate::algorithms::{CipherId, CompressionId, FecId};
use crate::algorithms::PipelineSettings;


pub const MAGIC: [u8; 4] = *b"RARC";
pub const FORMAT_VERSION: u16 = 1;
pub const ENTRY_FLAG_IS_DIRECTORY: u8 = 1 << 0;


#[derive(Debug, Clone)]
pub struct ArchiveEntry {
    pub relative_path: String,
    pub original_size: u64,
    pub stored_size: u64,
    pub payload_offset: u64,
    pub is_directory: bool,
    pub crc32: u32,
}

impl ArchiveEntry {
    fn entry_flags(&self) -> u8 {
        if self.is_directory { ENTRY_FLAG_IS_DIRECTORY } else { 0 }
    }
}


pub fn write_header(file: &mut File, entry_count: u32, pipeline: PipelineSettings) -> Result<(), AppError> {
    file.write_all(&MAGIC)?;
    file.write_all(&FORMAT_VERSION.to_le_bytes())?;
    file.write_all(&[pipeline.compression.as_u8()])?;
    file.write_all(&[pipeline.cipher.as_u8()])?;
    file.write_all(&[pipeline.fec.as_u8()])?;
    file.write_all(&0u8.to_le_bytes())?; // flags, зарезервировано
    file.write_all(&entry_count.to_le_bytes())?;
    file.write_all(&0u16.to_le_bytes())?; // reserved
    Ok(())
}


pub fn write_entry(file: &mut File, entry: &ArchiveEntry) -> Result<(), AppError> {
    let path_bytes = entry.relative_path.as_bytes();
    if path_bytes.len() > u16::MAX as usize {
        return Err(AppError::CorruptArchive(format!(
            "путь слишком длинный: {} байт",
            path_bytes.len()
        )));
    }

    file.write_all(&(path_bytes.len() as u16).to_le_bytes())?;
    file.write_all(path_bytes)?;
    file.write_all(&entry.original_size.to_le_bytes())?;
    file.write_all(&entry.stored_size.to_le_bytes())?;
    file.write_all(&entry.payload_offset.to_le_bytes())?;
    file.write_all(&[entry.entry_flags()])?;
    file.write_all(&entry.crc32.to_le_bytes())?;
    Ok(())
}



pub fn entry_size_bytes(relative_path: &str) -> u64 {
    2 + relative_path.as_bytes().len() as u64 + 8 + 8 + 8 + 1 + 4
}


fn read_exact_vec(file: &mut File, len: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}


pub fn read_header_and_entries(file: &mut File) -> Result<(PipelineSettings, Vec<ArchiveEntry>), AppError> {
    file.seek(SeekFrom::Start(0))?;

    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Err(AppError::CorruptArchive(
            "Неверная сигнатура файла - это не архив данного формата".to_string(),
        ));
    }

    let mut u16_buf = [0u8; 2];
    file.read_exact(&mut u16_buf)?;
    let version = u16::from_le_bytes(u16_buf);
    if version != FORMAT_VERSION {
        return Err(AppError::CorruptArchive(format!(
            "Неподдерживаемая версия формата: {version} (ожидалась {FORMAT_VERSION})"
        )));
    }

    let mut u8_buf = [0u8; 1];
    file.read_exact(&mut u8_buf)?;
    let compression = CompressionId::from_u8(u8_buf[0])?;
    file.read_exact(&mut u8_buf)?;
    let cipher = CipherId::from_u8(u8_buf[0])?;
    file.read_exact(&mut u8_buf)?;
    let fec = FecId::from_u8(u8_buf[0])?;
    file.read_exact(&mut u8_buf)?; // flags, пока игнорируем

    let mut u32_buf = [0u8; 4];
    file.read_exact(&mut u32_buf)?;
    let entry_count = u32::from_le_bytes(u32_buf);

    file.read_exact(&mut u16_buf)?; // reserved

    let pipeline = PipelineSettings { compression, cipher, fec };

    let mut entries = Vec::with_capacity(entry_count as usize);
    for _ in 0..entry_count {
        entries.push(read_one_entry(file)?);
    }

    Ok((pipeline, entries))
}


fn read_one_entry(file: &mut File) -> Result<ArchiveEntry, AppError> {
    let mut u16_buf = [0u8; 2];
    file.read_exact(&mut u16_buf)?;
    let path_len = u16::from_le_bytes(u16_buf) as usize;

    let path_bytes = read_exact_vec(file, path_len)?;
    let relative_path = String::from_utf8(path_bytes)
        .map_err(|_| AppError::CorruptArchive("Путь записи не является валидным UTF-8".to_string()))?;

    let mut u64_buf = [0u8; 8];
    file.read_exact(&mut u64_buf)?;
    let original_size = u64::from_le_bytes(u64_buf);

    file.read_exact(&mut u64_buf)?;
    let stored_size = u64::from_le_bytes(u64_buf);

    file.read_exact(&mut u64_buf)?;
    let payload_offset = u64::from_le_bytes(u64_buf);

    let mut u8_buf = [0u8; 1];
    file.read_exact(&mut u8_buf)?;
    let entry_flags = u8_buf[0];
    let is_directory = entry_flags & ENTRY_FLAG_IS_DIRECTORY != 0;

    let mut u32_buf = [0u8; 4];
    file.read_exact(&mut u32_buf)?;
    let crc32 = u32::from_le_bytes(u32_buf);

    Ok(ArchiveEntry {
        relative_path,
        original_size,
        stored_size,
        payload_offset,
        is_directory,
        crc32,
    })
}
