mod rle;
mod deflate;
mod utils;
mod huffman;
mod lzss;

pub use deflate::DeflateCompressor;
pub use huffman::HuffmanCompressor;
pub use lzss::LzssCompressor;
pub use rle::RleCompressor;

use super::ids::CompressionId;
use crate::archiver::Artifact;
use crate::error::AppError;


pub trait Compressor {
    /// Читает `artifact` от текущей позиции чтения до конца, отдаёт
    /// новый артефакт со сжатыми данными.
    fn compress(&self, artifact: Artifact) -> Result<(Artifact, CompressionId), AppError>;

    /// Читает `artifact` от текущей позиции чтения до конца, отдаёт
    /// новый артефакт со разжатыми данными.
    fn decompress(&self, artifact: Artifact) -> Result<Artifact, AppError>;
    fn id(&self) -> CompressionId;
}


pub struct NoneCompressor;


impl Compressor for NoneCompressor {
    fn compress(&self, artifact: Artifact) -> Result<(Artifact, CompressionId), AppError> {
        Ok((artifact, CompressionId::NoCompression))
    }

    fn decompress(&self, artifact: Artifact) -> Result<Artifact, AppError> {
        Ok(artifact)
    }

    fn id(&self) -> CompressionId {
        CompressionId::NoCompression
    }
}
