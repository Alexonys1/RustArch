use std::collections::VecDeque;

use crate::algorithms::compression::utils::{BufferedArtifactReader, HashChain, SlidingWindow, find_longest_match, flush_if_needed};
use crate::algorithms::compression::{CompressionId, Compressor};
use crate::archiver::Artifact;
use crate::error::AppError;


/// Минимальная длина совпадения, при которой выгодно ссылаться на прошлое
/// вхождение вместо того, чтобы просто записать литерал. При стоимости
/// токена-совпадения в 5 байт (см. `TAG_MATCH_WITH_LITERAL`) и токена-литерала
/// в 2 байта совпадение длиной 3 уже окупается: 5 байт вместо 3*2 = 6.
const MIN_MATCH_LEN: usize = 3;

const MAX_CHAIN_LEN: usize = 64;
const MAX_OFFSET: usize = u16::MAX as usize;
const MAX_LENGTH: usize = u8::MAX as usize;

const TAG_LITERAL: u8 = 0;
const TAG_MATCH_WITH_LITERAL: u8 = 1;
const TAG_MATCH_AT_EOF: u8 = 2;

const WINDOW_SIZE: usize = 65_535;
const LOOKAHEAD_SIZE: usize = 255;

/// Порог сброса накопленного выходного буфера в Artifact::write_chunk_from -
/// см. подробное объяснение в lzss.rs. Ограничивает выходной буфер
/// константой независимо от размера файла, вместо `encoded: Vec<u8>`,
/// растущего до конца обработки.
const OUTPUT_FLUSH_SIZE: usize = 256 * 1024;


/// Классический LZ77: скользящее окно `window_size` (уже просмотренные
/// данные, "словарь") + буфер упреждающего просмотра `lookahead_size`
/// (максимальная длина совпадения). Каждый выход алгоритма - тройка
/// (offset, length, next_literal): `offset` - на сколько байт назад от
/// текущей позиции начинается совпадение, `length` - его длина, а
/// `next_literal` - литерал сразу после совпадения (либо единственный
/// литерал, если совпадения не нашлось).
pub struct Lz77Compressor;


impl Compressor for Lz77Compressor {
    fn compress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        if WINDOW_SIZE == 0 || WINDOW_SIZE > MAX_OFFSET {
            return Err(AppError::Compression(format!(
                "LZ77: window_size={} вне допустимого диапазона 1..={} (формат хранит offset в 2 байтах)",
                WINDOW_SIZE, MAX_OFFSET
            )));
        }
        if LOOKAHEAD_SIZE == 0 || LOOKAHEAD_SIZE > MAX_LENGTH {
            return Err(AppError::Compression(format!(
                "LZ77: lookahead_size={} вне допустимого диапазона 1..={} (формат хранит length в 1 байте)",
                LOOKAHEAD_SIZE, MAX_LENGTH
            )));
        }

