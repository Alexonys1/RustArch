use std::path::Path;

use crate::algorithms::fec::FecReport;
use crate::algorithms::{Cipher, Compressor, ErrorCorrectionCode};
use crate::archiver::{Artifact, ArchiveEntry, crc32_of_artifact_and_rewind, resolve_output_path};
use crate::error::AppError;

/// fec-decode -> decrypt -> decompress.
/// Pipeline берётся из самой записи: разные файлы одного архива могут
/// использовать разные effective-compression после fallback.
pub fn unpack_file(
    entry: &ArchiveEntry,
    archive_path: &Path,
    output_dir: &Path,
    decode_key: &[u8],
) -> Result<(), AppError> {
    let output_path = resolve_output_path(output_dir, &entry.relative_path)?;

    if entry.is_directory {
        std::fs::create_dir_all(&output_path)?;
        return Ok(());
    }

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let windowed_artifact = Artifact::from_file_range(
        archive_path,
        entry.payload_offset,
        entry.stored_size,
    )?;

    let fec: Box<dyn ErrorCorrectionCode> = entry.pipeline.fec.get();
    let cipher: Box<dyn Cipher> = entry.pipeline.cipher.get();
    let compressor: Box<dyn Compressor> = entry.pipeline.compression.get();

    let (fec_decoded_artifact, _fec_report) = fec.decode(windowed_artifact)?;
    let decrypted_artifact = cipher.transform(fec_decoded_artifact, decode_key)?;
    let original_artifact = compressor.decompress(decrypted_artifact)?;

    original_artifact.save_as_finish_file(&output_path)?;

    let mut check_artifact = Artifact::from_file(&output_path)?;
    let actual_crc32 = crc32_of_artifact_and_rewind(&mut check_artifact, entry.pipeline)?;

    if actual_crc32 != entry.crc32 {
        return Err(AppError::ChecksumMismatch {
            path: output_path.display().to_string(),
        });
    }

    Ok(())
}
