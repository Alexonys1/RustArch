use std::path::Path;
use std::thread;

use crate::error::AppError;
use crate::algorithms::PipelineSettings;
use crate::archiver::ArchiveEntry;
use super::one_file::unpack_file;


const MAX_PARALLELISM: usize = usize::MAX;


pub fn unpack_entries_parallel(entries: Vec<ArchiveEntry>, archive_path: &Path, output_dir: &Path, pipeline_settings: PipelineSettings, decode_key: &[u8]) -> Result<(), AppError> {
    let groups_of_entries: Vec<&[ArchiveEntry]> = group_entries_for_workers(&entries);

    thread::scope(|scope| {
        let mut workers = Vec::with_capacity(groups_of_entries.len());

        for &entry_group in groups_of_entries.iter() {
            workers.push(scope.spawn(||
                handle_entry_group(entry_group, archive_path, output_dir, pipeline_settings, decode_key)
            ));
        }

        for worker in workers.into_iter() {
            worker
                .join()
                .map_err(|_| AppError::Compression("Паника в рабочем потоке".into()))??;
        }

        Ok(())
    })
}


fn handle_entry_group(entry_group: &[ArchiveEntry], archive_path: &Path, output_dir: &Path, pipeline_settings: PipelineSettings, decode_key: &[u8]) -> Result<(), AppError> {
    for entry in entry_group {
        unpack_file(entry, archive_path, output_dir, pipeline_settings, decode_key)?; // !!РАСПАКОВЫВАЕМ ЗДЕСЬ!!
    }

    Ok(())
}


fn group_entries_for_workers(entries: &[ArchiveEntry]) -> Vec<&[ArchiveEntry]> {
    if entries.is_empty() {
        return Vec::new();
    }

    let number_of_workers = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(entries.len())
        .min(MAX_PARALLELISM);

    let chunk_len = (entries.len() + number_of_workers - 1) / number_of_workers;

    entries.chunks(chunk_len).collect()
}
