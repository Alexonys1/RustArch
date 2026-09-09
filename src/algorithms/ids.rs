use crate::error::AppError;
use super::{compression, Compressor};
use super::{crypto, Cipher};
use super::{fec, ErrorCorrectionCode};


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionId {
    NoCompression = 0,
    Rle = 1,
    Huffman = 2,
    Lz77 = 3,
}

impl CompressionId {
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn get(self) -> Box<dyn Compressor> {
        match self {
            CompressionId::NoCompression => Box::new(compression::NoneCompressor),
            CompressionId::Rle => Box::new(compression::rle::RleCompressor),
            CompressionId::Huffman => Box::new(compression::huffman::HuffmanCompressor),
            CompressionId::Lz77 => Box::new(compression::lz77::Lz77Compressor::default()),
        }
    }

    pub fn from_u8(v: u8) -> Result<Self, AppError> {
        match v {
            0 => Ok(CompressionId::NoCompression),
            1 => Ok(CompressionId::Rle),
            2 => Ok(CompressionId::Huffman),
            3 => Ok(CompressionId::Lz77),
            other => Err(AppError::CorruptArchive(format!(
                "неизвестный compression_id: {other}"
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
        match self {
            CipherId::NoCipher => Box::new(crypto::NoneCipher),
            CipherId::Xor => Box::new(crypto::xor::XorCipher),
        }
    }

    pub fn from_u8(v: u8) -> Result<Self, AppError> {
        match v {
            0 => Ok(CipherId::NoCipher),
            1 => Ok(CipherId::Xor),
            other => Err(AppError::CorruptArchive(format!(
                "неизвестный cipher_id: {other}"
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
        match self {
            FecId::NoFec => Box::new(fec::NoneFec),
            FecId::Hamming7_4 => Box::new(fec::hamming::HammingCode::new_7_4()),
            FecId::Hamming15_11 => Box::new(fec::hamming::HammingCode::new_15_11()),
        }
    }

    pub fn from_u8(v: u8) -> Result<Self, AppError> {
        match v {
            0 => Ok(FecId::NoFec),
            1 => Ok(FecId::Hamming7_4),
            2 => Ok(FecId::Hamming15_11),
            other => Err(AppError::CorruptArchive(format!(
                "Неизвестный fec_id: {other}"
            ))),
        }
    }
}
