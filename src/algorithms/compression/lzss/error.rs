#[derive(Debug, thiserror::Error)]
pub enum LzssError {
    #[error("Размер окна {size} вне допустимого диапазона 1..={max}")]
    InvalidWindowSize { size: usize, max: usize },

    #[error("Размер буфера предпросмотра {size} вне допустимого диапазона 1..={max}")]
    InvalidLookaheadSize { size: usize, max: usize },

    #[error("Переполнение при вычислении размера скользящего окна")]
    WindowSizeOverflow,

    #[error("Не удалось зарезервировать {bytes} байт под скользящее окно: бюджет памяти исчерпан")]
    WindowMemoryBudgetExceeded { bytes: usize },

    #[error("Не удалось зарезервировать {bytes} байт под хэш-таблицу: бюджет памяти исчерпан")]
    HashTableMemoryBudgetExceeded { bytes: usize },

    #[error("Недостаточная ёмкость скользящего окна")]
    InsufficientWindowCapacity,

    #[error("Неожиданный конец потока при чтении токена")]
    UnexpectedEndOfToken,

    #[error("Некорректная длина литерального блока: {length}")]
    InvalidLiteralLength { length: usize },

    #[error("Некорректная ссылка назад: смещение {offset}, длина {length}")]
    InvalidBackReference { offset: usize, length: usize },

    #[error("Некорректное смещение ссылки назад: {offset}")]
    InvalidBackReferenceOffset { offset: usize },
}
