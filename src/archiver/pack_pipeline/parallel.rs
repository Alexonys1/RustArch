use std::cmp::Reverse;
use std::sync::{Arc, Mutex};
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

    let number_of_threads: usize = calc_number_of_threads(&files);
    let sorted_files: Arc<Mutex<std::vec::IntoIter<WalkedFile>>> = make_file_queue(files)?;

    #[cfg(feature = "statistics")]
    {
        let grouping_duration = start_program_time.elapsed() - grouping_started_at;
        statistics_println!("===> Начало группировки: {:?} | Длительность: {:?}", grouping_started_at, grouping_duration);
    }


    #[cfg(feature = "statistics")]
    let mut groups_of_files_with_running_time: Vec<Vec<(WalkedFile, Duration, Duration)>> = Vec::with_capacity(number_of_threads);


    let scope_result: Result<(), AppError> =
    thread::scope(|scope| {
        let mut workers = Vec::with_capacity(number_of_threads);

        for _ in 0..number_of_threads {
            workers.push(scope.spawn(|| {
                pack_file_group(sorted_files.clone(), pipeline_settings, encode_key, artifact_sender.clone(), #[cfg(
                    feature = "statistics"
                )] start_program_time)
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
    files: Arc<Mutex<std::vec::IntoIter<WalkedFile>>>,
    pipeline_settings: PipelineSettings,
    encode_key: &[u8],
    artifact_sender: Sender<(ArchivedArtifactEntry, Artifact)>,
    start_program_time: Instant,
) -> Result<Vec<(WalkedFile, Duration, Duration)>, AppError>
{
    let mut files_with_running_time: Vec<(WalkedFile, Duration, Duration)> = Vec::with_capacity(files.lock().unwrap().len());

    loop {
        let next_file = files.lock().unwrap().next();

        let Some(file) = next_file else {
            return Ok(files_with_running_time)
        };

        let started_at = start_program_time.elapsed();
        pack_file(&file, pipeline_settings, encode_key, &artifact_sender)?;
        let pack_duration = start_program_time.elapsed() - started_at;

        files_with_running_time.push((file, started_at, pack_duration));
    }
}


#[cfg(not(feature = "statistics"))]
fn pack_file_group(
    files: Arc<Mutex<std::vec::IntoIter<WalkedFile>>>,
    pipeline_settings: PipelineSettings,
    encode_key: &[u8],
    artifact_sender: Sender<(ArchivedArtifactEntry, Artifact)>,
) -> Result<(), AppError>
{
    loop {
        let next_file: Option<WalkedFile> = files.lock().expect("Мьютекс общей очереди отравлен!").next();

        let Some(file) = next_file
        else {
            break
        };

        pack_file(&file, pipeline_settings, encode_key, &artifact_sender)?;
    }

    Ok(())
}


pub fn make_file_queue(files: Vec<WalkedFile>) -> Result<Arc<Mutex<std::vec::IntoIter<WalkedFile>>>, AppError> {
    if files.is_empty() {
        return Ok(Arc::new(Mutex::new(Vec::new().into_iter())));
    }

    // Отсортируем файлы по их размеру по убыванию.
    // Так лучше, потому что потоки будут брать сначала самые тяжёлые файлы на обработку,
    // оставляя маленькие на потом.
    let mut sized_files: Vec<(&WalkedFile, u64)> = Vec::with_capacity(files.len());
    for file in files.iter() {
        let file_size: u64 = file.get_size()?;
        sized_files.push((file, file_size));
    }

    sized_files.sort_by_key(|f| Reverse(f.1));

    let sorted_files: Vec<WalkedFile> = sized_files.into_iter().map(|f| f.0.clone()).collect();

    Ok(
        Arc::new(Mutex::new(sorted_files.into_iter()))
    )
}


fn calc_number_of_threads(files: &[WalkedFile]) -> usize {
    thread::available_parallelism().expect("Нельзя узнать кол-во доступных потоков у ОС!").get()
        .min(MAX_PARALLELISM)
        .min(files.len())
}
