use std::path::{Path, PathBuf};

use crate::archiver::ArchivedDirectoryEntry;
use crate::error::AppError;
use crate::archiver::ArchiveError;


pub fn create_empty_dirs(root: &Path, target_empty_dirs: &[ArchivedDirectoryEntry]) -> Result<(), AppError> {
    for empty_dir in target_empty_dirs {
        let absolute_path = resolve_output_path(root, &empty_dir.relative_path)?;
        std::fs::create_dir_all(&absolute_path)?;
    }

    Ok(())
}


/// Переводит `relative_path` записи в реальный путь на диске при распаковке.
pub fn resolve_output_path(output_dir: &Path, relative_path: &str) -> Result<PathBuf, AppError> {
    let mut result = output_dir.to_path_buf();

    for part in relative_path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err(ArchiveError::UnsafePath { path: relative_path.into() }.into());
        }
        result.push(part);
    }

    Ok(result)
}
