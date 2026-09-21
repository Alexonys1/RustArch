use crate::error::AppError;
use crate::archiver::Artifact;
use crate::algorithms::compression::{CompressionId, Compressor};
use crate::algorithms::compression::utils::{
    BufferedArtifactReader, DecodeHistory, HashChain, SlidingWindow, find_longest_match,
};


const MIN_MATCH_LEN: usize = 3;
const MAX_CHAIN_LEN: usize = 125;
const MAX_OFFSET: usize = u16::MAX as usize;
const MAX_LENGTH: usize = u8::MAX as usize + MIN_MATCH_LEN;  // 255 + 3 = 258
const GROUP_SIZE: usize = 8;
const WINDOW_SIZE: usize = 32 * 1024;
const LOOKAHEAD_SIZE: usize = MAX_LENGTH;
const OUTPUT_FLUSH_SIZE: usize = 512 * 1024;
const MAX_LAZY_MATCH_LEN: usize = 32;
const FULL_INSERT_MATCH_LEN: usize = 32;
const TAIL_INSERTIONS: usize = 8;
const SAMPLE_BLOCK_SIZE: usize = WINDOW_SIZE;
const INCOMPRESSIBLE_RATIO_THRESHOLD: f64 = 0.8;
const MIN_LITERAL_RUN_LEN: usize = 34;
const MODE_COMPRESSED: u8 = 0;
const MODE_STORED: u8 = 1;


pub struct LzssCompressor;

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
        self.bodies.push(byte);
        self.advance(out);
    }

    fn push_match(&mut self, offset: usize, length: usize, out: &mut Vec<u8>) {
        debug_assert!(offset > 0, "offset=0 зарезервирован под литеральную последовательность");
        self.flags |= 1 << self.count;
        self.bodies.extend_from_slice(&(offset as u16).to_le_bytes());
        self.bodies.push((length - MIN_MATCH_LEN) as u8);
        self.advance(out);
    }

    /// Записывает подряд идущие литералы единым телом
    fn push_literal_run(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        debug_assert!(bytes.len() >= MIN_LITERAL_RUN_LEN);
        debug_assert!(bytes.len() <= u16::MAX as usize);
        self.flags |= 1 << self.count;
        self.bodies.extend_from_slice(&0u16.to_le_bytes());
        self.bodies.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
        self.bodies.extend_from_slice(bytes);
        self.advance(out);
    }

    fn advance(&mut self, out: &mut Vec<u8>) {
        self.count += 1;
        if self.count as usize == GROUP_SIZE {
            self.flush(out);
        }
    }

    /// Сбрасывает текущую (возможно неполную) группу.
    fn flush(&mut self, out: &mut Vec<u8>) {
        if self.count > 0 {
            out.push(self.flags);
            out.extend_from_slice(&self.bodies);
            self.flags = 0;
            self.count = 0;
            self.bodies.clear();
        }
    }

    /// Подсчет количества байт которые займет группа если сбросить её сейчас
    fn pending_len_if_flushed(&self) -> usize {
        if self.count > 0 { 1 + self.bodies.len() } else { 0 }
    }
}


/// Сбрасывает накопленный буфер подряд идущих литералов если их длина удовлетворяет MIN_LITERAL_RUN_LEN
fn flush_pending_literals(pending: &mut Vec<u8>, group: &mut TokenGroupWriter, out_buf: &mut Vec<u8>) {
    if pending.is_empty() {
        return;
    }
    if pending.len() >= MIN_LITERAL_RUN_LEN {
        group.push_literal_run(pending, out_buf);
    } else {
        for &byte in pending.iter() {
            group.push_literal(byte, out_buf);
        }
    }
    pending.clear();
}

