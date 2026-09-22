use crate::error::AppError;
use super::ids::{CipherId, CompressionId, FecId};


/// Полный набор алгоритмов, применённых к одному файлу.
/// Каждая запись архива (бывший файл) хранит свой PipelineSettings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    pub const fn to_descriptor(self) -> u16 {
        self.compression.as_u8() as u16
            | (self.cipher.as_u8() as u16) << CIPHER_SHIFT
            | (self.fec.as_u8() as u16) << FEC_SHIFT
    }

    pub fn from_descriptor(descriptor: u16) -> Result<Self, AppError> {
        if descriptor & RESERVED_MASK != 0 {
            return Err(AppError::CorruptArchive(format!(
                "Неизвестные флаги pipeline: {:#x}",
                descriptor & RESERVED_MASK
            )));
        }

        Ok(Self {
            compression: CompressionId::from_u8((descriptor & COMPRESSION_MASK) as u8)?,
            cipher: CipherId::from_u8(((descriptor & CIPHER_MASK) >> CIPHER_SHIFT) as u8)?,
            fec: FecId::from_u8(((descriptor & FEC_MASK) >> FEC_SHIFT) as u8)?,
        })
    }
}


impl Default for PipelineSettings {
    fn default() -> Self {
        Self::default()
    }
}

const COMPRESSION_MASK: u16 = 0x000F;
const CIPHER_MASK: u16 = 0x00F0;
const FEC_MASK: u16 = 0x0F00;
const RESERVED_MASK: u16 = 0xF000;
const CIPHER_SHIFT: u32 = 4;
const FEC_SHIFT: u32 = 8;
