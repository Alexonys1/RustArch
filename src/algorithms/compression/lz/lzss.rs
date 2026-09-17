use crate::error::AppError;
use crate::archiver::Artifact;
use crate::algorithms::compression::{CompressionId, Compressor};
use crate::algorithms::compression::utils::{find_longest_match, HashChain, SlidingWindow, BufferedArtifactReader};


/// Минимальная длина совпадения, при которой выгодно ссылаться на прошлое
/// вхождение вместо того, чтобы просто записать литерал. Тело совпадения -
/// 3 байта (offset:u16 + length:u8), тело литерала - 1 байт; оба несут ещё
/// амортизированно ~1/8 байта на бит флага группы (см. `GROUP_SIZE`).
/// При длине 3 совпадение (3 + 1/8 байта) уже дешевле трёх литералов
/// (3 * (1 + 1/8) байта).
const MIN_MATCH_LEN: usize = 3;
const MAX_CHAIN_LEN: usize = 255;
const MAX_OFFSET: usize = u16::MAX as usize;
const MAX_LENGTH: usize = u8::MAX as usize;

/// Сколько токенов покрывает один флаг-байт. Бит `i` флаг-байта: 1 -
/// совпадение, 0 - литерал. Тела токенов идут сразу после флаг-байта
/// подряд, без собственных тегов - это убирает 100%-й оверхед старого
/// формата "[tag, literal]" на каждый литерал: раньше литерал стоил
/// 2 байта (тег + сам байт), теперь - 1 байт данных плюс 1/8 байта
/// амортизированно на бит флага, то есть ~1.125 байта вместо 2.
///
/// На данных с редкими совпадениями (уже сжатые игровые ассеты - текстуры,
/// звук) это меняет всё: раньше LZSS почти удваивал размер файла, теперь
/// раздувает его всего на ~12%.
const GROUP_SIZE: usize = 8;

const WINDOW_SIZE: usize = 32 * 1024;
const LOOKAHEAD_SIZE: usize = 255;

const OUTPUT_FLUSH_SIZE: usize = 512 * 1024;

/// Ленивую проверку имеет смысл делать только для сравнительно КОРОТКИХ
/// совпадений - см. подробное объяснение в предыдущей версии файла.
const MAX_LAZY_MATCH_LEN: usize = 32;


pub struct LzssCompressor;


/// Копит до `GROUP_SIZE` токенов, прежде чем сбросить флаг-байт + их тела
/// в выходной буфер. Работает как маленький промежуточный буфер поверх
/// уже существующего `out_buf` (который сам ограничен `OUTPUT_FLUSH_SIZE`
/// и периодически сбрасывается в `Artifact`) - размер этого буфера
/// тривиален (максимум `GROUP_SIZE * 3` байт тел + 1 байт флага), поэтому
/// отдельного учёта в MemoryBudget не требует.
struct TokenGroupWriter {
    flags: u8,
    count: u8,
    bodies: Vec<u8>,
}

impl TokenGroupWriter {
    fn new() -> Self {
        Self { flags: 0, count: 0, bodies: Vec::with_capacity(GROUP_SIZE * 3) }
    }

    fn push_literal(&mut self, byte: u8, out: &mut Vec<u8>) {
        // Бит остаётся 0 (литерал) - flags уже инициализирован нулями
        // для текущей группы, ничего дополнительно выставлять не нужно.
        self.bodies.push(byte);
        self.advance(out);
    }

    fn push_match(&mut self, offset: usize, length: usize, out: &mut Vec<u8>) {
        self.flags |= 1 << self.count;
        self.bodies.extend_from_slice(&(offset as u16).to_le_bytes());
        self.bodies.push(length as u8);
        self.advance(out);
    }

    fn advance(&mut self, out: &mut Vec<u8>) {
        self.count += 1;
        if self.count as usize == GROUP_SIZE {
            self.flush(out);
        }
    }

    /// Сбрасывает текущую (возможно неполную) группу. Неполная последняя
    /// группа безопасна: decompress останавливается по `original_size`,
    /// а неиспользованные биты флага для несуществующих "хвостовых"
    /// токенов просто никогда не читаются - тот же принцип, что и с
    /// нулевым паддингом последнего байта в BitWriter Хаффмана.
    fn flush(&mut self, out: &mut Vec<u8>) {
        if self.count > 0 {
            out.push(self.flags);
            out.extend_from_slice(&self.bodies);
            self.flags = 0;
            self.count = 0;
            self.bodies.clear();
        }
    }
}


impl Compressor for LzssCompressor {
    fn compress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        if WINDOW_SIZE == 0 || WINDOW_SIZE > MAX_OFFSET {
            return Err(AppError::Compression(format!(
                "LZSS: window_size={} вне допустимого диапазона 1..={} (формат хранит offset в 2 байтах)",
                WINDOW_SIZE, MAX_OFFSET
            )));
        }
        if LOOKAHEAD_SIZE == 0 || LOOKAHEAD_SIZE > MAX_LENGTH {
            return Err(AppError::Compression(format!(
                "LZSS: lookahead_size={} вне допустимого диапазона 1..={} (формат хранит length в 1 байте)",
                LOOKAHEAD_SIZE, MAX_LENGTH
            )));
        }