// TODO: можно переиспользовать
/// Оценивает (НЕ мутируя состояние) сколько байт заняла бы буферизованная
/// пока литеральная последовательность, если бы её сейчас пришлось
/// сбросить - по тем же правилам, что и `flush_pending_literals`. Нужна
/// только для честной оценки размера пробного блока в эвристике
/// "не сжимать несжимаемое" (настоящий сброс там недопустим по той же
/// причине, что и у `TokenGroupWriter::pending_len_if_flushed`).
fn pending_literals_cost_estimate(pending: &[u8]) -> usize {
    if pending.is_empty() {
        0
    } else if pending.len() >= MIN_LITERAL_RUN_LEN {
        1 + 2 + 2 + pending.len() // ~1 байт на флаг-бит (округление) + offset(0) + run_len + сами байты
    } else {
        pending.len() + pending.len().div_ceil(8) // побайтовое округление битов флагов
    }
}


impl Compressor for LzssCompressor {
    fn compress(&self, mut artifact: Artifact) -> Result<(Artifact, CompressionId), AppError> {
        if WINDOW_SIZE == 0 || WINDOW_SIZE > MAX_OFFSET {
            return Err(AppError::Compression(format!(
                "LZSS: window_size={} вне допустимого диапазона 1..={} (формат хранит offset в 2 байтах)",
                WINDOW_SIZE, MAX_OFFSET
            )));
        }
        if LOOKAHEAD_SIZE == 0 || LOOKAHEAD_SIZE > MAX_LENGTH {
            return Err(AppError::Compression(format!(
                "LZSS: lookahead_size={} вне допустимого диапазона 1..={} (формат хранит length - {MIN_MATCH_LEN} в 1 байте)",
                LOOKAHEAD_SIZE, MAX_LENGTH
            )));
        }

        let original_size = artifact.get_payload_size() as u64;

        let mut window = SlidingWindow::new(WINDOW_SIZE, LOOKAHEAD_SIZE)?;
        let mut chain = HashChain::new(WINDOW_SIZE)?;

        let mut out_buf: Vec<u8> = Vec::with_capacity(OUTPUT_FLUSH_SIZE);
        let mut group = TokenGroupWriter::new();
        let mut pending_literals: Vec<u8> = Vec::new();
        let mut pos: u64 = 0;

        // ------------------------------------ Ранняя отсечка файла как НЕ сжимаемого ------------------------------------
        let sample_limit = (SAMPLE_BLOCK_SIZE as u64).min(original_size);
        while pos < sample_limit {
            match compress_step(&mut artifact, &mut window, &mut chain, &mut group, &mut pending_literals, &mut out_buf, pos)? {
                Some(new_pos) => pos = new_pos,
                None => break, // файл закончился раньше конца пробного блока
            }
        }

        // Использование отдельного буффера литералов.
        // В отсечке сбрасывать по настоящему ЗАПРЕЩЕНО!!!
        let sample_len = pos;
        let sample_compressed_len = out_buf.len()
            + group.pending_len_if_flushed()
            + pending_literals_cost_estimate(&pending_literals);
        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "compressed");

        // Проверка соотношения размера сжатого блока с исходжным
        let incompressible = sample_len > 0
            && sample_compressed_len as f64 >= sample_len as f64 * INCOMPRESSIBLE_RATIO_THRESHOLD;

        if incompressible {
            output.write_chunk_from(&[MODE_STORED])?;
            output.write_chunk_from(&original_size.to_le_bytes())?;

            // Внимание: `SlidingWindow::ensure_available` тянет данные из
            // `artifact` С ЗАПАСОМ на упреждающий просмотр (нужно для
            // поиска совпадений) - значит, к этому моменту в буфере окна
            // может лежать НЕ РОВНО `sample_len`, а немного больше байт, и
            // курсор чтения `artifact` уже продвинут соответственно.
            // Берём ВСЁ, что уже осело в окне (а не только первые
            // `sample_len` байт), и лишь потом читаем из `artifact`
            // остаток - иначе кусок файла между `sample_len` и фактической
            // позицией чтения потерялся бы при склейке с хвостом ниже.
            let buffered = window.available_after(0) as u64; // base_pos==0: обрезка окна ещё не срабатывала (см. `SAMPLE_BLOCK_SIZE`)
            let mut raw = Vec::with_capacity(buffered as usize);
            for i in 0..buffered {
                raw.push(window.byte_at(i));
            }
            output.write_chunk_from(&raw)?;

            // Запись остатка потоково, как есть.
            while let Some(chunk) = artifact.next_chunk()? {
                output.write_chunk_from(&chunk)?;
            }

            return Ok((output, CompressionId::NoCompression));
        }
        // ------------------------------------ Конец ранней отсечки ------------------------------------

