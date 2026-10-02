use crate::error::AppError;
use crate::archiver::ArchiveError;
use super::{Compressor, compression};
use super::{Cipher, crypto};
use super::{ErrorCorrectionCode, fec};


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionId {
    NoCompression = 0,
    Huffman = 1,
    LZSS = 2,
    Deflate = 3,
}


impl CompressionId {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn get(self) -> Box<dyn Compressor> {
        use CompressionId::*;
        match self {
            NoCompression => Box::new(compression::NoneCompressor),
            Huffman => Box::new(compression::HuffmanCompressor),
            LZSS => Box::new(compression::LzssCompressor),
            Deflate => Box::new(compression::DeflateCompressor),
        }
    }

    pub fn from_u8(v: u8) -> Result<Self, AppError> {
        use CompressionId::*;
        match v {
            0 => Ok(NoCompression),
            1 => Ok(Huffman),
            2 => Ok(LZSS),
            3 => Ok(Deflate),
            other => Err(ArchiveError::UnknownCompressionId { id: other }.into()),
        }
    }
    
    pub const fn as_str(self) -> &'static str {
        match self {
            CompressionId::NoCompression => "none",
            CompressionId::Huffman => "huffman",
            CompressionId::LZSS => "lzss",
            CompressionId::Deflate => "deflate",
        }
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherId {
    NoCipher = 0,
    Xor = 1,
}

impl CipherId {
    pub const fn as_u8(self) -> u8 {
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
            other => Err(ArchiveError::UnknownCipherId { id: other }.into()),
        }
    }
    
    pub const fn as_str(self) -> &'static str {
        match self {
            CipherId::NoCipher => "none",
            CipherId::Xor => "xor",
        }
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FecId {
    NoFec = 0,
    ReedSolomon = 1,
}

impl FecId {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn get(self) -> Box<dyn ErrorCorrectionCode> {
        use FecId::*;
        match self {
            NoFec => Box::new(fec::NoneFec),
            ReedSolomon => Box::new(fec::ReedSolomonCode),
        }
    }

    pub fn from_u8(v: u8) -> Result<Self, AppError> {
        use FecId::*;
        match v {
            0 => Ok(NoFec),
            1 => Ok(ReedSolomon),
            other => Err(ArchiveError::UnknownFecId { id: other }.into()),
        }
    }
    
    pub const fn as_str(self) -> &'static str {
        match self {
            FecId::NoFec => "none",
            FecId::ReedSolomon => "reed-solomon",
        }
    }
}
