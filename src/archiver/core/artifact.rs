use std::borrow::Cow;
use std::io::{self, Read, Write, Seek, SeekFrom};
use std::fs::{self, File, OpenOptions};
use std::sync::{Mutex, OnceLock};
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::num::NonZeroUsize;

use crate::algorithms::PipelineSettings;
use super::memory_budget::BudgetGuard;


pub const DEFAULT_CHUNK_SIZE_IN_BYTES: usize = 512 * 1024; // 512KB


pub struct Artifact {
    pub chunk_size: NonZeroUsize,
    file_path: PathBuf,
    state: ArtifactState,
    is_temp_file: bool,
    reading_position: usize,
    writing_position: usize,
}


pub struct ArtifactAfterPipeline {
    pub relative_path: String,
    pub original_size: u64,
    pub size_after_pipeline: u64,
    pub payload: Artifact,
    pub pipeline: PipelineSettings,
    pub crc32: u32,
}


enum ArtifactState {
    File { file: File, size: usize },
    Memory { data: Vec<u8>, budget_guard: BudgetGuard },
    FileWindow { file: File, base_offset: u64, len: u64 },
}


impl Artifact {
    /// Открывает существующий файл на диске ТОЛЬКО для чтения. Этот
    /// конструктор используется для исходных пользовательских файлов,
    /// которые пайплайн не должен иметь возможности случайно испортить.
    pub fn from_file(path: &Path) -> io::Result<Artifact> {
        let file = File::open(path)?;
        let size = file.metadata()?.len() as usize;

        Ok(Artifact {
            state: ArtifactState::File { file, size },
            chunk_size: NonZeroUsize::new(DEFAULT_CHUNK_SIZE_IN_BYTES).unwrap(),
            file_path: path.to_path_buf(),
            is_temp_file: false, // Файл мы удалять не будем, т.к. он не временный
            reading_position: 0,
            writing_position: 0,
        })
    }

    /// Возвращает артефакт, читающий только часть файла без возможности его изменить.
    pub fn from_file_range(path: &Path, base_offset: u64, len: u64) -> io::Result<Artifact> {
        let file = File::open(path)?;
        // TODO: Добавить проверку на выход за границу файла.
        Ok(Self {
            state: ArtifactState::FileWindow { file, base_offset, len },
            chunk_size: NonZeroUsize::new(DEFAULT_CHUNK_SIZE_IN_BYTES).unwrap(),
            file_path: path.to_path_buf(),
            is_temp_file: false,
            reading_position: 0,
            writing_position: 0,
        })
    }

    // TODO: Ужасное название... Было, но лучше не стало.
    pub fn new_with_temp_file_suffix(artifact: &Artifact, suffix: &str) -> Artifact {
        Artifact {
            chunk_size: artifact.chunk_size,
            file_path: make_numbered_temp_file_path(artifact.file_path.as_path(), suffix),
            is_temp_file: true, // Новый производный артефакт - всегда наш собственный временный файл
            state: ArtifactState::Memory { data: Vec::new(), budget_guard: BudgetGuard::default() },
            reading_position: 0,
            writing_position: 0,
        }
    }

    pub fn save_as_finish_file<FilePath: AsRef<Path>>(mut self, new_path: &FilePath) -> io::Result<()> {
        match &self.state {
            ArtifactState::File { .. } => {
                let old_path: &Path = self.file_path.as_ref();
                fs::rename(old_path, new_path)?;
            }
            ArtifactState::Memory { data, .. } => fs::write(new_path, data)?,
            ArtifactState::FileWindow { .. } => return Err(io::Error::from(io::ErrorKind::ReadOnlyFilesystem)),
        }
        // Файл уже переименован/записан по новому пути - Drop не должен
        // пытаться удалить то, чего по старому пути больше не существует.
        self.is_temp_file = false;
        Ok(())
    }

    /// Возвращает кол-во хранящихся байт без учёта курсора чтения/записи.
    pub fn payload_size(&self) -> usize {
        match &self.state {
            ArtifactState::File { size, .. } => *size,
            ArtifactState::Memory { data, .. } => data.len(),
            ArtifactState::FileWindow { len, .. } => *len as usize,
        }
    }

    pub fn on_disk(&self) -> bool {
        match &self.state {
            ArtifactState::File { .. } => true,
            ArtifactState::FileWindow { .. } => true,
            _ => false,
        }
    }

