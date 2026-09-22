use std::fs::{File, OpenOptions};
use std::io::{Read, Write, Seek, SeekFrom};
use std::path::Path;

use crate::algorithms::PipelineSettings;
use crate::error::AppError;


pub const MAGIC: [u8; 4] = *b"RARC";
pub const FORMAT_VERSION: u16 = 3;
pub const FOOTER_SIZE: u64 = 32;


#[derive(Debug, Clone, Copy)]
pub struct ArchiveFooter {
    pub table_offset: u64,
    pub table_size: u64,
    pub file_count: u32,
    pub directory_count: u32,
}

#[derive(Debug, Clone)]
pub struct ArchivedArtifactEntry {
    pub relative_path: String,
    pub original_size: u64,
    pub stored_size: u64,
    pub payload_offset: u64,
    pub crc32: u32,
    pub pipeline: PipelineSettings,
}

#[derive(Debug, Clone)]
pub struct ArchivedDirectoryEntry {
    pub relative_path: String,
}


pub fn write_archive_header(
    target_archive_path: &Path,
    archived_files: Vec<ArchivedArtifactEntry>,
    archived_directories: Vec<ArchivedDirectoryEntry>,
) -> Result<(), AppError>
{
    let mut archive_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(target_archive_path)?;
    let table_offset = archive_file.metadata()?.len();

    validate_payload_ranges(&archived_files, table_offset)?;
    archive_file.seek(SeekFrom::Start(table_offset))?;

    // Запишем сначала файлы, а потом пустые папки:
    for entry in archived_files.iter() {
        entry.write_to(&mut archive_file)?;
    }
    
    for entry in archived_directories.iter() {
        entry.write_to(&mut archive_file)?;
    }

    let table_end: u64 = archive_file.stream_position()?;
    let table_size: u64 = table_end
        .checked_sub(table_offset)
        .ok_or_else(|| AppError::CorruptArchive("Некорректный размер таблиц".into()))?;
    let file_count: u32 = u32::try_from(archived_files.len())
        .map_err(|_| AppError::CorruptArchive("Количество файлов превышает u32::MAX".into()))?;
    let directory_count: u32 = u32::try_from(archived_directories.len())
        .map_err(|_| AppError::CorruptArchive("Количество директорий превышает u32::MAX".into()))?;

    ArchiveFooter {
        table_offset,
        table_size,
        file_count,
        directory_count,
    }.write_to(&mut archive_file)?;
    
    archive_file.sync_all()?;
    
    Ok(())
}


pub fn read_archive_entries(file: &mut File) -> Result<(Vec<ArchivedArtifactEntry>, Vec<ArchivedDirectoryEntry>), AppError> {
    let footer = ArchiveFooter::read_from(file)?;
    let table_end: u64 = footer.table_end()?;

    file.seek(SeekFrom::Start(footer.table_offset))?;

    let file_capacity: usize = usize::try_from(footer.file_count)
        .map_err(|_| AppError::CorruptArchive("Слишком много файловых записей".into()))?;
    let directory_capacity = usize::try_from(footer.directory_count)
        .map_err(|_| AppError::CorruptArchive("Слишком много записей директорий".into()))?;

    let mut files: Vec<ArchivedArtifactEntry> = Vec::with_capacity(file_capacity);
    for _ in 0..footer.file_count {
        files.push(ArchivedArtifactEntry::read_from(file)?);
        ensure_inside_table(file, table_end)?;
    }

    let mut directories: Vec<ArchivedDirectoryEntry> = Vec::with_capacity(directory_capacity);
    for _ in 0..footer.directory_count {
        directories.push(ArchivedDirectoryEntry::read_from(file)?);
        ensure_inside_table(file, table_end)?;
    }

    if file.stream_position()? != table_end {
        return Err(AppError::CorruptArchive(
            "Размер таблиц не совпадает с количеством записей!".into(),
        ));
    }

    validate_payload_ranges(&files, footer.table_offset)?;
    
    Ok((files, directories))
}


impl ArchivedArtifactEntry {
    pub fn write_to<Writer: Write>(&self, writer: &mut Writer) -> Result<(), AppError> {
        write_path(writer, &self.relative_path)?;
        writer.write_all(&self.original_size.to_le_bytes())?;
        writer.write_all(&self.stored_size.to_le_bytes())?;
        writer.write_all(&self.payload_offset.to_le_bytes())?;
        writer.write_all(&self.crc32.to_le_bytes())?;
        writer.write_all(&self.pipeline.to_descriptor().to_le_bytes())?;
        Ok(())
    }

    pub fn read_from<Reader: Read>(reader: &mut Reader) -> Result<Self, AppError> {
        Ok(Self {
            relative_path: read_path(reader)?,
            original_size: read_u64(reader)?,
            stored_size: read_u64(reader)?,
            payload_offset: read_u64(reader)?,
            crc32: read_u32(reader)?,
            pipeline: PipelineSettings::from_descriptor(read_u16(reader)?)?,
        })
    }

    pub fn serialized_size(&self) -> u64 {
        2 + self.relative_path.len() as u64 + 8 + 8 + 8 + 4 + 2
    }
}


impl ArchivedDirectoryEntry {
    pub fn new(relative_path: String) -> Self {
        Self { relative_path }
    }

    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<(), AppError> {
        write_path(writer, &self.relative_path)
    }

    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self, AppError> {
        Ok(Self {
            relative_path: read_path(reader)?,
        })
    }

    pub fn serialized_size(&self) -> u64 {
        2 + self.relative_path.len() as u64
    }
}


