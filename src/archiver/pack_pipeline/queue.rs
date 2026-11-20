use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;
#[cfg(feature = "statistics")]
use std::time::{Instant, Duration};

#[cfg(feature = "statistics")]
use crate::archiver::statistics_println;
use crate::archiver::{ArchivedArtifactEntry, Artifact};
use crate::error::AppError;


#[cfg(not(feature = "statistics"))]
pub fn create_thread_with_queue_writer(
    target_archive_path: PathBuf,
    artifact_receiver: Receiver<(ArchivedArtifactEntry, Artifact)>,
) -> JoinHandle<Result<Vec<ArchivedArtifactEntry>, AppError>>
{
    std::thread::spawn(move || {
        let mut archive_file: File = File::create(&target_archive_path)?;
        let mut written_entries: Vec<ArchivedArtifactEntry> = Vec::new();

        for (mut entry, mut artifact) in artifact_receiver.into_iter() {
            let payload_offset: u64 = archive_file.seek(SeekFrom::End(0))?;
            artifact.write_to_file_end(&mut archive_file)?;

            entry.payload_offset = payload_offset;
            written_entries.push(entry);
        }

        archive_file.sync_all()?;
        Ok(written_entries)
    })
}


#[cfg(feature = "statistics")]
pub fn create_thread_with_queue_writer(
    target_archive_path: PathBuf,
    artifact_receiver: Receiver<(ArchivedArtifactEntry, Artifact)>,
    start_program_time: Instant,
) -> JoinHandle<Result<Vec<ArchivedArtifactEntry>, AppError>>
{
    std::thread::spawn(move || {
        let mut archive_file: File = File::create(&target_archive_path)?;
        let mut written_entries_with_times: Vec<(ArchivedArtifactEntry, Duration, Duration)> = Vec::new();

        for (mut entry, mut artifact) in artifact_receiver.into_iter() {
            let writing_started_at = start_program_time.elapsed();

            let payload_offset: u64 = archive_file.seek(SeekFrom::End(0))?;
            artifact.write_to_file_end(&mut archive_file)?;

            let writing_duration = start_program_time.elapsed() - writing_started_at;

            entry.payload_offset = payload_offset;
            written_entries_with_times.push(
                (entry, writing_started_at, writing_duration)
            );
        }


        let sync_started_at  = start_program_time.elapsed();

        archive_file.sync_all()?;

        let sync_duration = start_program_time.elapsed() - sync_started_at;

        statistics_println!(
            "===> Сброс всех изменений на диск. Начало: {:?} | Длительность: {:?}",
            sync_started_at,
            sync_duration,
        );


        statistics_println!("=== Запись файлов в целевой архив ===\n{written_entries_with_times:#?}\n");


        Ok(written_entries_with_times.into_iter().map(|x| x.0).collect())
    })
}
