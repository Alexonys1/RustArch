use super::{HuffmanCompressor, LzssCompressor};
use super::{CompressionId, Compressor};
use super::Lz77Compressor;
use crate::archiver::Artifact;
use crate::error::AppError;


/// Deflate-подобная схема: не переизобретает LZ77 и Хаффмана, а просто
/// прогоняет данные через уже готовые реализации по очереди.
///
/// compress:   исходные данные -> LZ77 -> поток токенов -> Huffman -> архив
/// decompress: архив -> Huffman -> поток токенов LZ77 -> LZ77 -> исходные данные
///
/// LZ77 убирает повторы (пары/фразы), а Хаффман затем сжимает получившийся
/// поток токенов за счёт неравномерного распределения байт в нём (именно
/// так устроен настоящий DEFLATE: LZSS + Хаффман-кодирование результата).
pub struct DeflateCompressor;


impl Compressor for DeflateCompressor {
    fn compress(&self, artifact: Artifact) -> Result<Artifact, AppError> {
        let lzss_stage = LzssCompressor.compress(artifact)?;
        let huffman_stage = HuffmanCompressor.compress(lzss_stage)?;
        Ok(huffman_stage)
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