        // Продолжения сжатия с уже отработанного блока
        output.write_chunk_from(&[MODE_COMPRESSED])?;
        output.write_chunk_from(&original_size.to_le_bytes())?;
        if !out_buf.is_empty() {
            output.write_chunk_from(&out_buf)?;
            out_buf.clear();
        }

        loop {
            match compress_step(&mut artifact, &mut window, &mut chain, &mut group, &mut pending_literals, &mut out_buf, pos)? {
                Some(new_pos) => pos = new_pos,
                None => break,
            }
            flush_if_needed(&mut out_buf, &mut output)?;
        }

        // Принудительный сброс остатка после окончания работы
        flush_pending_literals(&mut pending_literals, &mut group, &mut out_buf);
        group.flush(&mut out_buf);
        if !out_buf.is_empty() {
            output.write_chunk_from(&out_buf)?;
        }

        Ok((output, CompressionId::LZSS))
    }


    fn decompress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "decompressed");

        //TODO: Переработка пометки архивации файла
        // ПРОКЛЯТО!!!
        let mut reader = BufferedArtifactReader::new(&mut artifact);
        let mode = reader.read_u8()?;
        let original_size = reader.read_u64_le()?;

        if mode == MODE_STORED {
            // Файл был сохранён как есть - копируем оставшиеся байты без
            // разбора на токены. Читаем через `reader` (а не `artifact`
            // напрямую): часть данных уже могла осесть в его внутреннем
            // буфере при чтении заголовка, и должна попасть в вывод.
            let mut copied: u64 = 0;
            let mut chunk = Vec::with_capacity(OUTPUT_FLUSH_SIZE);
            while copied < original_size {
                let want = (original_size - copied).min(OUTPUT_FLUSH_SIZE as u64) as usize;
                chunk.clear();
                reader.append_exact(want, &mut chunk)?;
                output.write_chunk_from(&chunk)?;
                copied += want as u64;
            }
            return Ok(output);
        }

        if mode != MODE_COMPRESSED {
            return Err(AppError::CorruptArchive(format!("LZSS: неизвестный режим потока: {mode}")));
        }

        // Цикл деархивации
        let mut window = DecodeHistory::new(WINDOW_SIZE);
        let mut out_buf: Vec<u8> = Vec::with_capacity(OUTPUT_FLUSH_SIZE);
        let mut produced: u64 = 0;

        'outer: while produced < original_size {
            let flags = reader.read_u8()?;

            for bit_idx in 0..GROUP_SIZE {
                if produced >= original_size {
                    // Отбраcываем неиспользуемые БИТы флагов из целых БАЙТов
                    break 'outer;
                }

                // Проверка на нахождение токена
                let is_match = (flags >> bit_idx) & 1 == 1;

                if is_match {
                    // Обработка токена
                    let offset = reader.read_u16_le()? as usize;

                    if offset == 0 {
                        // Обработка ПОСЛЕДОВАТЕЛЬНОСТИ токенов
                        let run_len = reader.read_u16_le()? as usize;
                        if run_len == 0 || run_len as u64 > original_size - produced {
                            return Err(AppError::CorruptArchive(
                                "LZSS: некорректная длина литерального блока".to_string(),
                            ));
                        }
                        let start = out_buf.len();
                        reader.append_exact(run_len, &mut out_buf)?;
                        window.extend(&out_buf[start..]);
                        produced += run_len as u64;
                    } else {
                        // Обработка единичного токена
                        let length = reader.read_u8()? as usize + MIN_MATCH_LEN;

                        if offset > window.len() || length as u64 > original_size - produced {
                            return Err(AppError::CorruptArchive(
                                "LZSS: некорректная ссылка назад".to_string(),
                            ));
                        }
                        window.copy_match(offset, length, &mut out_buf)?;
                        produced += length as u64;
                    }
                } else {
                    // Запись некодированного байта
                    let literal = reader.read_u8()?;
                    window.push(literal);
                    out_buf.push(literal);
                    produced += 1;
                }

                // Сброс буффера
                if out_buf.len() >= OUTPUT_FLUSH_SIZE {
                    output.write_chunk_from(&out_buf)?;
                    out_buf.clear();
                }
            }
        }

        // Принудительный сброс буффера в конце файла
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

