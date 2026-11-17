use std::fs::{File, OpenOptions};
use std::io::{Read, Write, Seek, SeekFrom};
use std::path::Path;

use crate::algorithms::fec::ReedSolomonCode;
use crate::algorithms::{ErrorCorrectionCode, PipelineSettings};
use crate::error::AppError;
use crate::archiver::{ArchiveError, Artifact};


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


const MAGIC: [u8; 4] = *b"RARC";
const FORMAT_VERSION: u16 = 4;
const FOOTER_DATA_SIZE: u64 = 32;
const FOOTER_SIZE: u64 = 64; // Данные 32 байта + Добавочные данные Рида-Соломона 32 байта


#[derive(Debug, Clone, Copy)]
struct ArchiveFooter {
    table_start: u64,
    table_size: u64,
    file_count: u32,
    directory_count: u32,
}


pub fn write_archive_header(
    target_archive_path: &Path,
    archived_files: Vec<ArchivedArtifactEntry>,
    archived_directories: Vec<ArchivedDirectoryEntry>,
) -> Result<(), AppError>
{
    let mut archive_file: File = OpenOptions::new()
        .read(true)
        .write(true)
        .open(target_archive_path)?;

    let table_start: u64 = archive_file.metadata()?.len(); // Начало таблицы там, где конец данных. Данные уже записаны в определённом порядке.
    archive_file.seek(SeekFrom::Start(table_start))?;

    let mut table: Artifact = Artifact::new_small();

    // Запишем сначала файлы, а потом пустые папки:
    for entry in archived_files.iter() {
        entry.write_to(&mut table)?;
    }

    for entry in archived_directories.iter() {
        entry.write_to(&mut table)?;
    }

    table.rewind_cursor();
    let mut protected_table: Artifact = ReedSolomonCode.encode(table)?;
    protected_table.rewind_cursor();
    let table_size: u64 = protected_table.get_payload_size() as u64;

    let file_count: u32 = u32::try_from(archived_files.len())
        .map_err(|_| AppError::from(ArchiveError::TooManyFiles { count: archived_files.len() }))?;
    let directory_count: u32 = u32::try_from(archived_directories.len())
        .map_err(|_| AppError::from(ArchiveError::TooManyDirectories { count: archived_directories.len() }))?;

    let mut footer: Artifact = Artifact::new_small();
    ArchiveFooter {
        table_start,
        table_size,
        file_count,
        directory_count,
    }.write_to(&mut footer)?;

    footer.rewind_cursor();
    let mut protected_footer: Artifact = ReedSolomonCode.encode(footer)?;
    protected_footer.rewind_cursor();

    protected_table.write_to_file_end(&mut archive_file)?;
    protected_footer.write_to_file_end(&mut archive_file)?;

    archive_file.sync_all()?;

    Ok(())
}


