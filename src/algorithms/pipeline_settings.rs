use crate::error::AppError;
use crate::archiver::ArchiveError;
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
            return Err(ArchiveError::UnknownPipelineFlags { flags: descriptor & RESERVED_MASK }.into());
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


#[cfg(test)]
mod tests {
    use crate::error::AppErrorKind;

    use super::{CipherId, CompressionId, FecId, PipelineSettings};

    #[test]
    fn descriptor_uses_one_nibble_per_algorithm() {
        assert_eq!(PipelineSettings::default().to_descriptor(), 0x0000);

        let settings = PipelineSettings {
            compression: CompressionId::Deflate,
            cipher: CipherId::Xor,
            fec: FecId::ReedSolomon,
        };

        assert_eq!(settings.to_descriptor(), 0x0113);
    }

    #[test]
    fn every_supported_pipeline_round_trips_through_descriptor() {
        let compressions = [
            CompressionId::NoCompression,
            CompressionId::Huffman,
            CompressionId::LZSS,
            CompressionId::Deflate,
        ];
        let ciphers = [CipherId::NoCipher, CipherId::Xor];
        let fecs = [FecId::NoFec, FecId::ReedSolomon];

        for compression in compressions {
            for cipher in ciphers {
                for fec in fecs {
                    let settings = PipelineSettings {
                        compression,
                        cipher,
                        fec,
                    };
                    let descriptor = settings.to_descriptor();

                    assert_eq!(PipelineSettings::from_descriptor(descriptor).unwrap(), settings);
                }
            }
        }
    }

    #[test]
    fn descriptor_rejects_reserved_bits_and_unknown_algorithm_ids() {
        for descriptor in [0x1000, 0x000F, 0x00F0, 0x0F00] {
            let error = PipelineSettings::from_descriptor(descriptor).unwrap_err();
            assert!(matches!(error.kind(), AppErrorKind::Archive(_)));
        }
    }
}
