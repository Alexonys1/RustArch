pub mod rle;
pub mod huffman;
pub mod lz77;

use crate::error::AppError;
use crate::archiver::Artifact;
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
