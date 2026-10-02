#[derive(Debug, thiserror::Error)]
pub enum XorError {
    #[error("Ключ не может быть пустым")]
    EmptyKey,
}
