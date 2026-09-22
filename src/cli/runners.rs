use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use colored::Colorize;

use crate::error::AppError;
use crate::algorithms::PipelineSettings;
use crate::archiver::{ArchivedDirectoryEntry, ArchivedArtifactEntry, Artifact, WalkResult, WalkedFile};
use crate::archiver::{
    create_empty_dirs, create_thread_with_queue_writer, pack_files_parallel, read_archive_entries,
    unpack_entries_parallel, walk_directory_or_file, write_archive_header,
};
use super::{CLICommand, print_help};


pub fn run_command(command: CLICommand) -> Result<(), AppError> {
    match command {
        CLICommand::Pack {
            source_path,
            target_archive_path,
            settings,
            encode_key,
        } => run_pack(&source_path, &target_archive_path, settings, &encode_key),

        CLICommand::Unpack {
            source_path,
            target_unpack_path,
            decode_key,
        } => run_unpack(&source_path, &target_unpack_path, &decode_key),

        CLICommand::ShowArchiveInnerStructure { archive_path } => run_list(&archive_path),

        CLICommand::Help => {
            print_help();
            Ok(())
        }

        CLICommand::Version => {
            println!(
                "{} {}",
                "rustarch".cyan().bold(),
                env!("CARGO_PKG_VERSION").cyan(),
            );
            Ok(())
        }
    }
}


pub fn run_pack(source_path: &Path, target_archive_path: &Path, settings: PipelineSettings, encode_key: &[u8]) -> Result<(), AppError> {
    if !source_path.exists() {
        return Err(AppError::CLIUsage(format!(
            "'{}' не найден!",
            source_path.display()
        )));
    }

    if same_path(source_path, target_archive_path)? {
        return Err(AppError::CLIUsage(
            "Исходный файл и целевой архив не могут быть одним путём".into(),
        ));
    }

    if source_path.is_dir()
        && canonical_or_absolute(target_archive_path)?.starts_with(fs::canonicalize(source_path)?)
    {
        return Err(AppError::CLIUsage(
            "Целевой архив не может находиться внутри исходной директории".into(),
        ));
    }

    if let Some(parent) = target_archive_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let walked_targets: WalkResult = walk_directory_or_file(&source_path)?;
    let target_files: Vec<WalkedFile> = walked_targets.files;
    let archived_directories: Vec<ArchivedDirectoryEntry> = walked_targets.empty_dirs;

    let (sender, receiver): (
        Sender<(ArchivedArtifactEntry, Artifact)>,
        Receiver<(ArchivedArtifactEntry, Artifact)>,
    ) = channel();

    let handler_of_artifact_writer = create_thread_with_queue_writer(target_archive_path.to_path_buf(), receiver);
    pack_files_parallel(target_files, settings, &encode_key, sender)?;

    // Здесь мы ждём пока все артефакты не будут записаны в архив. Только после этого записываем заголовок:
    let archived_files = handler_of_artifact_writer
        .join()
        .map_err(|_| AppError::Compression("Поток записи архива аварийно завершился!".into()))??;

    write_archive_header(target_archive_path, archived_files, archived_directories)?;

    Ok(())
}


pub fn run_unpack(archive_path: &Path, unpack_path: &Path, decode_key: &[u8]) -> Result<(), AppError> {
    if !archive_path.is_file() {
        return Err(AppError::CLIUsage(format!(
            "'{}' не является файлом архива",
            archive_path.display()
        )));
    }

    let mut archive_file = fs::File::open(archive_path)?;
    let (files, directories) = read_archive_entries(&mut archive_file)?;
    drop(archive_file);

    fs::create_dir_all(unpack_path)?;
    create_empty_dirs(unpack_path, &directories)?;
    unpack_entries_parallel(files, archive_path, unpack_path, decode_key)?;

    Ok(())
}


