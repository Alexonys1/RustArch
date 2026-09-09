use crate::error::AppError;
use crate::archiver::Artifact;
use super::{CompressionId, Compressor};


pub struct Lz77Compressor {
    pub window_size: usize,
    pub lookahead_size: usize,
}


impl Default for Lz77Compressor {
    fn default() -> Self {
        Self { window_size: 4096, lookahead_size: 18 }
    }
}


impl Compressor for Lz77Compressor {
    fn compress(&self, _artifact: Artifact) -> Result<Artifact, AppError> {
        Err(AppError::NotImplemented("Lz77::compress"))
    }
    fn decompress(&self, _artifact: Artifact) -> Result<Artifact, AppError> {
        Err(AppError::NotImplemented("Lz77::decompress"))
    }
    fn id(&self) -> CompressionId {
        CompressionId::Lz77
    }
}