        let original_size = artifact.get_payload_size() as u64;
        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "compressed");
        output.write_chunk_from(&original_size.to_le_bytes())?;

        let mut window = SlidingWindow::new(WINDOW_SIZE, LOOKAHEAD_SIZE)?;
        let mut chain = HashChain::new(WINDOW_SIZE)?;

        let mut out_buf: Vec<u8> = Vec::with_capacity(OUTPUT_FLUSH_SIZE);
        let mut group = TokenGroupWriter::new();
        let mut pos: u64 = 0;

        loop {
            window.ensure_available(&mut artifact, pos, LOOKAHEAD_SIZE + 2)?;
            let available = window.available_after(pos);
            if available == 0 {
                break;
            }

            let max_len = LOOKAHEAD_SIZE.min(available).min(MAX_LENGTH);
            let (offset, length) = find_longest_match(
                &window, &chain, pos, WINDOW_SIZE as u64, max_len, MIN_MATCH_LEN, MAX_CHAIN_LEN,
            );

            chain.insert(&window, pos);

            if length >= MIN_MATCH_LEN && length < MAX_LAZY_MATCH_LEN && available > 1 {
                let available_next = window.available_after(pos + 1);
                let max_len_next = LOOKAHEAD_SIZE.min(available_next).min(MAX_LENGTH);
                let (_, length_next) = find_longest_match(
                    &window, &chain, pos + 1, WINDOW_SIZE as u64, max_len_next, MIN_MATCH_LEN, MAX_CHAIN_LEN,
                );

                if length_next > length {
                    group.push_literal(window.byte_at(pos), &mut out_buf);
                    pos += 1;
                    flush_if_needed(&mut out_buf, &mut output)?;
                    window.trim_if_needed(pos);
                    continue;
                }
            }

            if length >= MIN_MATCH_LEN {
                group.push_match(offset, length, &mut out_buf);

                for i in 1..length {
                    let p = pos + i as u64;
                    if window.available_after(p) >= 3 {
                        chain.insert(&window, p);
                    }
                }

                pos += length as u64;
            } else {
                group.push_literal(window.byte_at(pos), &mut out_buf);
                pos += 1;
            }

            flush_if_needed(&mut out_buf, &mut output)?;
            window.trim_if_needed(pos);
        }

        // Сбрасываем незавершённую последнюю группу - иначе до 7 токенов
        // в самом конце файла потерялись бы, оставшись в `group.bodies`.
        group.flush(&mut out_buf);
        if !out_buf.is_empty() {
            output.write_chunk_from(&out_buf)?;
        }

        Ok(output)
    }

    fn decompress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "decompressed");

        let mut reader = BufferedArtifactReader::new(&mut artifact);
        let original_size = u64::from_le_bytes(reader.read_exact(8)?.try_into().unwrap());

        let mut window: std::collections::VecDeque<u8> = std::collections::VecDeque::with_capacity(WINDOW_SIZE);
        let mut out_buf: Vec<u8> = Vec::with_capacity(OUTPUT_FLUSH_SIZE);
        let mut produced: u64 = 0;

        'outer: while produced < original_size {
            let flags = reader.read_u8()?;

            for bit_idx in 0..GROUP_SIZE {
                if produced >= original_size {
                    // Остаток бит текущего флаг-байта - "хвостовой" мусор
                    // от неполной последней группы, реальных тел под ним
                    // в потоке нет - прекращаем, не пытаясь их читать.
                    break 'outer;
                }

                let is_match = (flags >> bit_idx) & 1 == 1;

                if is_match {
                    let offset = u16::from_le_bytes(reader.read_exact(2)?.try_into().unwrap()) as usize;
                    let length = reader.read_u8()? as usize;

                    if offset == 0 || offset > window.len() {
                        return Err(AppError::CorruptArchive(
                            "LZSS: Некорректный offset ссылки назад".to_string(),
                        ));
                    }

                    // start фиксируется ДО копирования; подрезка окна -
                    // после полной обработки токена (см. комментарий ниже).
                    let start = window.len() - offset;
                    for i in 0..length {
                        let byte = window[start + i];
                        window.push_back(byte);
                        out_buf.push(byte);
                    }
                    produced += length as u64;
                } else {
                    let literal = reader.read_u8()?;
                    window.push_back(literal);
                    out_buf.push(literal);
                    produced += 1;
                }

                // Подрезаем окно ПОСЛЕ токена целиком - не в процессе
                // копирования, иначе индексы съехали бы при
                // самоссылающихся совпадениях (offset < length).
                while window.len() > WINDOW_SIZE {
                    window.pop_front();
                }

                if out_buf.len() >= OUTPUT_FLUSH_SIZE {
                    output.write_chunk_from(&out_buf)?;
                    out_buf.clear();
                }
            }
        }

        if !out_buf.is_empty() {
            output.write_chunk_from(&out_buf)?;
        }

        Ok(output)
    }

    fn id(&self) -> CompressionId {
        CompressionId::LZSS
    }
}


// ========== ВСПОМОГАТЕЛЬНЫЕ ФУНКЦИИ ==========

fn flush_if_needed(out_buf: &mut Vec<u8>, output: &mut Artifact) -> Result<(), AppError> {
    if out_buf.len() >= OUTPUT_FLUSH_SIZE {
        output.write_chunk_from(out_buf)?;
        out_buf.clear();
    }
    Ok(())
}
