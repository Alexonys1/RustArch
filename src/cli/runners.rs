use std::fs;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::error::AppError;
use crate::algorithms::PipelineSettings;
use crate::archiver::{WalkedFile, WalkResult, ArchiveEntry, ArchivedArtifactEntry, Artifact};
use crate::archiver::{
    read_header_and_entries,
    unpack_entries_parallel,
    walk_directory_or_file,
    pack_files_parallel,
    write_empty_dirs,
    write_archive_header,
    create_thread_with_queue_writer,
};


pub fn run_pack(
    source_path: &str,
    target_archive_path: &str,
    settings: PipelineSettings,
    encode_key: Vec<u8>,
) -> Result<(), AppError>
{
    let source_path = Path::new(source_path);
    let target_archive_path = Path::new(target_archive_path);

    if !source_path.exists() {
        return Err(AppError::CLIUsage(format!(
            "'{}' не найден!", source_path.display()
        )));
    }

    fs::create_dir_all(
        target_archive_path
            .parent()
            .expect("У целевого архива всегда есть родитель! Даже Some(\"\")"),
    )?;

    let walked_targets: WalkResult = walk_directory_or_file(&source_path)?;
    let target_files: Vec<WalkedFile> = walked_targets.files;
    let target_empty_dirs: Vec<String> = walked_targets.empty_dirs;

    let (sender, receiver): (
        Sender<(ArchivedArtifactEntry, Artifact)>,
        Receiver<(ArchivedArtifactEntry, Artifact)>
    ) = channel();

    let handler_of_artifact_writer = create_thread_with_queue_writer(target_archive_path.to_path_buf(), receiver);
    pack_files_parallel(target_files, settings, &encode_key, sender)?;

    // Здесь мы ждём пока все артефакты не будут записаны в архив. Только после этого записываем заголовок:
    let archived_files = handler_of_artifact_writer
        .join()
        .map_err(|_| AppError::Compression("Поток записи архива аварийно завершился!".into()))??;

    write_empty_dirs(target_archive_path, target_empty_dirs)?;
    write_archive_header(target_archive_path, archived_files)?;

    Ok(())
}


pub fn run_unpack(
    source_path: &str,
    target_unpack_path: &str,
    decode_key: &[u8],
) -> Result<(), AppError>
{
    let source_path = Path::new(source_path);
    let target_unpack_path = Path::new(target_unpack_path);

    if !source_path.exists() {
        return Err(AppError::CLIUsage(format!(
            "'{}' не найден!", source_path.display()
        )));
    }

    let mut archive_file = fs::File::open(source_path)?;
    let entries: Vec<ArchiveEntry> = read_header_and_entries(&mut archive_file)?;
    drop(archive_file);

    fs::create_dir_all(target_unpack_path)?;
    unpack_entries_parallel(entries, source_path, target_unpack_path, decode_key)?;

    Ok(())
}
