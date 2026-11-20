use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
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

    let mut sample = [0u8; PROBE_SIZE];
    let mut weighted_files: Vec<(f64, &WalkedFile)> = Vec::with_capacity(files.len());

    for file in files {
        let mut input = File::open(&file.absolute_path)?;
        let size = input.metadata()?.len();
        let weight = file_weight(&mut input, size, &mut sample)?;
        weighted_files.push((weight, file));
    }

    weighted_files.sort_by(|left, right| right.0.total_cmp(&left.0));

    let mut groups = vec![Vec::new(); number_of_workers];

    let mut loads: BinaryHeap<_> = (0..number_of_workers)
        .map(|worker| Reverse((0.0_f64.to_bits(), 0usize, worker)))
        .collect();

    for (weight, file) in weighted_files {
        let Reverse((load_bits, count, worker)) = loads.pop().unwrap();
        groups[worker].push(file);
        let load = f64::from_bits(load_bits) + weight;
        loads.push(Reverse((load.to_bits(), count + 1, worker)));
    }

    Ok(groups)
}


const PROBE_SIZE: usize = 4 * 1024;


fn file_weight(input: &mut (impl Read + Seek), size: u64, buffer: &mut [u8; PROBE_SIZE]) -> io::Result<f64> {
    if size == 0 {
        return Ok(0.0);
    }

    let sampled = size.min(3 * PROBE_SIZE as u64) as usize;
    let edge = sampled / 3;
    let lengths = if sampled <= PROBE_SIZE {
        [sampled, 0, 0]
    }
    else {
        [
            edge,
            sampled - edge - sampled.div_ceil(3),
            sampled.div_ceil(3),
        ]
    };

    let offsets = [0, (size - lengths[1] as u64) / 2, size - lengths[2] as u64];
    let mut matched = 0;
    let mut position = 0;

    for (offset, length) in offsets.into_iter().zip(lengths) {
        if length == 0 {
            continue;
        }

        if offset != position {
            input.seek(SeekFrom::Start(offset))?;
        }

        let region = &mut buffer[..length];

        input.read_exact(region)?;
        matched += lzss_matched_bytes(region);
        position = offset + length as u64;
    }

    // При размере 4 МиБ множитель резерва равен двум. Так потоку с крупным
    // файлом обычно достаётся меньше дополнительной работы.
    const RESERVE_SCALE_BYTES: f64 = 4.0 * 1024.0 * 1024.0;

    let size = size as f64;
    let match_ratio = matched as f64 / sampled as f64;

    Ok(size * (1.0 + match_ratio) * (1.0 + size / RESERVE_SCALE_BYTES))
}


/// Считает байты, покрытые совпадениями.
fn lzss_matched_bytes(data: &[u8]) -> usize {
    const WINDOW: usize = 4096;
    const MAX_MATCH: usize = 258;
    const HASH_BITS: u32 = 10;

    let mut latest = [usize::MAX; 1 << HASH_BITS];
    let mut matched = 0;
    let mut pos = 0;

    while pos + 3 <= data.len() {
        let key = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], 0]);
        let slot = (key.wrapping_mul(0x9e37_79b1) >> (32 - HASH_BITS)) as usize;
        let previous = std::mem::replace(&mut latest[slot], pos);
        if previous == usize::MAX
            || pos - previous > WINDOW
            || data[previous..previous + 3] != data[pos..pos + 3]
        {
            pos += 1;
            continue;
        }

        let limit = MAX_MATCH.min(data.len() - pos);
        let mut length = 3;

        while length < limit && data[previous + length] == data[pos + length] {
            length += 1;
        }

        matched += length;
        pos += length;
    }

    matched
}
