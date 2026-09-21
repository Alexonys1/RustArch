use super::{HuffmanCompressor, LzssCompressor};
use super::{CompressionId, Compressor};
use crate::archiver::Artifact;
use crate::error::AppError;


/// Составная схема LZSS + byte-wise Huffman. Это намеренно не заявляется
/// как совместимый с RFC 1951 поток: настоящий DEFLATE кодирует литералы,
/// длины и расстояния общей битовой грамматикой, а не сжимает байтовое
/// представление LZSS вторым независимым этапом.
///
/// Huffman здесь нельзя самовольно пропустить даже при невыгодном результате:
/// декодер всегда выполняет оба этапа, а отдельного признака bypass в формате
/// нет. Поэтому используется `compress_always`.
pub struct DeflateCompressor;


impl Compressor for DeflateCompressor {
    fn compress(&self, artifact: Artifact) -> Result<(Artifact, CompressionId), AppError> {
        let (lzss_stage, _) = LzssCompressor.compress(artifact)?;
        let huffman_stage = HuffmanCompressor.compress_always(lzss_stage)?;
        
        Ok((huffman_stage, CompressionId::Deflate))
    }

    fn decompress(&self, artifact: Artifact) -> Result<Artifact, AppError> {
        let huffman_stage = HuffmanCompressor.decompress(artifact)?;
        let lzss_stage = LzssCompressor.decompress(huffman_stage)?;
        Ok(lzss_stage)
    }

    fn id(&self) -> CompressionId {
        CompressionId::Deflate
    }
}
