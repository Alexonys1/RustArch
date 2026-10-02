#[derive(Debug, thiserror::Error)]
pub enum HuffmanError {
    #[error("Число символов {count} превышает размер алфавита")]
    TooManySymbols { count: usize },

    #[error("Символ {symbol:#04x} имеет нулевую частоту")]
    ZeroFrequency { symbol: u8 },

    #[error("Символ {symbol:#04x} повторяется в таблице частот")]
    DuplicateSymbol { symbol: u8 },

    #[error("Переполнение суммы частот")]
    FrequencySumOverflow,

    #[error("Пустой поток должен храниться без сжатия")]
    EmptyEncodedStream,
}
