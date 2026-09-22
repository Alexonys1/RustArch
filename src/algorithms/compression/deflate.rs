use crate::archiver::{ArchivedArtifactEntry, Artifact};
use crate::error::AppError;
use super::{CompressionId, Compressor};
use super::{HuffmanCompressor, LzssCompressor};


pub struct DeflateCompressor;

impl Compressor for DeflateCompressor {
    fn compress(&self, artifact: Artifact) -> Result<(Artifact, CompressionId), AppError> {
        let (lzss_stage, lzss_compression_id) = LzssCompressor.compress(artifact)?;
        let (huffman_stage, huffman_compression_id) = HuffmanCompressor.compress(lzss_stage)?;

        use CompressionId::*;
        let result_compression_id: CompressionId = match (lzss_compression_id, huffman_compression_id) {
            (LZSS,          Huffman)       => Deflate,
            (NoCompression, Huffman)       => Huffman,
            (LZSS,          NoCompression) => LZSS,
            (NoCompression, NoCompression) => NoCompression,
            _ => unreachable!("Алгоритмы сжатия в Deflate возвращают не себя и не NoCompression!"),
        };

        Ok((huffman_stage, result_compression_id))
    }

    fn decompress(&self, artifact: Artifact, entry: &ArchivedArtifactEntry) -> Result<Artifact, AppError> {
        // Huffman восстанавливает размер промежуточного LZSS-потока по сумме
        // частот, а LZSS берет конечный размер файла из entry.

        let huffman_stage = HuffmanCompressor.decompress(artifact, entry)?;
        let lzss_stage = LzssCompressor.decompress(huffman_stage, entry)?;

        Ok(lzss_stage)
    }

    fn id(&self) -> CompressionId {
        CompressionId::Deflate
    }
}
