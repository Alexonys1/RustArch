use std::path::PathBuf;
use std::string::FromUtf8Error;


#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("Некорректный размер таблиц")]
    InvalidTableSize,
    #[error("Количество файлов {count} превышает u32::MAX")]
    TooManyFiles { count: usize },
    #[error("Количество директорий {count} превышает u32::MAX")]
    TooManyDirectories { count: usize },
    #[error("Слишком много файловых записей: {count}")]
    FileCountExceedsPlatformLimit { count: u32 },
    #[error("Слишком много записей директорий: {count}")]
    DirectoryCountExceedsPlatformLimit { count: u32 },
    #[error("Размер таблиц не совпадает с количеством записей")]
    TableSizeMismatch,
    #[error("Архив слишком короткий для footer")]
    ArchiveTooShort,
    #[error("Неверная сигнатура footer архива")]
    InvalidSignature,
    #[error("Неподдерживаемая версия формата: {version} (ожидалась {expected})")]
    UnsupportedVersion { version: u16, expected: u16 },
    #[error("Неизвестные флаги архива: {flags:#x}")]
    UnknownArchiveFlags { flags: u16 },
    #[error("Неизвестные флаги pipeline: {flags:#x}")]
    UnknownPipelineFlags { flags: u16 },
    #[error("Неизвестный compression_id: {id}")]
    UnknownCompressionId { id: u8 },
    #[error("Неизвестный cipher_id: {id}")]
    UnknownCipherId { id: u8 },
    #[error("Неизвестный fec_id: {id}")]
    UnknownFecId { id: u8 },
    #[error("Некорректные границы таблиц архива")]
    InvalidTableBounds,
    #[error("Переполнение размера таблиц")]
    TableSizeOverflow,
    #[error("Путь слишком длинный: {length} байт")]
    PathTooLong { length: usize },
    #[error("Путь записи не является валидным UTF-8")]
    InvalidPathUtf8(#[source] FromUtf8Error),
    #[error("Запись выходит за пределы своей таблицы")]
    EntryOutsideTable,
    #[error("Переполнение payload диапазона '{path}'")]
    PayloadRangeOverflow { path: String },
    #[error("Payload '{path}' выходит за пределы payload-секции")]
    PayloadOutsideSection { path: String },
    #[error("Payload'ы не образуют непрерывную непересекающуюся секцию")]
    InvalidPayloadLayout,
    #[error("Конец payload-секции не совпадает с началом таблиц")]
    PayloadEndMismatch,
    #[error("Пустой файл '{path}' содержит ненулевой payload")]
    EmptyFileHasPayload { path: String },
    #[error("Пустой файл '{path}' содержит ненулевой pipeline")]
    EmptyFileHasPipeline { path: String },
    #[error("Пустой файл '{path}' содержит некорректную CRC32-заглушку")]
    InvalidEmptyFileChecksum { path: String },
    #[error("Путь записи содержит '..' - потенциально небезопасный архив: '{path}'")]
    UnsafePath { path: String },
    #[error("Неожиданный конец потока при чтении")]
    UnexpectedEndOfData,
}

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("'{}' не найден", path.display())]
    SourceNotFound { path: PathBuf },
    #[error("'{}' не является файлом архива", path.display())]
    ArchiveNotFile { path: PathBuf },
    #[error("Исходный файл '{}' и целевой архив '{}' не могут быть одним путём", input.display(), output.display())]
    SameInputAndOutput { input: PathBuf, output: PathBuf },
    #[error("Целевой архив '{}' не может находиться внутри исходной директории '{}'", archive.display(), directory.display())]
    ArchiveInsideSource { archive: PathBuf, directory: PathBuf },
    #[error("Не удалось определить имя файла из пути '{}'", path.display())]
    MissingFileName { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("Паника в рабочем потоке")]
    WorkerPanicked,
    #[error("Поток записи архива аварийно завершился")]
    WriterPanicked,
    #[error("Очередь записи архива недоступна")]
    WriterQueueClosed,
}
