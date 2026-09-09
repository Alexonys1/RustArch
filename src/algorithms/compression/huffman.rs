use crate::error::AppError;
use crate::archiver::Artifact;
use super::{CompressionId, Compressor};


pub struct HuffmanCompressor;


impl Compressor for HuffmanCompressor {
    fn compress(&self, _artifact: Artifact) -> Result<Artifact, AppError> {
        Err(AppError::NotImplemented("Huffman::compress"))
    }
    fn decompress(&self, _artifact: Artifact) -> Result<Artifact, AppError> {
        Err(AppError::NotImplemented("Huffman::decompress"))
    }
    fn id(&self) -> CompressionId {
        CompressionId::Huffman
    }
}
