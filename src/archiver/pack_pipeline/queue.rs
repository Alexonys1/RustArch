use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;

use crate::archiver::{ArchivedArtifactEntry, Artifact};
use crate::error::AppError;


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
            // Для Коли: артефакт дропнется из памяти сам. Не просто же так я .into_iter() пишу.
        }

        archive_file.sync_all()?;
        Ok(written_entries)
    })
}
