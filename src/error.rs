use std::fmt;
use std::io;


/// Единый тип ошибки на всё приложение. Все подсистемы (сжатие, крипто, FEC,
/// формат архива, CLI) заворачивают свои ошибки сюда через `From`, поэтому
/// в коде пайплайна можно свободно использовать `?` независимо от того,
/// в каком модуле реализована конкретная функция.
#[derive(Debug)]
pub enum AppError {
    /// Ошибка чтения/записи файла (диск, права, нехватка места и т.п.)
    Io(io::Error),

    /// Файл архива повреждён или имеет несовместимый формат
    /// (неверная сигнатура, битые метаданные, обрыв на середине таблицы).
    CorruptArchive(String),

    /// Ошибка конкретного алгоритма сжатия/декомпрессии.
    Compression(String),

    /// Ошибка шифрования/расшифровки (например, некорректный ключ).
    Crypto(String),

    /// Ошибка помехоустойчивого кодирования/декодирования.
    Fec(String),

    /// CRC32 распакованных данных не совпал с сохранённым в архиве -
    /// сигнал о повреждении данных или неверном ключе шифрования.
    ChecksumMismatch { path: String },

    /// Алгоритм выбран, но его реализация ещё не написана (Huffman, LZ77,
    /// Hamming - задел под вашу часть работы). Это НЕ баг, а явная заглушка.
    NotImplemented(&'static str),

    /// Ошибка использования CLI (неверные аргументы и т.п.)
    CLIUsage(String),
}


impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Io(e) => write!(f, "ошибка ввода-вывода: {e}"),
            AppError::CorruptArchive(msg) => write!(f, "архив повреждён: {msg}"),
            AppError::Compression(msg) => write!(f, "ошибка сжатия: {msg}"),
            AppError::Crypto(msg) => write!(f, "ошибка шифрования: {msg}"),
            AppError::Fec(msg) => write!(f, "ошибка помехоустойчивого кодирования: {msg}"),
            AppError::ChecksumMismatch { path } => {
                write!(f, "контрольная сумма не совпала для '{path}': файл повреждён или неверный ключ")
            }
            AppError::NotImplemented(name) => {
                write!(f, "алгоритм '{name}' ещё не реализован - это заготовка под вашу реализацию")
            }
            AppError::CLIUsage(msg) => write!(f, "ошибка использования: {msg}"),
        }
    }
}


impl std::error::Error for AppError {}


impl From<io::Error> for AppError {
    fn from(e: io::Error) -> Self {
        AppError::Io(e)
    }
}
