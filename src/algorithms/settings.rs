use super::ids::{CompressionId, CipherId, FecId};


/// Полный набор алгоритмов, применённых к одному файлу.
/// Каждая запись архива (бывший файл) хранит свою копию PipelineSettings.
#[derive(Debug, Clone, Copy)]
pub struct PipelineSettings {
    pub compression: CompressionId,
    pub cipher: CipherId,
    pub fec: FecId,
}


impl PipelineSettings {
    pub const fn default() -> Self {
        Self {
            compression: CompressionId::NoCompression,
            cipher: CipherId::NoCipher,
            fec: FecId::NoFec,
        }
    }
}


impl Default for PipelineSettings {
    fn default() -> Self {
        Self::default()
    }
}
