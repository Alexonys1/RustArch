use std::io::{self, Read, Write, Seek, SeekFrom};
use std::fs::{self, File, OpenOptions};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::sync::atomic::Ordering;
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::borrow::Cow;

use crate::error::AppError;
use crate::archiver::ArchiveError;
use super::memory_budget::{BudgetGuard, MEMORY_BUDGET_IN_BYTES};


pub const DEFAULT_CHUNK_SIZE_IN_BYTES: usize = 512 * 1024; // 512KB // Найдено опытным путём. Больше-меньше - хуже скорость на моей машине

// TODO: Я бы мог сделать ArtifactBuilder!! Как в стандартной библиотеке для открытия файла с указанием параметров в OpenOptions!!!


pub struct Artifact {
    pub chunk_size: NonZeroUsize,
    state: ArtifactState,
    file_path: PathBuf,
    is_temp_file: bool,
    cursor: usize,
}


enum ArtifactState {
    File { file: File, size: usize },
    Memory { data: Vec<u8>, budget_guard: Option<BudgetGuard> },
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
            cursor: 0,
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
            cursor: 0,
        })
    }

    // TODO: Ужасное название... Было, но лучше не стало.
    pub fn new_with_temp_file_suffix(artifact: &Artifact, suffix: &str) -> Result<Artifact, AppError> {
        if Self::should_artifact_be_placed_on_disk(artifact) {
            let file_path: PathBuf = make_numbered_temp_file_path(artifact.file_path.as_ref(), suffix);
            Ok(Artifact {
                is_temp_file: true, // Новый артефакт - всегда наш собственный временный файл
                state: ArtifactState::File {
                    file: OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .open(&file_path)?,
                    size: 0,
                },
                chunk_size: artifact.chunk_size,
                cursor: 0,
                file_path,
            })
        }
        else {
            Ok(Artifact {
                chunk_size: artifact.chunk_size,
                file_path: make_numbered_temp_file_path(artifact.file_path.as_ref(), suffix),
                is_temp_file: true,
                state: ArtifactState::Memory { data: Vec::new(), budget_guard: Some(BudgetGuard::default()) },
                cursor: 0,
            })
        }
    }

    /// Создаёт артефакт, который лежит *только* в оперативной памяти. Он не может быть сброшен на диск.
    /// И он не участвует в подсчёте бюджета памяти.
    pub fn new_small() -> Artifact {
        Artifact {
            chunk_size: NonZeroUsize::new(DEFAULT_CHUNK_SIZE_IN_BYTES).unwrap(),
            state: ArtifactState::Memory { data: vec![], budget_guard: None },
            file_path: "ONLY_MEMORY".into(),
            is_temp_file: false, // Файла в любом случае не будет. См. деструктор
            cursor: 0,
        }
    }

    pub fn save_as_finish_file(mut self, new_path: &Path) -> io::Result<()> {
        match &self.state {
            ArtifactState::File { .. } => {
                let old_path: &Path = self.file_path.as_ref();
                fs::rename(old_path, new_path)?;
            }
            
            ArtifactState::Memory { data, .. } => fs::write(new_path, data)?,
            
            ArtifactState::FileWindow { file, base_offset, len } => {
                let mut source = file.try_clone()?; // отдельный курсор чтения, не мешающий остальным операциям с этим же file
                source.seek(SeekFrom::Start(*base_offset))?;

                let mut out = File::create(new_path)?;
                let mut remaining = *len;
                let mut buffer = vec![0u8; self.chunk_size.get()];

                while remaining > 0 {
                    let want = remaining.min(buffer.len() as u64) as usize;
                    source.read_exact(&mut buffer[..want])?;
                    out.write_all(&buffer[..want])?;
                    remaining -= want as u64;
                }
            }
        }
        // Файл уже переименован/записан по новому пути - Drop не должен
        // пытаться удалить то, чего по старому пути больше не существует.
        self.is_temp_file = false;
        Ok(())
    }

    /// Преобразует Artifact в Vec<u8>. Внутренние курсоры cursor, writing_position
    /// не учитываются и чтение начинается с начала файла или памяти.
    /// Учитывается только курсор для файлового окна.
    pub fn into_vec(mut self) -> io::Result<Vec<u8>> {
        match &mut self.state {
            ArtifactState::File { file, size } => {
                let mut data = vec![0; *size];

                file.seek(SeekFrom::Start(0))?;
                file.read_exact(&mut data)?;

                Ok(data)
            }

            ArtifactState::Memory { data, .. } => Ok(std::mem::take(data)),

            ArtifactState::FileWindow { file, base_offset, len } => {
                let size = *len as usize;
                let mut data = vec![0; size];

                file.seek(SeekFrom::Start(*base_offset))?;
                file.read_exact(&mut data)?;

                Ok(data)
            }
        }
    }

    pub fn write_to_file_end(&mut self, sink: &mut File) -> io::Result<()> {
        while let Some(chunk) = self.next_chunk()? {
            sink.write_all(&chunk)?;
        }

        sink.flush()?;

        Ok(())
    }

    /// Возвращает кол-во хранящихся байт без учёта положения курсора чтения/записи.
    pub fn get_payload_size(&self) -> usize {
        match &self.state {
            ArtifactState::File      { size, .. } => *size,
            ArtifactState::Memory { data, .. } => data.len(),
            ArtifactState::FileWindow  { len, .. } => *len as usize,
        }
    }

    pub fn on_disk(&self) -> bool {
        match &self.state {
            ArtifactState::File       { .. } => true,
            ArtifactState::FileWindow { .. } => true,
            _                                => false,
        }
    }

    pub fn get_file_path(&self) -> &Path {
        self.file_path.as_path()
    }

    // TODO: Подумать над надобностью этой функции. Может, сигнатуру нужно переписать.
    /// Если данные на диске, то будет возвращён `None`.
    /// Если данные в оперативной памяти, то будет возвращён `Some`.
    pub fn next_mut_chunk_from_memory(&mut self) -> io::Result<Option<&mut [u8]>> {
        match &mut self.state {
            ArtifactState::File { .. } => Ok(None),
            
            ArtifactState::Memory { data, .. } => {
                let to_read_bytes = data.len().saturating_sub(self.cursor);
                let result = Ok(Some(&mut data[self.cursor..self.cursor + to_read_bytes]));
                self.cursor += to_read_bytes;
                result
            }
            
            ArtifactState::FileWindow { .. } => Ok(None),
        }
    }

    /// Возвращает `Ok(Some(Cow)) длиной <= chunk_size`, если данные можно прочесть.
    /// Если данные закончились (курсор уехал за пределы файла или массива), то вернётся `Ok(None)`.
    pub fn next_chunk(&mut self) -> io::Result<Option<Cow<'_, [u8]>>> {
        match &mut self.state {
            ArtifactState::File { file, .. } => {
                file.seek(SeekFrom::Start(self.cursor as u64))?;
                let mut buffer = vec![0; self.chunk_size.get()];
                let n = file.read(&mut buffer)?;

                if n == 0 {
                    return Ok(None);
                }

                self.cursor += n;
                buffer.truncate(n);

                Ok(Some(Cow::Owned(buffer)))
            }

            ArtifactState::Memory { data, .. } => {
                if self.cursor >= data.len() {
                    return Ok(None);
                }
                let start = self.cursor;
                let end = (start + self.chunk_size.get()).min(data.len());
                self.cursor = end;

                Ok(Some(Cow::Borrowed(&data[start..end])))
            }

            ArtifactState::FileWindow { file, base_offset, len } => {
                let remaining = (*len).saturating_sub(self.cursor as u64);

                if remaining == 0 {
                    return Ok(None);
                }

                let want = remaining.min(self.chunk_size.get() as u64) as usize;
                file.seek(SeekFrom::Start(*base_offset + self.cursor as u64))?;
                let mut buffer = vec![0; want];
                let n = file.read(&mut buffer)?;

                if n == 0 {
                    return Ok(None);
                }

                self.cursor += n;
                buffer.truncate(n);

                Ok(Some(Cow::Owned(buffer)))
            }
        }
    }

    /// Читает до `chunk_size` байт (или меньше, если `buf` меньше) с
    /// текущей позиции чтения. `0` означает конец данных.
    pub fn read_chunk_to(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let max_to_read = buf.len().min(self.chunk_size.get());
        let actual_buf = &mut buf[..max_to_read];

        match &mut self.state {
            ArtifactState::File { file, .. } => {
                file.seek(SeekFrom::Start(self.cursor as u64))?; // TODO: А нужно ли?

                let mut filled = 0;
                while filled < actual_buf.len() {
                    let n = file.read(&mut actual_buf[filled..])?;
                    if n == 0 { break; }
                    filled += n;
                }

                self.cursor += filled;
                Ok(filled)
            }
            
            ArtifactState::Memory { data, .. } => {
                let remaining_of_bytes = data.len().saturating_sub(self.cursor);
                let to_read_bytes = remaining_of_bytes.min(max_to_read);
                actual_buf[..to_read_bytes]
                    .copy_from_slice(&data[self.cursor..self.cursor + to_read_bytes]);
                self.cursor += to_read_bytes;
                Ok(to_read_bytes)
            }
            
            ArtifactState::FileWindow { file, base_offset, len } => {
                let remaining = (*len).saturating_sub(self.cursor as u64);
                let to_read = (remaining as usize).min(max_to_read);
                if to_read == 0 {
                    return Ok(0);
                }

                file.seek(SeekFrom::Start(*base_offset + self.cursor as u64))?;

                let mut filled = 0;
                while filled < to_read {
                    let n = file.read(&mut actual_buf[filled..to_read])?;
                    if n == 0 { break; }
                    filled += n;
                }

                self.cursor += filled;
                Ok(filled)
            }
        }
    }

    /// Обёртка над `read_chunk_to`: сама выделяет буфер размера
    /// `chunk_size` и возвращает `None` на конце данных.
    pub fn read_next_chunk_with_clone(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut buf = vec![0u8; self.chunk_size.get()];
        let filled = self.read_chunk_to(&mut buf)?;

        if filled == 0 {
            return Ok(None);
        }

        buf.truncate(filled);
        Ok(Some(buf))
    }

    /// Записывает ВЕСЬ `buf` с текущей позиции записи, при необходимости
    /// разбивая его на куски по `chunk_size` - частичная запись невозможна:
    /// либо весь буфер будет записан, либо вернётся ошибка.
    pub fn write_chunk_from(&mut self, buf: &[u8]) -> io::Result<()> {
        let mut offset = 0;

        while offset < buf.len() {
            let written = self.write_chunk_once_from(&buf[offset..])?;
            if written == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Artifact::write_chunk_from: нулевая запись", // TODO: А нужно ли?
                ));
            }
            offset += written;
        }

        Ok(())
    }

    pub fn set_cursor(&mut self, pos: usize) {
        self.cursor = pos;
    }

    pub fn rewind_cursor(&mut self) {
        self.cursor = 0;
    }

    /// Скопирует данные артефакта в Writable.
    pub fn copy_into<Writable: Write>(&mut self, dest: &mut Writable) -> io::Result<()> {
        while let Some(chunk) = self.read_next_chunk_with_clone()? {
            dest.write_all(&chunk)?;
        }
        Ok(())
    }

    // TODO: Да, это повод для макроса. Но мне лень.
    pub fn read_le_u8(&mut self) -> Result<u8, AppError> {
        Ok(u8::from_le_bytes(
            self.read_n_bytes()?.try_into().unwrap()
        ))
    }

    pub fn read_le_u16(&mut self) -> Result<u16, AppError> {
        Ok(u16::from_le_bytes(
            self.read_n_bytes()?.try_into().unwrap()
        ))
    }

    pub fn read_le_u32(&mut self) -> Result<u32, AppError> {
        Ok(u32::from_le_bytes(
            self.read_n_bytes()?.try_into().unwrap()
        ))
    }

    pub fn read_le_u64(&mut self) -> Result<u64, AppError> {
        Ok(u64::from_le_bytes(
            self.read_n_bytes()?.try_into().unwrap()
        ))
    }

    fn should_artifact_be_placed_on_disk(artifact: &Artifact) -> bool {
        artifact.get_payload_size() > MEMORY_BUDGET_IN_BYTES.load(Ordering::Acquire) as usize
    }

    // Несмотря на свою устрашающую сигнатуру, она делает код чище. Я удивлён
    fn read_n_bytes<const N: usize>(&mut self) -> Result<[u8; N], AppError> {
        let mut buffer: [u8; N] = [0; N];
        let mut filled: usize = 0;

        while filled < N {
            let read_bytes: usize = self.read_chunk_to(&mut buffer[filled..])?;

            if read_bytes == 0 {
                return Err(ArchiveError::UnexpectedEndOfData.into());
            }

            filled += read_bytes;
        }

        Ok(buffer)
    }
    
    /// Записывает не больше `chunk_size` байт за один вызов. Двигает курсор.
    fn write_chunk_once_from(&mut self, buf: &[u8]) -> io::Result<usize> {
        let max_to_write = buf.len().min(self.chunk_size.get());
        let actual_buf = &buf[..max_to_write];

        match &mut self.state {
            ArtifactState::File { file, size } => {
                file.seek(SeekFrom::Start(self.cursor as u64))?;
                let written_bytes = file.write(actual_buf)?;
                self.cursor += written_bytes;

                let new_end = self.cursor;
                if new_end > *size {
                    *size = new_end;
                    file.set_len(new_end as u64)?;
                }
                Ok(written_bytes)
            }

            ArtifactState::Memory { data, budget_guard: maybe_budget_guard } => {
                let new_len = self.cursor + max_to_write;

                if new_len > data.len() {
                    let additional_bytes: i64 = new_len as i64 - data.len() as i64;

                    if maybe_budget_guard
                        .as_mut()
                        .is_none_or(|guard| guard.try_grow(additional_bytes))
                    {
                        data.resize(new_len, 0); // Расширим вектор нулями
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

                        return self.write_chunk_once_from(buf);
                    }
                }

                let end = self.cursor + max_to_write;
                data[self.cursor..end].copy_from_slice(actual_buf);
                self.cursor += max_to_write;

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
                    let _: io::Result<()> = fs::remove_file(path);
                    //println!("The artifact [FILE] has been removed: {:?}. Path: {:?}", result, self.file_path);
                } else {
                    //println!("The artifact [FILE] has been dropped, but the file is still exists. Path: {:?}", self.file_path);
                }
            }

            ArtifactState::Memory { data: _data, .. } => {
                //println!("The artifact [MEMORY]  has been dropped: {} bytes", data.len());
                //super::memory_budget::show_memory_bar();
            }

            ArtifactState::FileWindow { len: _len, .. } => {
                //println!("The artifact [FILE WINDOW] has been dropped: {} bytes", len);
            }
        }
    }
}


