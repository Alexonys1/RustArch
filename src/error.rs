use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::algorithms::compression::{HuffmanError, LzssError};
use crate::algorithms::crypto::XorError;
use crate::algorithms::fec::ReedSolomonError;
use crate::archiver::{ArchiveError, InputError, PipelineError};
use crate::cli::CliError;


#[derive(Debug)]
pub struct AppError {
    kind: AppErrorKind,
}


#[derive(Debug, thiserror::Error)]
pub enum AppErrorKind {
    #[error("Ошибка ввода-вывода: {0}")]
    Io(#[source] io::Error),

    #[error("Ошибка формата архива: {0}")]
    Archive(#[source] ArchiveError),

    #[error("Ошибка использования: {0}")]
    Cli(#[source] CliError),

    #[error("Ошибка входных данных: {0}")]
    Input(#[source] InputError),

    #[error("Ошибка конвейера: {0}")]
    Pipeline(#[source] PipelineError),

    #[error("Ошибка Хаффмана: {0}")]
    Huffman(#[source] HuffmanError),

    #[error("Ошибка LZSS: {0}")]
    Lzss(#[source] LzssError),

    #[error("Ошибка XOR: {0}")]
    Xor(#[source] XorError),

    #[error("Ошибка Reed-Solomon: {0}")]
    ReedSolomon(#[source] ReedSolomonError),

    #[error(
        "Контрольная сумма не совпала для '{}': \
         файл повреждён или неверный ключ", path.display()
    )]
    ChecksumMismatch { path: PathBuf },

    #[error("Алгоритм '{0}' ещё не реализован")]
    NotImplemented(&'static str),
}


impl AppError {
    #[track_caller]
    #[cold]
    #[inline(never)]
    pub fn new(kind: AppErrorKind) -> AppError {
        AppError { kind }
    }

    pub fn kind(&self) -> &AppErrorKind {
        &self.kind
    }

    #[track_caller]
    #[cold]
    #[inline(never)]
    pub fn checksum_mismatch(path: impl Into<PathBuf>) -> Self {
        Self::new(AppErrorKind::ChecksumMismatch { path: path.into() })
    }

    #[track_caller]
    #[cold]
    #[inline(never)]
    pub fn not_implemented(name: &'static str) -> Self {
        Self::new(AppErrorKind::NotImplemented(name))
    }
}

impl fmt::Display for AppError {
    #[cold]
    #[inline(never)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl Error for AppError {
    #[cold]
    #[inline(never)]
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.kind.source()
    }
}

impl From<AppErrorKind> for AppError {
    #[track_caller]
    #[cold]
    #[inline(never)]
    fn from(kind: AppErrorKind) -> Self {
        Self::new(kind)
    }
}

impl From<io::Error> for AppError {
    #[track_caller]
    #[cold]
    #[inline(never)]
    fn from(source: io::Error) -> Self {
        Self::new(AppErrorKind::Io(source))
    }
}

impl From<HuffmanError> for AppError {
    #[track_caller]
    #[cold]
    #[inline(never)]
    fn from(source: HuffmanError) -> Self {
        Self::new(AppErrorKind::Huffman(source))
    }
}

impl From<LzssError> for AppError {
    #[track_caller]
    #[cold]
    #[inline(never)]
    fn from(source: LzssError) -> Self {
        Self::new(AppErrorKind::Lzss(source))
    }
}

impl From<XorError> for AppError {
    #[track_caller]
    #[cold]
    #[inline(never)]
    fn from(source: XorError) -> Self {
        Self::new(AppErrorKind::Xor(source))
    }
}

impl From<ReedSolomonError> for AppError {
    #[track_caller]
    #[cold]
    #[inline(never)]
    fn from(source: ReedSolomonError) -> Self {
        Self::new(AppErrorKind::ReedSolomon(source))
    }
}

impl From<CliError> for AppError {
    #[track_caller]
    #[cold]
    #[inline(never)]
    fn from(source: CliError) -> Self {
        Self::new(AppErrorKind::Cli(source))
    }
}

impl From<ArchiveError> for AppError {
    #[track_caller]
    #[cold]
    #[inline(never)]
    fn from(source: ArchiveError) -> Self {
        Self::new(AppErrorKind::Archive(source))
    }
}

impl From<InputError> for AppError {
    #[track_caller]
    #[cold]
    #[inline(never)]
    fn from(source: InputError) -> Self {
        Self::new(AppErrorKind::Input(source))
    }
}

impl From<PipelineError> for AppError {
    #[track_caller]
    #[cold]
    #[inline(never)]
    fn from(source: PipelineError) -> Self {
        Self::new(AppErrorKind::Pipeline(source))
    }
}
