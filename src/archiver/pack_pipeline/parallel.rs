use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::mpsc::Sender;
use std::thread;
#[cfg(feature = "statistics")]
use std::time::{Duration, Instant};

use crate::algorithms::PipelineSettings;
use crate::archiver::{ArchivedArtifactEntry, Artifact, WalkedFile};
#[cfg(feature = "statistics")]
use crate::archiver::statistics_println;
use crate::error::AppError;
use crate::archiver::PipelineError;
use super::one_file::pack_file;

const MAX_PARALLELISM: usize = usize::MAX;


pub fn pack_files_parallel(
    files: Vec<WalkedFile>,
    pipeline_settings: PipelineSettings,
    encode_key: &[u8],
    artifact_sender: Sender<(ArchivedArtifactEntry, Artifact)>,
    #[cfg(feature = "statistics")] start_program_time: Instant,
) -> Result<(), AppError>
{
    #[cfg(feature = "statistics")]
    let grouping_started_at  = start_program_time.elapsed();

    let groups_of_files: Vec<Vec<&WalkedFile>> = group_files_for_workers(&files, MAX_PARALLELISM)?;

    #[cfg(feature = "statistics")]
    {
        let grouping_duration = start_program_time.elapsed() - grouping_started_at;
        statistics_println!("===> Начало группировки: {:?} | Длительность: {:?}", grouping_started_at, grouping_duration);
    }


    #[cfg(feature = "statistics")]
    let mut groups_of_files_with_running_time: Vec<Vec<(WalkedFile, Duration, Duration)>> = Vec::with_capacity(groups_of_files.len());


    let scope_result: Result<(), AppError> =
    thread::scope(|scope| {
        let mut workers = Vec::with_capacity(groups_of_files.len());

        for file_group in &groups_of_files {
            workers.push(scope.spawn(|| {
                pack_file_group(file_group, pipeline_settings, encode_key, artifact_sender.clone(), #[cfg(feature = "statistics")] start_program_time)
            }));
        }

        for worker in workers {
            #[allow(unused)]
            let files_with_running_time = worker
                .join()
                .map_err(|_| PipelineError::WorkerPanicked)??;

            #[cfg(feature = "statistics")]
            groups_of_files_with_running_time.push(files_with_running_time);
        }

        Ok(())
    });

    drop(artifact_sender); // Для того, чтобы вывод статистки не замедлял работу программы.

    // Обработаем статистику прямо здесь, не поднимая её наверх. Здесь у нас уже есть все необходимые данные:
    #[cfg(feature = "statistics")]
    {
        statistics_println!("Номера потоков условны и не привязаны к физическим ядрам процессора и тому подобному!");

        for (i, group) in (1..).zip(groups_of_files_with_running_time) {
            statistics_println!("ПОТОК #{i})\n{group:#?}\n");
        }
    }

    scope_result
}


#[cfg(feature = "statistics")]
fn pack_file_group(
    file_group: &[&WalkedFile],
    pipeline_settings: PipelineSettings,
    encode_key: &[u8],
    artifact_sender: Sender<(ArchivedArtifactEntry, Artifact)>,
    start_program_time: Instant,
) -> Result<Vec<(WalkedFile, Duration, Duration)>, AppError>
{
    let mut files_with_running_time: Vec<(WalkedFile, Duration, Duration)> = Vec::with_capacity(file_group.len());

    for file in file_group {
        // Замеряя время прохождения одного файла через пайплайн, мы можем посчитать и суммарное время работы всего потока (работника).
        let started_at = start_program_time.elapsed();

        pack_file(file, pipeline_settings, encode_key, artifact_sender.clone())?;

        let duration = start_program_time.elapsed() - started_at;

        files_with_running_time.push(
            (
                (**file).clone(),  // На самом деле можно обойтись и одной :D
                started_at,
                duration,
            )
        );
    }

    Ok(files_with_running_time)
}


#[cfg(not(feature = "statistics"))]
fn pack_file_group(
    file_group: &[&WalkedFile],
    pipeline_settings: PipelineSettings,
    encode_key: &[u8],
    artifact_sender: Sender<(ArchivedArtifactEntry, Artifact)>,
) -> Result<(), AppError>
{
    for file in file_group {
        pack_file(file, pipeline_settings, encode_key, artifact_sender.clone())?;
    }

    Ok(())
}


pub fn group_files_for_workers(files: &[WalkedFile], max_parallelism: usize) -> Result<Vec<Vec<&WalkedFile>>, AppError> {
    assert!(max_parallelism > 0);

    if files.is_empty() {
        return Ok(Vec::new());
    }

    let number_of_workers = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(files.len())
        .min(max_parallelism);

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

    let result = groups
        .into_iter()
        .map(|group| group.into_iter().map(|(_, file)| file).collect())
        .collect();

    //println!("{result:#?}");

    Ok(result)
}
