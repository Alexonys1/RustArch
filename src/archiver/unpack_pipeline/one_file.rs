use std::path::Path;

use crate::algorithms::fec::FecReport;
use crate::algorithms::{Cipher, Compressor, ErrorCorrectionCode, PipelineSettings};
use crate::archiver::{Artifact, ArchiveEntry, crc32_of_artifact_and_rewind, resolve_output_path};
use crate::error::AppError;


/// fec-decode -> decrypt -> decompress
pub fn unpack_file(entry: &ArchiveEntry, archive_path: &Path, output_dir: &Path, pipeline_settings: PipelineSettings, decode_key: &[u8]) -> Result<(), AppError> {
    let output_path = resolve_output_path(output_dir, &entry.relative_path)?;

    if entry.is_directory {
        std::fs::create_dir_all(&output_path)?;
        return Ok(());
    }

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let windowed_artifact: Artifact = Artifact::from_file_range(archive_path, entry.payload_offset, entry.stored_size)?;

    let fec: Box<dyn ErrorCorrectionCode> = pipeline_settings.fec.get();
    let cipher: Box<dyn Cipher> = pipeline_settings.cipher.get();
    let compressor: Box<dyn Compressor> = pipeline_settings.compression.get();

    let (fec_decoded_artifact, fec_report): (Artifact, FecReport) = fec.decode(windowed_artifact)?;
    let decrypted_artifact: Artifact = cipher.transform(fec_decoded_artifact, decode_key)?;
    let original_artifact: Artifact = compressor.decompress(decrypted_artifact)?;

    original_artifact.save_as_finish_file(&output_path)?; // !!РАСПАКОВЫВАЕМ ЗДЕСЬ!!

    let mut check_artifact: Artifact = Artifact::from_file(&output_path)?;
    let actual_crc32: u32 = crc32_of_artifact_and_rewind(&mut check_artifact)?;

    if actual_crc32 != entry.crc32 {
        return Err(AppError::ChecksumMismatch { path: output_path.display().to_string() });
    }

    Ok(())
}
