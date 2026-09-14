use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::thread;

use super::one_file::pack_file;
use crate::algorithms::PipelineSettings;
use crate::archiver::{ArtifactAfterPipeline, WalkedFile};
use crate::error::AppError;


const MAX_PARALLELISM: usize = usize::MAX;


pub fn pack_files_parallel(files: Vec<WalkedFile>, pipeline_settings: PipelineSettings, encode_key: &[u8]) -> Result<Vec<ArtifactAfterPipeline>, AppError> {
    let groups_of_files: Vec<Vec<&WalkedFile>> = group_files_for_workers(&files)?;

    thread::scope(|scope| {
        let mut workers = Vec::with_capacity(groups_of_files.len());

        for file_group in groups_of_files.iter() {
            workers.push(scope.spawn(||
                handle_file_group(file_group, pipeline_settings, encode_key)
            ));
        }

        let mut result_artifacts: Vec<ArtifactAfterPipeline> = Vec::with_capacity(files.len());

        for worker in workers.into_iter() {
            let thread_result = worker
                .join()
                .map_err(|_| AppError::Compression("Паника в рабочем потоке".into()))?;

            result_artifacts.extend(thread_result?);
        }

        Ok(result_artifacts)
    })
}


fn handle_file_group(file_group: &[&WalkedFile], pipeline_settings: PipelineSettings, encode_key: &[u8]) -> Result<Vec<ArtifactAfterPipeline>, AppError> {
    let mut result_artifacts: Vec<ArtifactAfterPipeline> = Vec::with_capacity(file_group.len());

    for &file in file_group {
        result_artifacts.push(
            pack_file(file, pipeline_settings, encode_key)? // !!АРХИВИРУЕМ ЗДЕСЬ!!
        );
    }

    Ok(result_artifacts)
}


/// Разбивает файлы на группы (по числу доступных потоков) так, чтобы
/// суммарный размер файлов в каждой группе был примерно одинаковым.
/// Внутри группы файлы отсортированы по возрастанию размера -
/// это позволяет каждому потоку быстрее закрыть маленькие файлы,
/// экономя при этом память при обработке больших файлов в конце.
fn group_files_for_workers(files: &[WalkedFile]) -> Result<Vec<Vec<&WalkedFile>>, AppError> {
    if files.is_empty() {
        return Ok(Vec::new());
    }

    let number_of_workers = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(files.len())
        .min(MAX_PARALLELISM);
    assert!(MAX_PARALLELISM != 0);

    let mut sized_files: Vec<(u64, &WalkedFile)> = Vec::with_capacity(files.len());
    for file in files {
        sized_files.push((file.get_size()?, file));
    }

    sized_files.sort_by_key(|&(size, _)| Reverse(size));

    let mut groups: Vec<Vec<(u64, &WalkedFile)>> = (0..number_of_workers).map(|_| Vec::new()).collect();

    let mut heap: BinaryHeap<Reverse<(u64, usize)>> =
        (0..number_of_workers).map(|i| Reverse((0u64, i))).collect();

    for (size, file) in sized_files {
        let Reverse((total_size, group_idx)) = heap.pop().unwrap();
        groups[group_idx].push((size, file));
        heap.push(Reverse((total_size + size, group_idx)));
    }

    for group in groups.iter_mut() {
        group.sort_by_key(|&(size, _)| Reverse(size));
    }

    let mut result = Vec::with_capacity(groups.len());
    for group in groups {
        let files_only: Vec<&WalkedFile> = group
            .into_iter()
            .map(|(_size, file)| file)
            .collect();
        result.push(files_only);
    }

    Ok(result)
}
