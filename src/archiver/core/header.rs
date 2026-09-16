//! Бинарный формат архива RustArch.
//!
//! Новый формат не требует перемещения payload'ов после их записи.
//!
//! ```text
//! [0 .. payload_end)       payload #1 | payload #2 | ... | payload #N
//! [payload_end .. table)  таблица ArchiveEntry
//! [table .. EOF)           фиксированный footer
//! ```
//!
//! Все числа little-endian.

use std::fs::File;
use std::io::{Read, Seek, Write, SeekFrom};

use crate::algorithms::{CipherId, CompressionId, FecId, PipelineSettings};
use crate::error::AppError;


pub const MAGIC: [u8; 4] = *b"RARC";
pub const FORMAT_VERSION: u16 = 2;
pub const ENTRY_FLAG_IS_DIRECTORY: u8 = 1 << 0;

// Footer: magic(4) + version(2) + flags(2) + table_offset(8) +
// entry_count(4) + table_size(8) + reserved(4) = 32 bytes.
pub const FOOTER_SIZE: u64 = 32;


#[derive(Debug, Clone)]
pub struct ArchiveEntry {
    pub relative_path: String,
    pub original_size: u64,
    pub stored_size: u64,
    pub payload_offset: u64,
    pub is_directory: bool,
    pub crc32: u32,
    pub pipeline: PipelineSettings,
}


impl ArchiveEntry {
    fn entry_flags(&self) -> u8 {
        if self.is_directory { ENTRY_FLAG_IS_DIRECTORY } else { 0 }
    }
}


#[derive(Debug, Clone, Copy)]
pub struct ArchiveFooter {
    pub table_offset: u64,
    pub entry_count: u32,
    pub table_size: u64,
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
    file.write_all(&[entry.pipeline.compression.as_u8()])?;
    file.write_all(&[entry.pipeline.cipher.as_u8()])?;
    file.write_all(&[entry.pipeline.fec.as_u8()])?;
    file.write_all(&0u8.to_le_bytes())?;
    Ok(())
}


pub fn entry_size_bytes(relative_path: &str) -> u64 {
    // path_len + path + original_size + stored_size + payload_offset +
    // flags + crc32 + compression + cipher + fec + reserved.
    2 + relative_path.as_bytes().len() as u64 + 8 + 8 + 8 + 1 + 4 + 1 + 1 + 1 + 1
}


fn read_exact_vec(file: &mut File, len: usize) -> Result<Vec<u8>, AppError> {
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}


fn read_one_entry(file: &mut File) -> Result<ArchiveEntry, AppError> {
    let mut u16_buf = [0u8; 2];
    file.read_exact(&mut u16_buf)?;
    let path_len = u16::from_le_bytes(u16_buf) as usize;

    let path_bytes = read_exact_vec(file, path_len)?;
    let relative_path = String::from_utf8(path_bytes)
        .map_err(|_| AppError::CorruptArchive("Путь записи не является валидным UTF-8".into()))?;

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
    if entry_flags & !ENTRY_FLAG_IS_DIRECTORY != 0 {
        return Err(AppError::CorruptArchive(format!(
            "Неизвестные флаги записи '{}': {entry_flags:#x}",
            relative_path
        )));
    }
    let is_directory = entry_flags & ENTRY_FLAG_IS_DIRECTORY != 0;

    let mut u32_buf = [0u8; 4];
    file.read_exact(&mut u32_buf)?;
    let crc32 = u32::from_le_bytes(u32_buf);

    file.read_exact(&mut u8_buf)?;
    let compression = CompressionId::from_u8(u8_buf[0])?;
    file.read_exact(&mut u8_buf)?;
    let cipher = CipherId::from_u8(u8_buf[0])?;
    file.read_exact(&mut u8_buf)?;
    let fec = FecId::from_u8(u8_buf[0])?;
    file.read_exact(&mut u8_buf)?; // reserved

    Ok(ArchiveEntry {
        relative_path,
        original_size,
        stored_size,
        payload_offset,
        is_directory,
        crc32,
        pipeline: PipelineSettings { compression, cipher, fec },
    })
}