        // get_payload_size() - это метаданные (просто число), а не чтение
        // содержимого, поэтому узнать общий размер заранее можно без
        // единого обращения к самим данным файла.
        let n = artifact.get_payload_size() as u64;
        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "compressed");
        output.write_chunk_from(&n.to_le_bytes())?;

        // Как и в LZSS: единственное, что держится в памяти - ограниченное
        // окно и хэш-таблицы фиксированного размера, НЕ зависящие от
        // размера файла.
        let mut window = SlidingWindow::new(WINDOW_SIZE, LOOKAHEAD_SIZE)?;
        let mut chain = HashChain::new(WINDOW_SIZE)?;

        let mut out_buf: Vec<u8> = Vec::with_capacity(OUTPUT_FLUSH_SIZE);
        let mut pos: u64 = 0;

        while pos < n {
            // +2 с запасом: 1 байт под сам литерал, идущий сразу после
            // совпадения, дотянувшегося до конца текущего окна просмотра.
            window.ensure_available(&mut artifact, pos, LOOKAHEAD_SIZE + 2)?;
            let available = window.available_after(pos);
            if available == 0 {
                break;
            }

            let max_len = LOOKAHEAD_SIZE.min(available).min(MAX_LENGTH);
            let (offset, length) = find_longest_match(
                &window, &chain, pos, WINDOW_SIZE as u64, max_len, MIN_MATCH_LEN, MAX_CHAIN_LEN,
            );

            // Индексируем ВСЕ позиции, которые сейчас "проглатываем" (даже
            // внутри найденного совпадения) - это позволяет будущим
            // совпадениям начинаться внутри уже закодированного участка.
            // Как и в LZSS, вставляем только если под позицией реально
            // есть 3 байта для хэша - на самом конце файла это не так.
            let advance = if length > 0 { length as u64 } else { 1 };
            let end = (pos + advance).min(pos + available as u64);
            for i in pos..end {
                if window.available_after(i) >= 3 {
                    chain.insert(&window, i);
                }
            }

            if length > 0 {
                let next_pos = pos + length as u64;
                if window.available_after(next_pos) > 0 {
                    push_match_token(&mut out_buf, offset, length, Some(window.byte_at(next_pos)));
                    pos = next_pos + 1;
                } else {
                    // Совпадение дотянулось ровно до конца данных - литерала после него нет.
                    push_match_token(&mut out_buf, offset, length, None);
                    pos = next_pos;
                }
            } else {
                push_literal_token(&mut out_buf, window.byte_at(pos));
                pos += 1;
            }

            flush_if_needed(&mut out_buf, &mut output, OUTPUT_FLUSH_SIZE)?;
            window.trim_if_needed(pos);
        }

        if !out_buf.is_empty() {
            output.write_chunk_from(&out_buf)?;
        }

        Ok(output)
    }

    fn decompress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "decompressed");

        // Буферизованное чтение токенов вместо read_chunk_to по 1-2 байта
        // напрямую из Artifact - см. подробности в lzss.rs.
        let mut reader = BufferedArtifactReader::new(&mut artifact);
        let original_size = u64::from_le_bytes(reader.read_exact(8)?.try_into().unwrap());

        // Окно последних WINDOW_SIZE выведенных байт вместо накопления
        // всего `decoded: Vec<u8>` размером с файл.
        let mut window: VecDeque<u8> = VecDeque::with_capacity(WINDOW_SIZE);
        let mut out_buf: Vec<u8> = Vec::with_capacity(OUTPUT_FLUSH_SIZE);
        let mut produced: u64 = 0;

        while produced < original_size {
            let tag = reader.read_u8()?;

            match tag {
                TAG_LITERAL => {
                    let literal = reader.read_u8()?;
                    window.push_back(literal);
                    out_buf.push(literal);
                    produced += 1;
                }
                TAG_MATCH_WITH_LITERAL | TAG_MATCH_AT_EOF => {
                    let offset = u16::from_le_bytes(reader.read_exact(2)?.try_into().unwrap()) as usize;
                    let length = reader.read_u8()? as usize;

                    if offset == 0 || offset > window.len() {
                        return Err(AppError::CorruptArchive(
                            "LZ77: Некорректный offset ссылки назад".to_string(),
                        ));
                    }

                    // `start` фиксируется ДО копирования; подрезка окна
                    // (pop_front) делается ПОСЛЕ полной обработки токена
                    // (включая литерал, если он есть) - иначе индексы
                    // съехали бы при самоссылающихся совпадениях
                    // (offset < length) или при обрезке ровно посреди
                    // копирования.
                    let start = window.len() - offset;
                    for i in 0..length {
                        let byte = window[start + i];
                        window.push_back(byte);
                        out_buf.push(byte);
                    }
                    produced += length as u64;

                    if tag == TAG_MATCH_WITH_LITERAL {
                        let literal = reader.read_u8()?;
                        window.push_back(literal);
                        out_buf.push(literal);
                        produced += 1;
                    }
                }
                other => {
                    return Err(AppError::CorruptArchive(format!(
                        "LZ77: Неизвестный тег токена: {other}"
                    )));
                }
            }

            while window.len() > WINDOW_SIZE {
                window.pop_front();
            }

            if out_buf.len() >= OUTPUT_FLUSH_SIZE {
                output.write_chunk_from(&out_buf)?;
                out_buf.clear();
            }
        }

        if !out_buf.is_empty() {
            output.write_chunk_from(&out_buf)?;
        }

        Ok(output)
    }

    fn id(&self) -> CompressionId {
        CompressionId::LZ77
    }
}


// ========== ВСПОМОГАТЕЛЬНЫЕ ФУНКЦИИ ==========

fn push_literal_token(buf: &mut Vec<u8>, literal: u8) {
    buf.push(TAG_LITERAL);
    buf.push(literal);
}

fn push_match_token(buf: &mut Vec<u8>, offset: usize, length: usize, literal: Option<u8>) {
    let offset_bytes = (offset as u16).to_le_bytes();
    match literal {
        Some(byte) => {
            buf.push(TAG_MATCH_WITH_LITERAL);
            buf.extend_from_slice(&offset_bytes);
            buf.push(length as u8);
            buf.push(byte);
        }
        None => {
            buf.push(TAG_MATCH_AT_EOF);
            buf.extend_from_slice(&offset_bytes);
            buf.push(length as u8);
        }
    }
}