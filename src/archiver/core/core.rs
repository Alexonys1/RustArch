use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom};
use std::path::Path;

use crate::archiver::{read_entries_in_range, write_entry, write_footer, ArchiveEntry, ArchiveFooter, ArchivedArtifactEntry};
use crate::error::AppError;

/// Записывает записи пустых директорий в таблицу, которая находится после
/// всех payload'ов. Финальный footer будет записан write_archive_header().
pub fn write_empty_dirs(
    target_archive_path: &Path,
    mut target_empty_dirs: Vec<String>,
) -> Result<(), AppError> {
    target_empty_dirs.sort();
    target_empty_dirs.dedup();

    let mut archive_file = OpenOptions::new()
        .append(true)
        .open(target_archive_path)?;

    for relative_path in target_empty_dirs {
        let entry = ArchiveEntry {
            relative_path,
            original_size: 0,
            stored_size: 0,
            payload_offset: 0,
            is_directory: true,
            crc32: 0,
            pipeline: Default::default(),
        };
        write_entry(&mut archive_file, &entry)?;
    }

    Ok(())
}

/// Завершает архив: добавляет записи файлов и фиксированный footer в конец.
/// Payload'ы не перемещаются — их реальные offsets были назначены writer-потоком.
pub fn write_archive_header(
    target_archive_path: &Path,
    mut archived_files: Vec<ArchivedArtifactEntry>,
) -> Result<(), AppError>
{
    let payload_end = archived_files.iter().try_fold(0u64, |sum, entry| {
        sum.checked_add(entry.size_after_pipeline)
            .ok_or_else(|| AppError::CorruptArchive("Переполнение общего размера payload'ов".into()))
    })?;

    // Детерминированный порядок таблицы не обязан совпадать с порядком записи
    // payload'ов: payload_offset уже содержит фактическое положение данных.
    archived_files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    let mut archive_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(target_archive_path)?;

    let file_len = archive_file.metadata()?.len();
    if file_len < payload_end {
        return Err(AppError::CorruptArchive(format!(
            "Размер архива ({file_len}) меньше ожидаемого конца payload-секции ({payload_end})"
        )));
    }

    // До добавления файлов здесь находятся только записи empty-dir.
    archive_file.seek(SeekFrom::Start(payload_end))?;
    let directory_entries = read_entries_in_range(&mut archive_file, payload_end, file_len)?;
    if directory_entries.iter().any(|entry| !entry.is_directory) {
        return Err(AppError::CorruptArchive(
            "В секции пустых директорий обнаружена файловая запись".into(),
        ));
    }

    for entry in &directory_entries {
        if entry.original_size != 0 || entry.stored_size != 0 || entry.payload_offset != 0 {
            return Err(AppError::CorruptArchive(format!(
                "Некорректная запись директории '{}'",
                entry.relative_path
            )));
        }
    }

    let mut ranges: Vec<(u64, u64, String)> =
        Vec::with_capacity(archived_files.len());

    for entry in &archived_files {
        let start = entry.payload_offset;

        let end = start.checked_add(entry.size_after_pipeline)
            .ok_or_else(|| AppError::CorruptArchive(format!(
                "Переполнение payload диапазона '{}'",
                entry.relative_path
            )))?;

        if end > payload_end {
            return Err(AppError::CorruptArchive(format!(
                "Payload '{}' выходит за границы payload-секции",
                entry.relative_path
            )));
        }

        // Пустой файл не занимает места и не должен участвовать
        // в проверке непрерывного покрытия payload-секции.
        if start == end {
            continue;
        }

        ranges.push((start, end, entry.relative_path.clone()));
    }

    ranges.sort_unstable_by_key(|&(start, _, _)| start);

    let mut expected = 0u64;

    for (start, end, path) in ranges {
        if start != expected {
            println!(
                "start={}, end={}, expected={}, path={}",
                start, end, expected, path
            );

            return Err(AppError::CorruptArchive(
                "Payload'ы не образуют непрерывную секцию архива".into(),
            ));
        }

        expected = end;
    }

    if expected != payload_end {
        return Err(AppError::CorruptArchive(
            "Фактическая сумма payload'ов не совпадает с их offsets".into(),
        ));
    }

    archive_file.seek(SeekFrom::End(0))?;
    for processed in &archived_files {
        let entry = ArchiveEntry {
            relative_path: processed.relative_path.clone(),
            original_size: processed.original_size,
            stored_size: processed.size_after_pipeline,
            payload_offset: processed.payload_offset,
            is_directory: false,
            crc32: processed.crc32,
            pipeline: processed.pipeline,
        };
        write_entry(&mut archive_file, &entry)?;
    }

    let table_offset = payload_end;
    let table_end = archive_file.stream_position()?;
    let table_size = table_end.checked_sub(table_offset)
        .ok_or_else(|| AppError::CorruptArchive("Некорректный размер таблицы".into()))?;
    let entry_count = directory_entries.len()
        .checked_add(archived_files.len())
        .ok_or_else(|| AppError::CorruptArchive("Слишком много записей в архиве".into()))?;
    let entry_count = u32::try_from(entry_count)
        .map_err(|_| AppError::CorruptArchive("Количество записей превышает u32::MAX".into()))?;

    write_footer(&mut archive_file, ArchiveFooter {
        table_offset,
        entry_count,
        table_size,
    })?;
    archive_file.sync_all()?;

    Ok(())
}
