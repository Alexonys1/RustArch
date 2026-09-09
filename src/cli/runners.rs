use std::fs;
use std::path::Path;

use crate::error::AppError;
use crate::algorithms::PipelineSettings;
use crate::archiver::{ArtifactAfterPipeline, WalkedFile, WalkResult, ArchiveEntry};
use crate::archiver::{assemble_archive, read_header_and_entries, unpack_entries_parallel, walk_directory_or_file, pack_files_parallel};


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

    // TODO: Всё-таки лучше передавать настройки архивации явно через аргументы функций, чем через глобальную переменную.
    // TODO: Так и для тестирования лучше...
    //set_pipeline_settings_as_global(settings, encode_key);

    fs::create_dir_all(target_archive_path.parent().expect("У целевого архива всегда есть родитель! Даже Some(\"\")"))?;

    // Получим все файлы и директории внутри source_path, если
    // это папка; или файл, если это один файл:
    let walked_targets: WalkResult = walk_directory_or_file(&source_path)?;
    let target_files: Vec<WalkedFile> = walked_targets.files;
    let target_empty_dirs: Vec<String> = walked_targets.empty_dirs;

    let cooked_artifacts: Vec<ArtifactAfterPipeline> = pack_files_parallel(target_files, settings, &encode_key)?;

    // На этом моменте создастся файл архива, если нет никакой ошибки:
    assemble_archive(target_archive_path, settings, cooked_artifacts, target_empty_dirs)?;

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
    let (pipeline_settings, entries): (PipelineSettings, Vec<ArchiveEntry>) = read_header_and_entries(&mut archive_file)?;
    drop(archive_file); // Дальше каждый поток откроет archive_path сам

    fs::create_dir_all(target_unpack_path)?;

    unpack_entries_parallel(entries, source_path, target_unpack_path, pipeline_settings, decode_key)?;

    Ok(())
}
