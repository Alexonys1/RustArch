#[derive(Debug, thiserror::Error)]
pub enum ReedSolomonError {
    #[error("Неожиданный конец потока при чтении блока")]
    UnexpectedEndOfBlock,

    #[error("Некорректная длина потока {encoded_size}: остаток блока {shortened_size} должен быть 0 или 33..=254 байт")]
    InvalidEncodedLength { encoded_size: usize, shortened_size: usize },

    #[error("Блок {block_number} из {blocks_count} содержит неисправимые ошибки")]
    UncorrectableBlock { block_number: usize, blocks_count: usize },
}
