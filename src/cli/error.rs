use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("Неизвестная команда '{name}'")]
    UnknownCommand { name: String },
    #[error("Неизвестная опция {command}: {name}")]
    UnknownOption { command: &'static str, name: String },
    #[error("Имя команды не является валидной UTF-8 строкой")]
    InvalidCommandUtf8,
    #[error("Имя опции не является валидной UTF-8 строкой")]
    InvalidOptionNameUtf8,
    #[error("Значение {option} не является UTF-8 строкой")]
    InvalidOptionValueUtf8 { option: String },
    #[error("После {option} ожидается значение")]
    MissingOptionValue { option: String },
    #[error("Опция {option} получила пустое значение")]
    EmptyOptionValue { option: String },
    #[error("Опция {option} указана несколько раз")]
    DuplicateOption { option: String },
    #[error("pack ожидает два пути: <SOURCE> <ARCHIVE>")]
    InvalidPackArguments,
    #[error("unpack ожидает два пути: <ARCHIVE> <DESTINATION>")]
    InvalidUnpackArguments,
    #[error("list ожидает один путь: <ARCHIVE>")]
    InvalidListArguments,
    #[error("Неизвестный алгоритм сжатия '{name}'")]
    UnknownCompression { name: String },
    #[error("Неизвестный шифр '{name}'")]
    UnknownCipher { name: String },
    #[error("Неизвестный FEC '{name}'")]
    UnknownFec { name: String },
    #[error("Hamming пока не подключён к pipeline; используйте none или reed-solomon")]
    UnsupportedHamming,
    #[error("--key-hex должен содержать ненулевое чётное число hex-цифр")]
    InvalidHexKeyLength,
    #[error("Недопустимый символ в --key-hex: '{}'", char::from(*byte))]
    InvalidHexDigit { byte: u8 },
    #[error("Для --cipher xor требуется --key, --key-hex или --key-file")]
    MissingXorKey,
    #[error("Ключ указан, но шифрование выключено; добавьте --cipher xor")]
    KeyWithoutCipher,
    #[error("Не удалось прочитать файл ключа '{}': {source}", path.display())]
    KeyFileRead { path: PathBuf, #[source] source: io::Error },
}