impl Write for Artifact {
    fn write(&mut self, source: &[u8]) -> io::Result<usize> {
        if source.is_empty() {
            return Ok(0);
        }

        self.write_chunk_once_from(source)
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.state {
            ArtifactState::File { file, .. } => file.flush(),
            ArtifactState::Memory { .. } | ArtifactState::FileWindow { .. } => Ok(()),
        }
    }
}


impl Read for Artifact {
    fn read(&mut self, dest: &mut [u8]) -> io::Result<usize> {
        let limit = dest
            .len()
            .min(self.chunk_size.get())
            .min(usize::MAX - self.cursor);

        if limit == 0 {
            return Ok(0);
        }

        let dest: &mut [u8] = &mut dest[..limit];

        let read_bytes: usize = match &mut self.state {
            ArtifactState::File { file, .. } => {
                file.seek(SeekFrom::Start(self.cursor as u64))?;
                file.read(dest)?
            }

            ArtifactState::Memory { data, .. } => {
                if self.cursor >= data.len() {
                    return Ok(0);
                }

                let count = dest.len().min(data.len() - self.cursor);
                dest[..count].copy_from_slice(
                    &data[self.cursor..self.cursor + count],
                );
                count
            }

            ArtifactState::FileWindow { file, base_offset, len } => {
                let remaining = len.saturating_sub(self.cursor as u64);
                let count = remaining.min(dest.len() as u64) as usize;

                if count == 0 {
                    return Ok(0);
                }

                let offset = base_offset
                    .checked_add(self.cursor as u64)
                    .ok_or(io::ErrorKind::InvalidInput)?;

                file.seek(SeekFrom::Start(offset))?;
                file.read(&mut dest[..count])?
            }
        };

        self.cursor += read_bytes;
        Ok(read_bytes)
    }
}