pub fn read_footer(file: &mut File) -> Result<ArchiveFooter, AppError> {
    let file_len = file.metadata()?.len();
    if file_len < FOOTER_SIZE {
        return Err(AppError::CorruptArchive("Архив слишком короткий для footer".into()));
    }

    file.seek(SeekFrom::Start(file_len - FOOTER_SIZE))?;

    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Err(AppError::CorruptArchive("Неверная сигнатура footer архива".into()));
    }

    let mut u16_buf = [0u8; 2];
    file.read_exact(&mut u16_buf)?;
    let version = u16::from_le_bytes(u16_buf);
    if version != FORMAT_VERSION {
        return Err(AppError::CorruptArchive(format!(
            "Неподдерживаемая версия формата: {version} (ожидалась {FORMAT_VERSION})"
        )));
    }

    file.read_exact(&mut u16_buf)?; // flags

    let mut u64_buf = [0u8; 8];
    file.read_exact(&mut u64_buf)?;
    let table_offset = u64::from_le_bytes(u64_buf);

    let mut u32_buf = [0u8; 4];
    file.read_exact(&mut u32_buf)?;
    let entry_count = u32::from_le_bytes(u32_buf);

    file.read_exact(&mut u64_buf)?;
    let table_size = u64::from_le_bytes(u64_buf);

    let mut reserved = [0u8; 4];
    file.read_exact(&mut reserved)?;

    if table_offset.checked_add(table_size).is_none()
        || table_offset + table_size != file_len - FOOTER_SIZE
    {
        return Err(AppError::CorruptArchive("Некорректные границы таблицы архива".into()));
    }

    Ok(ArchiveFooter { table_offset, entry_count, table_size })
}


pub fn read_entries_in_range(
    file: &mut File,
    start: u64,
    end: u64,
) -> Result<Vec<ArchiveEntry>, AppError> {
    if end < start {
        return Err(AppError::CorruptArchive("Некорректный диапазон таблицы".into()));
    }

    file.seek(SeekFrom::Start(start))?;
    let mut entries = Vec::new();
    while file.stream_position()? < end {
        let before = file.stream_position()?;
        entries.push(read_one_entry(file)?);
        let after = file.stream_position()?;
        if after <= before || after > end {
            return Err(AppError::CorruptArchive("Повреждённая запись таблицы".into()));
        }
    }
    if file.stream_position()? != end {
        return Err(AppError::CorruptArchive("Таблица не выровнена по границе записи".into()));
    }
    Ok(entries)
}


pub fn read_header_and_entries(file: &mut File) -> Result<Vec<ArchiveEntry>, AppError> {
    let footer = read_footer(file)?;

    file.seek(SeekFrom::Start(footer.table_offset))?;
    let table_end = footer.table_offset + footer.table_size;
    let mut entries = Vec::with_capacity(footer.entry_count as usize);

    for _ in 0..footer.entry_count {
        let before = file.stream_position()?;
        entries.push(read_one_entry(file)?);
        let after = file.stream_position()?;
        if after > table_end {
            return Err(AppError::CorruptArchive("Запись выходит за пределы таблицы".into()));
        }
        if after <= before {
            return Err(AppError::CorruptArchive("Некорректный размер записи".into()));
        }
    }

    if file.stream_position()? != table_end {
        return Err(AppError::CorruptArchive("Размер таблицы не совпадает с её записями".into()));
    }

    let mut payload_ranges: Vec<(u64, u64)> = Vec::new();

    for entry in &entries {
        if entry.is_directory {
            if entry.stored_size != 0 || entry.payload_offset != 0 || entry.original_size != 0 {
                return Err(AppError::CorruptArchive(format!(
                    "Некорректная запись директории '{}'",
                    entry.relative_path
                )));
            }
        } else {
            let end = entry.payload_offset.checked_add(entry.stored_size)
                .ok_or_else(|| AppError::CorruptArchive(format!(
                    "Переполнение payload диапазона '{}'",
                    entry.relative_path
                )))?;
            if end > footer.table_offset {
                return Err(AppError::CorruptArchive(format!(
                    "Payload '{}' выходит за пределы payload-секции",
                    entry.relative_path
                )));
            }
            payload_ranges.push((entry.payload_offset, end));
        }
    }

    payload_ranges.sort_unstable_by_key(|&(start, end)| (start, end));
    let mut expected = 0u64;
    for (start, end) in payload_ranges {
        if start != expected {
            return Err(AppError::CorruptArchive(
                "Payload'ы не образуют непрерывную непересекающуюся секцию".into(),
            ));
        }
        expected = end;
    }
    if expected != footer.table_offset {
        return Err(AppError::CorruptArchive(
            "Конец payload-секции не совпадает с table_offset".into(),
        ));
    }

    Ok(entries)
}


pub fn write_footer(file: &mut File, footer: ArchiveFooter) -> Result<(), AppError> {
    file.write_all(&MAGIC)?;
    file.write_all(&FORMAT_VERSION.to_le_bytes())?;
    file.write_all(&0u16.to_le_bytes())?;
    file.write_all(&footer.table_offset.to_le_bytes())?;
    file.write_all(&footer.entry_count.to_le_bytes())?;
    file.write_all(&footer.table_size.to_le_bytes())?;
    file.write_all(&0u32.to_le_bytes())?;
    Ok(())
}
