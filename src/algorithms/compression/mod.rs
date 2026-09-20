mod rle;
mod deflate;
mod lz;
mod utils;
mod huffman;

pub use deflate::DeflateCompressor;
pub use huffman::HuffmanCompressor;
pub use lz::Lz77Compressor;
pub use lz::LzssCompressor;
pub use rle::RleCompressor;

use crate::archiver::Artifact;
use crate::error::AppError;
use super::ids::CompressionId;


pub trait Compressor {
    /// Читает `artifact` от текущей позиции чтения до конца, отдаёт
    /// новый артефакт со сжатыми данными.
    fn compress(&self, artifact: Artifact) -> Result<Artifact, AppError>;

    /// Читает `artifact` от текущей позиции чтения до конца, отдаёт
    /// новый артефакт со разжатыми данными.
    fn decompress(&self, artifact: Artifact) -> Result<Artifact, AppError>;
    fn id(&self) -> CompressionId;
}


pub struct NoneCompressor;


impl Compressor for NoneCompressor {
    fn compress(&self, artifact: Artifact) -> Result<Artifact, AppError> {
        Ok(artifact)
    }

    fn decompress(&self, artifact: Artifact) -> Result<Artifact, AppError> {
        Ok(artifact)
    }

    fn id(&self) -> CompressionId {
        CompressionId::NoCompression
    }
}