impl Seek for Artifact {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let position: u64 = match position {
            SeekFrom::Start(offset) => offset,

            SeekFrom::Current(offset) => (self.cursor as u64)
                .checked_add_signed(offset)
                .ok_or(io::ErrorKind::InvalidInput)?,

            SeekFrom::End(offset) => {
                let len = match &self.state {
                    ArtifactState::File { file, .. } => file.metadata()?.len(),
                    ArtifactState::Memory { data, .. } => data.len() as u64,
                    ArtifactState::FileWindow { len, .. } => *len,
                };

                len.checked_add_signed(offset)
                    .ok_or(io::ErrorKind::InvalidInput)?
            }
        };

        self.cursor = usize::try_from(position)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

        Ok(position)
    }

    fn stream_position(&mut self) -> io::Result<u64> {
        Ok(self.cursor as u64)
    }
}


fn make_numbered_temp_file_path(old_path: &Path, suffix: &str) -> PathBuf {
    static NUM_MAP: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();

    let filename: String = old_path
        .file_stem()
        .expect("Нельзя извлечь основу имени файла (stem)!")
        .to_string_lossy()
        .to_string();

    let mut map: MutexGuard<HashMap<PathBuf, usize>> = NUM_MAP
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


#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::Artifact;
    use super::super::memory_budget::TestMemoryBudget;

    static TEST_FILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TestSourceFile {
        path: PathBuf,
    }

    impl TestSourceFile {
        fn new(contents: &[u8]) -> Self {
            let id = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "rustarch_artifact_unit_{}_{}",
                std::process::id(),
                id,
            ));
            fs::write(&path, contents).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            self.path.as_path()
        }
    }

    impl Drop for TestSourceFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    #[test]
    fn derived_artifact_starts_on_disk_when_source_exceeds_budget() {
        let budget = TestMemoryBudget::new(2);
        let source_file = TestSourceFile::new(&[1, 2, 3]);
        let source = Artifact::from_file(source_file.path()).unwrap();
        let mut derived = Artifact::new_with_temp_file_suffix(&source, "starts_on_disk").unwrap();

        assert!(derived.on_disk());
        assert_eq!(budget.remaining(), 2);

        derived.write_chunk_from(&[4, 5, 6]).unwrap();
        derived.rewind_cursor();

        assert_eq!(derived.get_payload_size(), 3);
        assert_eq!(derived.read_next_chunk_with_clone().unwrap().unwrap(), [4, 5, 6]);
        assert!(derived.read_next_chunk_with_clone().unwrap().is_none());
    }

    #[test]
    fn artifact_spills_to_disk_and_releases_reserved_memory() {
        let budget = TestMemoryBudget::new(4);
        let source_file = TestSourceFile::new(&[0]);
        let source = Artifact::from_file(source_file.path()).unwrap();
        let mut derived = Artifact::new_with_temp_file_suffix(&source, "spills_to_disk").unwrap();

        assert!(!derived.on_disk());

        derived.write_chunk_from(&[1, 2, 3]).unwrap();
        assert_eq!(budget.remaining(), 1);

        derived.write_chunk_from(&[4, 5]).unwrap();
        assert!(derived.on_disk());
        assert_eq!(budget.remaining(), 4);

        derived.rewind_cursor();
        assert_eq!(derived.get_payload_size(), 5);
        assert_eq!(
            derived.read_next_chunk_with_clone().unwrap().unwrap(),
            [1, 2, 3, 4, 5],
        );
        assert!(derived.read_next_chunk_with_clone().unwrap().is_none());
    }
}