/// Обработка однога шага архивации
/// Вынесено в отдельную функцию, чтобы использовать при сжатии первого блока
/// В первом блоке ЗАПРЕЩЕНА запись
fn compress_step(
    artifact: &mut Artifact,
    window: &mut SlidingWindow,
    chain: &mut HashChain,
    group: &mut TokenGroupWriter,
    pending_literals: &mut Vec<u8>,
    out_buf: &mut Vec<u8>,
    pos: u64,
) -> Result<Option<u64>, AppError>
{
    // Получаем окно на чтение от текущей позиции
    window.ensure_available(artifact, pos, LOOKAHEAD_SIZE + 2)?;
    let available = window.available_after(pos);
    if available == 0 {
        return Ok(None);
    }

    let max_len = LOOKAHEAD_SIZE.min(available).min(MAX_LENGTH);
    // Ищем наибольшую подстроку
    let (offset, length) = find_longest_match(
        window, chain, pos, WINDOW_SIZE as u64, max_len, MIN_MATCH_LEN, MAX_CHAIN_LEN,
    );

    chain.insert(window, pos);

    if length >= MIN_MATCH_LEN && length < MAX_LAZY_MATCH_LEN && available > 1 {
        // ВАЖНАЯ ЭВРИСТИКА
        // Если текущая подстрока хуже ленивой проверки,
        // То пробуем увеличить длину за счет смены позиции
        //TODO: Хрень какая то, разобраться, переделать
        let available_next = window.available_after(pos + 1);
        let max_len_next = LOOKAHEAD_SIZE.min(available_next).min(MAX_LENGTH);
        let (_, length_next) = find_longest_match(
            window, chain, pos + 1, WINDOW_SIZE as u64, max_len_next, MIN_MATCH_LEN, MAX_CHAIN_LEN,
        );

        if length_next > length {
            pending_literals.push(window.byte_at(pos));
            maybe_flush_overflowing_run(pending_literals, group, out_buf);
            let new_pos = pos + 1;
            window.trim_if_needed(new_pos);
            return Ok(Some(new_pos));
        }
    }

    let new_pos;
    if length >= MIN_MATCH_LEN {
        // Найдена удовлетворителоьная подстрока
        // Она прерывает поток литералов, нужно принимать решение о их сбросе из буффера
        flush_pending_literals(pending_literals, group, out_buf);

        group.push_match(offset, length, out_buf);

        // Устанавливаем битовый флаг
        let first_to_insert = if length <= FULL_INSERT_MATCH_LEN {
            1
        } else {
            length.saturating_sub(TAIL_INSERTIONS)
        };
        for i in first_to_insert..length {
            let p = pos + i as u64;
            if window.available_after(p) >= 3 {
                chain.insert(window, p);
            }
        }

        new_pos = pos + length as u64;
    } else {
        // Удовлетворительная подстрока не найдена, продолжаем поток литералов
        pending_literals.push(window.byte_at(pos));
        maybe_flush_overflowing_run(pending_literals, group, out_buf);
        new_pos = pos + 1;
    }

    window.trim_if_needed(new_pos);
    Ok(Some(new_pos))
}


/// Буфер литералов ограничен размером u16
/// При достижении сбрасываем сырым блоком
fn maybe_flush_overflowing_run(pending: &mut Vec<u8>, group: &mut TokenGroupWriter, out_buf: &mut Vec<u8>) {
    if pending.len() >= u16::MAX as usize {
        flush_pending_literals(pending, group, out_buf);
    }
}


fn flush_if_needed(out_buf: &mut Vec<u8>, output: &mut Artifact) -> Result<(), AppError> {
    if out_buf.len() >= OUTPUT_FLUSH_SIZE {
        output.write_chunk_from(out_buf)?;
        out_buf.clear();
    }
    Ok(())
}
