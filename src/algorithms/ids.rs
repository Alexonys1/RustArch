use crate::error::AppError;
use super::{Compressor, compression};
use super::{Cipher, crypto};
use super::{ErrorCorrectionCode, fec};


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionId {
    NoCompression = 0,
    RLE = 1,
    Huffman = 2,
    LZ77 = 3,
    LZSS = 4,
    Deflate = 5,
}


impl CompressionId {
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn get(self) -> Box<dyn Compressor> {
        use CompressionId::*;
        match self {
            NoCompression => Box::new(compression::NoneCompressor),
            RLE => Box::new(compression::RleCompressor),
            Huffman => Box::new(compression::HuffmanCompressor),
            LZ77 => Box::new(compression::Lz77Compressor),
            LZSS => Box::new(compression::LzssCompressor),
            Deflate => Box::new(compression::DeflateCompressor),
        }
    }

    pub fn from_u8(v: u8) -> Result<Self, AppError> {
        use CompressionId::*;
        match v {
            0 => Ok(NoCompression),
            1 => Ok(RLE),
            2 => Ok(Huffman),
            3 => Ok(LZ77),
            4 => Ok(LZSS),
            5 => Ok(Deflate),
            other => Err(AppError::CorruptArchive(format!(
                "Неизвестный compression_id: {other}"
            ))),
        }
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherId {
    NoCipher = 0,
    Xor = 1,
}

impl CipherId {
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn get(self) -> Box<dyn Cipher> {
        use CipherId::*;
        match self {
            NoCipher => Box::new(crypto::NoneCipher),
            Xor => Box::new(crypto::XorCipher),
        }
    }

    pub fn from_u8(v: u8) -> Result<Self, AppError> {
        use CipherId::*;
        match v {
            0 => Ok(NoCipher),
            1 => Ok(Xor),
            other => Err(AppError::CorruptArchive(format!(
                "Неизвестный cipher_id: {other}"
            ))),
        }
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FecId {
    NoFec = 0,
    Hamming7_4 = 1,
    Hamming15_11 = 2,
}

impl FecId {
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn get(self) -> Box<dyn ErrorCorrectionCode> {
        use FecId::*;
        match self {
            NoFec => Box::new(fec::NoneFec),
            _hamming => Box::new(fec::HammingCode::new_7_4()),
        }
    }

    pub fn from_u8(v: u8) -> Result<Self, AppError> {
        use FecId::*;
        match v {
            0 => Ok(NoFec),
            1 => Ok(Hamming7_4),
            2 => Ok(Hamming15_11),
            other => Err(AppError::CorruptArchive(format!(
                "Неизвестный fec_id: {other}"
            ))),
        }
    }
}