    /// Если данные на диске, то будет возвращён None.
    /// Если данные в оперативной памяти, то будет возвращён Some
    pub fn new__next_mut_chunk(&mut self) -> io::Result<Option<&mut [u8]>> {
        match &mut self.state {
            ArtifactState::File { .. } => Ok(None),
            ArtifactState::Memory { data, .. } => {
                let to_read_bytes = data.len().saturating_sub(self.reading_position);
                let result = Ok(Some(&mut data[self.reading_position..self.reading_position + to_read_bytes]));
                self.reading_position += to_read_bytes;
                result
            }
            ArtifactState::FileWindow { .. } => Ok(None),
        }
    }

    /// Как это должно было выглядеть!
    pub fn new__next_chunk(&mut self) -> io::Result<Cow<'_, [u8]>> {
        match &mut self.state {
            ArtifactState::File { file, .. } => {
                file.seek(SeekFrom::Start(self.reading_position as u64))?; // TODO: А нужно ли?
                let mut buffer = vec![0; self.chunk_size.get()];
                self.reading_position += file.read(&mut buffer)?;

                Ok(Cow::Owned(buffer))
            }
            ArtifactState::Memory { data, .. } => {
                let to_read_bytes = data.len().saturating_sub(self.reading_position);
                self.reading_position += to_read_bytes;

                Ok(Cow::Borrowed(&data[self.reading_position..self.reading_position + to_read_bytes]))
            }
            ArtifactState::FileWindow { file, base_offset, len } => {
                let to_read = (*len).saturating_sub(self.reading_position as u64);

                if to_read == 0 {
                    return Ok(Cow::Owned(vec![]));
                }

                file.seek(SeekFrom::Start(*base_offset + self.reading_position as u64))?;
                let mut buffer = vec![0; self.chunk_size.get()];
                self.reading_position += file.read(&mut buffer)?;

                Ok(Cow::Owned(buffer))
            }
        }
    }

    /// Читает до `chunk_size` байт (или меньше, если `buf` меньше) с
    /// текущей позиции чтения. `0` означает конец данных.
    pub fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let max_to_read = buf.len().min(self.chunk_size.get());
        let actual_buf = &mut buf[..max_to_read];

        match &mut self.state {
            ArtifactState::File { file, .. } => {
                file.seek(SeekFrom::Start(self.reading_position as u64))?; // TODO: А нужно ли?

                let mut filled = 0;
                while filled < actual_buf.len() {
                    let n = file.read(&mut actual_buf[filled..])?;
                    if n == 0 { break; }
                    filled += n;
                }

                self.reading_position += filled;
                Ok(filled)
            }
            ArtifactState::Memory { data, .. } => {
                let remaining_of_bytes = data.len().saturating_sub(self.reading_position);
                let to_read_bytes = remaining_of_bytes.min(max_to_read);
                actual_buf[..to_read_bytes]
                    .copy_from_slice(&data[self.reading_position..self.reading_position + to_read_bytes]);
                self.reading_position += to_read_bytes;
                Ok(to_read_bytes)
            }
            ArtifactState::FileWindow { file, base_offset, len } => {
                let remaining = (*len).saturating_sub(self.reading_position as u64);
                let to_read = (remaining as usize).min(max_to_read);
                if to_read == 0 {
                    return Ok(0);
                }

                file.seek(SeekFrom::Start(*base_offset + self.reading_position as u64))?;

                let mut filled = 0;
                while filled < to_read {
                    let n = file.read(&mut actual_buf[filled..to_read])?;
                    if n == 0 { break; }
                    filled += n;
                }

                self.reading_position += filled;
                Ok(filled)
            }
        }
    }

    /// Обёртка над `read_chunk`: сама выделяет буфер размера
    /// `chunk_size` и возвращает `None` на конце данных.
    pub fn read_next_chunk(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut buf = vec![0u8; self.chunk_size.get()];
        let filled = self.read_chunk(&mut buf)?;

        if filled == 0 {
            return Ok(None);
        }

        buf.truncate(filled);
        Ok(Some(buf))
    }

    /// Записывает ВЕСЬ `buf` с текущей позиции записи, при необходимости
    /// разбивая его на куски по `chunk_size` - частичная запись невозможна:
    /// либо весь буфер будет записан, либо вернётся ошибка.
    pub fn write_chunk(&mut self, buf: &[u8]) -> io::Result<()> {
        let mut offset = 0;

        while offset < buf.len() {
            let written = self.write_chunk_once(&buf[offset..])?;
            if written == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Artifact::write_chunk: нулевая запись", // TODO: А нужно ли?
                ));
            }
            offset += written;
        }

        Ok(())
    }

    pub fn set_reading_position(&mut self, pos: usize) {
        self.reading_position = pos;
    }

    pub fn set_writing_position(&mut self, pos: usize) {
        self.writing_position = pos;
    }

    pub fn rewind_reading(&mut self) {
        self.reading_position = 0;
    }

    /// Скопирует данные артефакта в Writable.
    pub fn copy_into<Writable: Write>(&mut self, dest: &mut Writable) -> io::Result<()> {
        while let Some(chunk) = self.read_next_chunk()? {
            dest.write_all(&chunk)?;
        }
        Ok(())
    }


    /// Записывает не больше `chunk_size` байт за один вызов. Двигает курсор.
    fn write_chunk_once(&mut self, buf: &[u8]) -> io::Result<usize> {
        let max_to_write = buf.len().min(self.chunk_size.get());
        let actual_buf = &buf[..max_to_write];

        match &mut self.state {
            ArtifactState::File { file, size } => {
                file.seek(SeekFrom::Start(self.writing_position as u64))?;
                let written_bytes = file.write(actual_buf)?;
                self.writing_position += written_bytes;

                let new_end = self.writing_position;
                if new_end > *size {
                    *size = new_end;
                    file.set_len(new_end as u64)?;
                }
                Ok(written_bytes)
            }
            ArtifactState::Memory { data, budget_guard } => {
                let new_len = self.writing_position + max_to_write;

                if new_len > data.len() {
                    let additional_bytes: i64 = new_len as i64 - data.len() as i64;

                    if budget_guard.try_grow(additional_bytes) {
                        data.resize(new_len, 0);
                    } else {
                        // Не хватило бюджета памяти - сбрасываем накопленные
                        // данные на диск и продолжаем запись уже туда:
                        let temp_path: &Path = self.file_path.as_ref();

                        let mut file = OpenOptions::new()
                            .read(true)
                            .write(true)
                            .create(true)
                            .truncate(true)
                            .open(temp_path)?;

                        file.write_all(data)?;

                        self.state = ArtifactState::File { file, size: data.len() };

                        return self.write_chunk_once(buf);
                    }
                }

                let end = self.writing_position + max_to_write;
                data[self.writing_position..end].copy_from_slice(actual_buf);
                self.writing_position += max_to_write;

                Ok(max_to_write)
            }
            ArtifactState::FileWindow { .. } => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Artifact::FileWindow доступен только для чтения!",
            )),
        }
    }
}


