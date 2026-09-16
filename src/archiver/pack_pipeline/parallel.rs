use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::thread;

use crate::algorithms::PipelineSettings;
use crate::archiver::{ArchivedArtifactEntry, Artifact, WalkedFile};
use crate::error::AppError;
use super::one_file::pack_file;

const MAX_PARALLELISM: usize = usize::MAX;


/// Параллельно готовит payload'ы. Фактические payload_offset назначаются
/// единственным writer-потоком, поэтому порядок завершения worker'ов не важен.
pub fn pack_files_parallel(
    files: Vec<WalkedFile>,
    pipeline_settings: PipelineSettings,
    encode_key: &[u8],
    artifact_sender: Sender<(ArchivedArtifactEntry, Artifact)>,
) -> Result<Vec<ArchivedArtifactEntry>, AppError> {
    let groups_of_files = group_files_for_workers(&files)?;

    thread::scope(|scope| {
        let mut workers = Vec::with_capacity(groups_of_files.len());

        for file_group in &groups_of_files {
            workers.push(scope.spawn(|| {
                start_packing_file_group(file_group, pipeline_settings, encode_key, artifact_sender.clone())
            }));
        }

        let mut result = Vec::with_capacity(files.len());
        for worker in workers {
            let entries = worker
                .join()
                .map_err(|_| AppError::Compression("Паника в рабочем потоке".into()))??;
            result.extend(entries);
        }

        Ok(result)
    })
}

fn start_packing_file_group(
    file_group: &[&WalkedFile],
    pipeline_settings: PipelineSettings,
    encode_key: &[u8],
    artifact_sender: Sender<(ArchivedArtifactEntry, Artifact)>,
) -> Result<Vec<ArchivedArtifactEntry>, AppError> {
    let mut result = Vec::with_capacity(file_group.len());

    for &file in file_group {
        result.push(pack_file(file, pipeline_settings, encode_key, artifact_sender.clone())?);
    }

    Ok(result)
}

fn group_files_for_workers(files: &[WalkedFile]) -> Result<Vec<Vec<&WalkedFile>>, AppError> {
    if files.is_empty() {
        return Ok(Vec::new());
    }

    let number_of_workers = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(files.len())
        .min(MAX_PARALLELISM);

    let mut sized_files = Vec::with_capacity(files.len());
    for file in files {
        sized_files.push((file.get_size()?, file));
    }
    sized_files.sort_by_key(|&(size, _)| Reverse(size));

    let mut groups: Vec<Vec<(u64, &WalkedFile)>> =
        (0..number_of_workers).map(|_| Vec::new()).collect();
    let mut heap: BinaryHeap<Reverse<(u64, usize)>> =
        (0..number_of_workers).map(|i| Reverse((0, i))).collect();

    for (size, file) in sized_files {
        let Reverse((total_size, group_idx)) = heap.pop().unwrap();
        groups[group_idx].push((size, file));
        heap.push(Reverse((total_size + size, group_idx)));
    }

    for group in &mut groups {
        group.sort_by_key(|&(size, _)| Reverse(size));
    }

    Ok(groups
        .into_iter()
        .map(|group| group.into_iter().map(|(_, file)| file).collect())
        .collect())
}