pub fn run_list(archive_path: &Path) -> Result<(), AppError> {
    if !archive_path.is_file() {
        return Err(AppError::CLIUsage(format!(
            "'{}' не является файлом архива",
            archive_path.display()
        )));
    }

    let mut archive_file = fs::File::open(archive_path)?;
    let (files, directories) = read_archive_entries(&mut archive_file)?;
    let total_original = files
        .iter()
        .fold(0u64, |sum, entry| sum.saturating_add(entry.original_size));
    let total_stored = files
        .iter()
        .fold(0u64, |sum, entry| sum.saturating_add(entry.stored_size));
    let total_saved_ratio = format_saved_ratio(total_original, total_stored);

    println!(
        "{} {}",
        "Архив:".bold(),
        archive_path.display().to_string().cyan(),
    );
    println!(
        "{} {} файлов, {} директорий; {} {}; {} {}; {} {}",
        "Записей:".bold(),
        files.len().to_string().cyan(),
        directories.len().to_string().cyan(),
        "исходный размер:".bold(),
        to_human_size(total_original).cyan(),
        "payload:".bold(),
        to_human_size(total_stored).cyan(),
        "сжатие:".bold(),
        total_saved_ratio.cyan(),
    );
    println!();
    let header = format!(
        "{:<4} {:>12} {:>12} {:>8} {:<12} {:<8} {:<12} {}",
        "TYPE", "ORIGINAL", "STORED", "RATIO", "COMPRESSION", "CIPHER", "FEC", "PATH"
    );
    println!("{}", header.cyan().bold());

    for entry in files {
        let saved_ratio = format_saved_ratio(entry.original_size, entry.stored_size);
        let kind = format!("{:<4}", "FILE").green().bold();
        let original = format!("{:>12}", to_human_size(entry.original_size)).cyan();
        let stored = format!("{:>12}", to_human_size(entry.stored_size)).cyan();
        let saved_ratio = format!("{:>8}", saved_ratio).cyan();
        let compression = format!("{:<12}", entry.pipeline.compression.as_str()).yellow();
        let cipher = format!("{:<8}", entry.pipeline.cipher.as_str()).yellow();
        let fec = format!("{:<12}", entry.pipeline.fec.as_str()).yellow();
        let path = entry.relative_path.to_string().cyan();
        println!("{kind} {original} {stored} {saved_ratio} {compression} {cipher} {fec} {path}");
    }

    for entry in directories {
        let kind = format!("{:<4}", "DIR").blue().bold();
        let original = format!("{:>12}", "-").dimmed();
        let stored = format!("{:>12}", "-").dimmed();
        let saved_ratio = format!("{:>8}", "-").dimmed();
        let compression = format!("{:<12}", "-").dimmed();
        let cipher = format!("{:<8}", "-").dimmed();
        let fec = format!("{:<12}", "-").dimmed();
        let path = entry.relative_path.to_string().cyan();
        println!("{kind} {original} {stored} {saved_ratio} {compression} {cipher} {fec} {path}");
    }

    Ok(())
}


fn format_saved_ratio(original_size: u64, stored_size: u64) -> String {
    if original_size == 0 {
        "-".to_string()
    } else {
        format!(
            "{:.1}%",
            (1.0 - stored_size as f64 / original_size as f64) * 100.0,
        )
    }
}


fn to_human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}


fn same_path(left: &Path, right: &Path) -> Result<bool, AppError> {
    let left = canonical_or_absolute(left)?;
    let right = canonical_or_absolute(right)?;
    Ok(left == right)
}


fn canonical_or_absolute(path: &Path) -> Result<PathBuf, AppError> {
    if path.exists() {
        return Ok(fs::canonicalize(path)?);
    }

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    if let (Some(parent), Some(file_name)) = (absolute.parent(), absolute.file_name()) {
        if parent.exists() {
            return Ok(fs::canonicalize(parent)?.join(file_name));
        }
    }
    Ok(absolute)
}
