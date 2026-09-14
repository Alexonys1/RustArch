use std::path::Path;

use crate::algorithms::{Cipher, Compressor, ErrorCorrectionCode, PipelineSettings, CompressionId};
use crate::archiver::{crc32_of_artifact_and_rewind, Artifact, ArtifactAfterPipeline, WalkedFile};
use crate::error::AppError;


const IS_OPTIMIZE_ON: bool = false; // TODO: Трудно придумать подходящее название.


/// compress -> encrypt -> fec-encode
/// Если файл после сжатия (compress) больше исходного, то оставляем исходный и проходим оставшиеся преобразования (encrypt -> fec).
pub fn pack_file(file: &WalkedFile, pipeline_settings: PipelineSettings, encode_key: &[u8]) -> Result<ArtifactAfterPipeline, AppError> {
    let mut artifact: Artifact = Artifact::from_file(file.absolute_path.as_ref())?;

    let original_size: u64 = artifact.payload_size() as u64;
    let crc32: u32 = crc32_of_artifact_and_rewind(&mut artifact, pipeline_settings)?;

    let cipher: Box<dyn Cipher> = pipeline_settings.cipher.get();
    let fec: Box<dyn ErrorCorrectionCode> = pipeline_settings.fec.get();

    let (compressed_artifact, effective_compression): (Artifact, CompressionId) = compress_with_fallback(
        artifact,
        pipeline_settings.compression,
        file.absolute_path.as_ref(),
        original_size,
    )?;
    let encoded_artifact: Artifact = cipher.transform(compressed_artifact, encode_key)?;
    let cooked_artifact: Artifact = fec.encode(encoded_artifact)?;

    Ok(ArtifactAfterPipeline {
        relative_path: file.relative_path.clone(),
        original_size,
        size_after_pipeline: cooked_artifact.payload_size() as u64,
        pipeline: PipelineSettings {
            compression: effective_compression,
            cipher: pipeline_settings.cipher,
            fec: pipeline_settings.fec,
        },
        crc32,
        payload: cooked_artifact,
    })
}


fn compress_with_fallback(
    artifact: Artifact,
    original_compressor_id: CompressionId,
    original_path: &Path,
    original_size: u64,
) -> Result<(Artifact, CompressionId), AppError>
{
    if original_compressor_id == CompressionId::NoCompression {
        return Ok((artifact, original_compressor_id));
    }

    let original_compressor: Box<dyn Compressor> = original_compressor_id.get();
    let compressed_artifact: Artifact = original_compressor.compress(artifact)?;

    if (compressed_artifact.payload_size() as u64) < original_size || !IS_OPTIMIZE_ON {
        Ok((compressed_artifact, original_compressor_id))
    }
    else {
        // Удаляем раздутый артефакт и открываем исходный по пути original_path:
        drop(compressed_artifact);

        let raw_artifact: Artifact = Artifact::from_file(original_path)
            .map_err(|e| AppError::Compression(format!(
                "compress_with_fallback: Не удалось повторно открыть '{}' для отката на NoCompressor: {e}",
                original_path.display()
            )))?;

        Ok((raw_artifact, CompressionId::NoCompression))
    }
}
