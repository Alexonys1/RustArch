use std::fs::File;
use std::io::Write;
use std::path::Path;

use crate::algorithms::PipelineSettings;
use crate::archiver::{entry_size_bytes, write_header, write_entry, ArchiveEntry, ArtifactAfterPipeline};
use crate::error::AppError;


/// Собирает итоговый файл архива: заголовок с общим pack_pipeline, таблицу
/// записей (пустые директории + файлы, отсортированные по relative_path
/// для детерминированности вне зависимости от порядка потоков) и,
/// наконец, сами payload'ы, дописанные подряд в конец файла.
pub fn assemble_archive(
    target_archive_path: &Path,
    pipeline: PipelineSettings,
    mut processed_files: Vec<ArtifactAfterPipeline>,
    mut empty_dirs: Vec<String>,
) -> Result<(), AppError>
{
    processed_files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    empty_dirs.sort();

    let dir_entries: Vec<ArchiveEntry> = empty_dirs
        .into_iter()
        .map(|relative_path| ArchiveEntry {
            relative_path,
            original_size: 0,
            stored_size: 0,
            payload_offset: 0, // у директорий нет payload'а
            is_directory: true,
            crc32: 0,
        })
        .collect();

    let entry_count = (dir_entries.len() + processed_files.len()) as u32;

    const HEADER_SIZE: u64 = 16;
    let table_size: u64 = dir_entries.iter().map(|e| entry_size_bytes(&e.relative_path)).sum::<u64>()
        + processed_files.iter().map(|p| entry_size_bytes(&p.relative_path)).sum::<u64>();

    let mut running_offset = HEADER_SIZE + table_size;
    let mut file_entries: Vec<ArchiveEntry> = Vec::with_capacity(processed_files.len());
    for processed in &processed_files {
        let payload_offset = running_offset;
        running_offset += processed.size_after_pipeline;

        file_entries.push(ArchiveEntry {
            relative_path: processed.relative_path.clone(),
            original_size: processed.original_size,
            stored_size: processed.size_after_pipeline,
            payload_offset,
            is_directory: false,
            crc32: processed.crc32,
        });
    }

    let mut archive_file = File::create(target_archive_path)?;
    write_header(&mut archive_file, entry_count, pipeline)?;

    for entry in &dir_entries {
        write_entry(&mut archive_file, entry)?;
    }
    for entry in &file_entries {
        write_entry(&mut archive_file, entry)?;
    }

    // Дописываем payload'ы строго в том же порядке, в котором были
    // рассчитаны их payload_offset - иначе таблица разъедется с данными.
    for processed in &mut processed_files {
        processed.payload.rewind_reading();
        processed.payload.copy_into(&mut archive_file)?;
    }

    archive_file.flush()?;
    Ok(())
}
