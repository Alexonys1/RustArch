use std::path::{Path, PathBuf};

use crate::archiver::ArchivedDirectoryEntry;
use crate::error::AppError;


#[derive(Debug)]
pub struct WalkedFile {
    pub absolute_path: PathBuf,
    /// Путь относительно корня архивации, всегда с '/'-разделителем.
    pub relative_path: String,
}


pub struct WalkResult {
    pub files: Vec<WalkedFile>,
    pub empty_dirs: Vec<ArchivedDirectoryEntry>,
}


impl WalkedFile {
    pub fn get_size(&self) -> Result<u64, AppError> {
        let metadata = std::fs::metadata(self.absolute_path.as_path())?;
        Ok(metadata.len())
    }
}


// TODO: Наверное, эти дженерики всё-таки лишние и можно было обойтись обычным &Path
pub fn walk_directory_or_file<RootPath: AsRef<Path> + ?Sized>(root: &RootPath) -> Result<WalkResult, AppError> {
    let root = root.as_ref(); // Дженерики в приватных функциях были правда лишние, пока я не додумался до let root = root.as_ref()

    if root.is_file() {
        return get_one_file(root);
    }

    let mut files = Vec::new();
    let mut empty_dirs = Vec::new();

    walk_recursive(root, root, &mut files, &mut empty_dirs)?;

    Ok(WalkResult { files, empty_dirs })
}


fn walk_recursive(
    root: &Path, 
    current: &Path,
    files: &mut Vec<WalkedFile>,
    empty_dirs: &mut Vec<ArchivedDirectoryEntry>
) -> Result<(), AppError>
{
    let mut saw_any_entry = false;

    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        saw_any_entry = true;

        if file_type.is_dir() {
            walk_recursive(root, &path, files, empty_dirs)?;
        }
        else if file_type.is_file() {
            let relative_path = path
                .strip_prefix(root)
                .expect("Путь всегда должен быть внутри root, так как получен обходом от root");

            files.push(WalkedFile {
                absolute_path: path.clone(),
                relative_path: convert_to_platform_undepended_path(&relative_path),
            });
        }
        // Симлинки мы не храним, поэтому и не читаем
    }

    // Пустая директория - это либо сам root без единого элемента внутри,
    // либо любая вложенная директория без файлов и без непустых поддиректорий.
    if !saw_any_entry && current != root {
        let relative_path = current
            .strip_prefix(root)
            .expect("Путь всегда должен быть внутри root!");
        empty_dirs.push(
            ArchivedDirectoryEntry::new(convert_to_platform_undepended_path(&relative_path))
        );
    }

    Ok(())
}


fn convert_to_platform_undepended_path(relative_path: &Path) -> String {
    // components() разбивает путь платформо-независимо;
    // склеиваем обратно через /, игнорируя ОС-специфичный разделитель.
    relative_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}


fn get_one_file(root: &Path) -> Result<WalkResult, AppError> {
    let file_name = root.file_name().ok_or_else(|| {
        AppError::CLIUsage(format!("Не удалось определить имя файла из пути '{}'", root.display()))
    })?;

    Ok(WalkResult {
        files: vec![WalkedFile {
            absolute_path: root.to_path_buf(),
            relative_path: file_name.to_string_lossy().into_owned(),
        }],
        empty_dirs: vec![],
    })
}
