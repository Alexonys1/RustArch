use std::thread;

use super::one_file::pack_file;
use crate::algorithms::PipelineSettings;
use crate::archiver::{ArtifactAfterPipeline, WalkedFile};
use crate::error::AppError;


// TODO: Написать разбиение файлов по группам с учётом размера файла,
// TODO: чтобы на каждый поток ложилась +- одинаковая нагрузка.
const MAX_PARALLELISM: usize = 8;


pub fn pack_files_parallel(files: Vec<WalkedFile>, pipeline_settings: PipelineSettings, encode_key: &[u8]) -> Result<Vec<ArtifactAfterPipeline>, AppError> {
    let groups_of_files: Vec<&[WalkedFile]> = group_files_for_workers(&files);

    thread::scope(|scope| {
        let mut workers = Vec::with_capacity(groups_of_files.len());

        for &file_group in groups_of_files.iter() {
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


fn handle_file_group(file_group: &[WalkedFile], pipeline_settings: PipelineSettings, encode_key: &[u8]) -> Result<Vec<ArtifactAfterPipeline>, AppError> {
    let mut result_artifacts: Vec<ArtifactAfterPipeline> = Vec::with_capacity(file_group.len());

    for file in file_group {
        result_artifacts.push(
            pack_file(file, pipeline_settings, encode_key)? // !!АРХИВИРУЕМ ЗДЕСЬ!!
        );
    }

    Ok(result_artifacts)
}


fn group_files_for_workers(files: &[WalkedFile]) -> Vec<&[WalkedFile]> {
    if files.is_empty() {
        return Vec::new();
    }

    let number_of_workers = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(files.len())
        .min(MAX_PARALLELISM);

    let chunk_len = (files.len() + number_of_workers - 1) / number_of_workers;

    files.chunks(chunk_len).collect()
}