impl Drop for Artifact {
    fn drop(&mut self) {
        match &self.state {
            ArtifactState::File { .. } => {
                if self.is_temp_file {
                    let path: &Path = self.file_path.as_ref();
                    let result: io::Result<()> = fs::remove_file(path);
                    println!("The artifact [FILE] has been removed: {:?}. Path: {:?}", result, self.file_path);
                } else {
                    println!("The artifact [FILE] has been dropped, but the file is still exists. Path: {:?}", self.file_path);
                }
            }
            ArtifactState::Memory { data, .. } => {
                println!("The artifact [MEMORY]  has been dropped: {} bytes", data.len());
                //super::memory_budget::show_memory_bar();
            }
            ArtifactState::FileWindow { len, .. } => {
                println!("The artifact [FILE WINDOW] has been dropped: {} bytes", len);
            }
        }
    }
}


impl Clone for Artifact {
    fn clone(&self) -> Artifact {
        todo!()
    }
}


fn make_numbered_temp_file_path(old_path: &Path, suffix: &str) -> PathBuf {
    static NUM_MAP: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();

    let filename = old_path
        .file_stem()
        .expect("Нельзя извлечь основу имени файла (stem)!")
        .to_string_lossy()
        .to_string();

    let mut map = NUM_MAP
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("Мьютекс отравлен!");

    let counter: &mut usize = map.entry(filename.clone().into()).or_insert(0);
    *counter += 1;

    if *counter > 1 {
        // TODO: Лучше сохранять временные файлы в отдельной временной директории, а потом её удалять.
        // TODO: Хотя тут подумать надо...
        format!("{}_{}_{}.tmp", filename, suffix, counter).into()
    } else {
        format!("{}_{}.tmp", filename, suffix).into()
    }//.into() // А что лучше по читаемости?
}