pub fn read_archive_header(archive_path: &Path) -> Result<(Vec<ArchivedArtifactEntry>, Vec<ArchivedDirectoryEntry>), AppError> {
    let archive_file = File::open(archive_path)?;

    let start_of_protected_footer = archive_file
        .metadata()?
        .len()
        .checked_sub(FOOTER_SIZE)
        .ok_or_else(|| AppError::from(ArchiveError::ArchiveTooShort))?;

    let protected_footer: Artifact = Artifact::from_file_range(archive_path, start_of_protected_footer, FOOTER_SIZE)?;

    let (mut footer, _): (Artifact, _) = ReedSolomonCode.decode(protected_footer)?;
    footer.rewind_cursor();

    let footer: ArchiveFooter = ArchiveFooter::from_artifact_footer(footer)?;

    if footer.table_end()? != start_of_protected_footer {
        return Err(ArchiveError::InvalidTableBounds.into());
    }

    let protected_table: Artifact = Artifact::from_file_range(
        archive_path,
        footer.table_start,
        footer.table_size,
    )?;

    let (mut table, _): (Artifact, _) = ReedSolomonCode.decode(protected_table)?;
    table.rewind_cursor();

    let table_end = table.get_payload_size() as u64;
    let file_capacity = footer.file_count as usize;
    let directory_capacity = footer.directory_count as usize;

    let mut files: Vec<ArchivedArtifactEntry> = Vec::with_capacity(file_capacity);
    for _ in 0..footer.file_count {
        files.push(ArchivedArtifactEntry::read_from(&mut table)?);
        ensure_inside_table(&mut table, table_end)?;
    }

    let mut directories: Vec<ArchivedDirectoryEntry> = Vec::with_capacity(directory_capacity);
    for _ in 0..footer.directory_count {
        directories.push(ArchivedDirectoryEntry::read_from(&mut table)?);
        ensure_inside_table(&mut table, table_end)?;
    }

    if table.stream_position()? != table_end {
        return Err(ArchiveError::TableSizeMismatch.into());
    }

    validate_payload_ranges(&files, footer.table_start)?;

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
    pub fn write_to<Writable: Write + ?Sized>(self, file: &mut Writable) -> Result<(), AppError> {
        file.write_all(&MAGIC)?;
        file.write_all(&FORMAT_VERSION.to_le_bytes())?;
        file.write_all(&0u16.to_le_bytes())?;
        file.write_all(&self.table_start.to_le_bytes())?;
        file.write_all(&self.table_size.to_le_bytes())?;
        file.write_all(&self.file_count.to_le_bytes())?;
        file.write_all(&self.directory_count.to_le_bytes())?;
        Ok(())
    }

    fn from_artifact_footer(mut artifact_footer: Artifact) -> Result<Self, AppError> {
        let footer_size: u64 = artifact_footer.get_payload_size() as u64;

        if footer_size != FOOTER_DATA_SIZE {
            return Err(ArchiveError::InvalidTableSize.into());
        }

        let mut magic: [u8; 4] = [0; _];
        artifact_footer.read_exact(&mut magic)?;

        if magic != MAGIC {
            return Err(ArchiveError::InvalidSignature.into());
        }

        let version = read_u16(&mut artifact_footer)?;
        if version != FORMAT_VERSION {
            return Err(AppError::from(ArchiveError::UnsupportedVersion { version, expected: FORMAT_VERSION }));
        }

        let flags = read_u16(&mut artifact_footer)?;
        if flags != 0 {
            return Err(AppError::from(ArchiveError::UnknownArchiveFlags { flags }));
        }

        let footer = Self {
            table_start: read_u64(&mut artifact_footer)?,
            table_size: read_u64(&mut artifact_footer)?,
            file_count: read_u32(&mut artifact_footer)?,
            directory_count: read_u32(&mut artifact_footer)?,
        };

        Ok(footer)
    }

    fn table_end(self) -> Result<u64, AppError> {
        self.table_start
            .checked_add(self.table_size)
            .ok_or_else(|| AppError::from(ArchiveError::TableSizeOverflow))
    }
}


fn write_path<Writer: Write>(writer: &mut Writer, path: &str) -> Result<(), AppError> {
    let bytes = path.as_bytes();
    let length = u16::try_from(bytes.len()).map_err(|_| {
        AppError::from(ArchiveError::PathTooLong { length: bytes.len() })
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
        .map_err(|source| ArchiveError::InvalidPathUtf8(source).into())
}


fn ensure_inside_table<Seekable: Seek>(dest: &mut Seekable, table_end: u64) -> Result<(), AppError> {
    if dest.stream_position()? > table_end {
        return Err(AppError::from(ArchiveError::EntryOutsideTable));
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
                AppError::from(ArchiveError::PayloadRangeOverflow { path: entry.relative_path.clone() })
            })?;

        if end > payload_end {
            return Err(AppError::from(ArchiveError::PayloadOutsideSection { path: entry.relative_path.clone() }));
        }

        if entry.stored_size != 0 {
            ranges.push((entry.payload_offset, end));
        }
    }

    ranges.sort_unstable();
    let mut expected_offset = 0;
    for (start, end) in ranges {
        if start != expected_offset {
            return Err(AppError::from(ArchiveError::InvalidPayloadLayout));
        }
        expected_offset = end;
    }

    if expected_offset != payload_end {
        return Err(AppError::from(ArchiveError::PayloadEndMismatch));
    }

    Ok(())
}


pub fn validate_empty_artifact_entry(entry: &ArchivedArtifactEntry) -> Result<(), AppError> {
    if entry.original_size != 0 {
        return Ok(());
    }

    if entry.stored_size != 0 {
        return Err(AppError::from(ArchiveError::EmptyFileHasPayload { path: entry.relative_path.clone() }));
    }

    if entry.pipeline != PipelineSettings::default() {
        return Err(AppError::from(ArchiveError::EmptyFileHasPipeline { path: entry.relative_path.clone() }));
    }

    if entry.crc32 != u32::MAX {
        return Err(AppError::from(ArchiveError::InvalidEmptyFileChecksum { path: entry.relative_path.clone() }));
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