impl ArchiveFooter {
    pub fn write_to(self, file: &mut File) -> Result<(), AppError> {
        file.write_all(&MAGIC)?;
        file.write_all(&FORMAT_VERSION.to_le_bytes())?;
        file.write_all(&0u16.to_le_bytes())?;
        file.write_all(&self.table_offset.to_le_bytes())?;
        file.write_all(&self.table_size.to_le_bytes())?;
        file.write_all(&self.file_count.to_le_bytes())?;
        file.write_all(&self.directory_count.to_le_bytes())?;
        Ok(())
    }

    fn read_from(file: &mut File) -> Result<Self, AppError> {
        let archive_size = file.metadata()?.len();
        if archive_size < FOOTER_SIZE {
            return Err(AppError::CorruptArchive(
                "Архив слишком короткий для footer".into(),
            ));
        }

        file.seek(SeekFrom::Start(archive_size - FOOTER_SIZE))?;

        let mut magic = [0u8; 4];
        file.read_exact(&mut magic)?;
        if magic != MAGIC {
            return Err(AppError::CorruptArchive(
                "Неверная сигнатура footer архива".into(),
            ));
        }

        let version = read_u16(file)?;
        if version != FORMAT_VERSION {
            return Err(AppError::CorruptArchive(format!(
                "Неподдерживаемая версия формата: {version} (ожидалась {FORMAT_VERSION})"
            )));
        }

        let flags = read_u16(file)?;
        if flags != 0 {
            return Err(AppError::CorruptArchive(format!(
                "Неизвестные флаги архива: {flags:#x}"
            )));
        }

        let footer = Self {
            table_offset: read_u64(file)?,
            table_size: read_u64(file)?,
            file_count: read_u32(file)?,
            directory_count: read_u32(file)?,
        };

        if footer.table_end()? != archive_size - FOOTER_SIZE {
            return Err(AppError::CorruptArchive(
                "Некорректные границы таблиц архива".into(),
            ));
        }

        Ok(footer)
    }

    fn table_end(self) -> Result<u64, AppError> {
        self.table_offset
            .checked_add(self.table_size)
            .ok_or_else(|| AppError::CorruptArchive("Переполнение размера таблиц".into()))
    }
}


fn write_path<Writer: Write>(writer: &mut Writer, path: &str) -> Result<(), AppError> {
    let bytes = path.as_bytes();
    let length = u16::try_from(bytes.len()).map_err(|_| {
        AppError::CorruptArchive(format!("Путь слишком длинный: {} байт", bytes.len()))
    })?;

    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(bytes)?;
    Ok(())
}


fn read_path<Reader: Read>(reader: &mut Reader) -> Result<String, AppError> {
    let length = read_u16(reader)? as usize;
    let mut bytes = vec![0u8; length];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes)
        .map_err(|_| AppError::CorruptArchive("Путь записи не является валидным UTF-8".into()))
}


fn ensure_inside_table(file: &mut File, table_end: u64) -> Result<(), AppError> {
    if file.stream_position()? > table_end {
        return Err(AppError::CorruptArchive(
            "Запись выходит за пределы своей таблицы".into(),
        ));
    }
    Ok(())
}


pub fn validate_payload_ranges(files: &[ArchivedArtifactEntry], payload_end: u64) -> Result<(), AppError> {
    let mut ranges: Vec<(u64, u64)> = Vec::with_capacity(files.len());

    for entry in files {
        validate_empty_artifact_entry(entry)?;

        let end = entry
            .payload_offset
            .checked_add(entry.stored_size)
            .ok_or_else(|| {
                AppError::CorruptArchive(format!(
                    "Переполнение payload диапазона '{}'",
                    entry.relative_path
                ))
            })?;

        if end > payload_end {
            return Err(AppError::CorruptArchive(format!(
                "Payload '{}' выходит за пределы payload-секции",
                entry.relative_path
            )));
        }

        if entry.stored_size != 0 {
            ranges.push((entry.payload_offset, end));
        }
    }

    ranges.sort_unstable();
    let mut expected_offset = 0;
    for (start, end) in ranges {
        if start != expected_offset {
            return Err(AppError::CorruptArchive(
                "Payload'ы не образуют непрерывную непересекающуюся секцию".into(),
            ));
        }
        expected_offset = end;
    }

    if expected_offset != payload_end {
        return Err(AppError::CorruptArchive(
            "Конец payload-секции не совпадает с началом таблиц".into(),
        ));
    }

    Ok(())
}


pub fn validate_empty_artifact_entry(entry: &ArchivedArtifactEntry) -> Result<(), AppError> {
    if entry.original_size != 0 {
        return Ok(());
    }

    if entry.stored_size != 0 {
        return Err(AppError::CorruptArchive(format!(
            "Пустой файл '{}' содержит ненулевой payload",
            entry.relative_path
        )));
    }

    if entry.pipeline != PipelineSettings::default() {
        return Err(AppError::CorruptArchive(format!(
            "Пустой файл '{}' содержит ненулевой pipeline",
            entry.relative_path
        )));
    }

    if entry.crc32 != u32::MAX {
        return Err(AppError::CorruptArchive(format!(
            "Пустой файл '{}' содержит некорректную CRC32-заглушку",
            entry.relative_path
        )));
    }

    Ok(())
}


fn read_u16<Reader: Read>(reader: &mut Reader) -> Result<u16, AppError> {
    let mut bytes = [0u8; 2];
    reader.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}


fn read_u32<Reader: Read>(reader: &mut Reader) -> Result<u32, AppError> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}


fn read_u64<Reader: Read>(reader: &mut Reader) -> Result<u64, AppError> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}
