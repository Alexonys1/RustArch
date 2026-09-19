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

/// Байт длины хранит НЕ саму длину совпадения, а `length - MIN_MATCH_LEN`
/// (см. `TokenGroupWriter::push_match`) - ведь длина короче MIN_MATCH_LEN
/// в токен-совпадение попасть не может по построению, так что эти
/// значения были бы потрачены впустую. Тот же приём, которым в настоящем
/// DEFLATE длины 3..=258 кодируются через таблицу длин с "лишними" битами:
/// там тоже физически нет значения "длина 0, 1 или 2" у кода совпадения.
/// Смещение на MIN_MATCH_LEN при тех же 8 битах поля увеличивает
/// максимальную длину совпадения с 255 (как было) до 258 - ровно то
/// значение, которое использует zip/DEFLATE.
const MAX_LENGTH: usize = u8::MAX as usize + MIN_MATCH_LEN;

/// Сколько токенов покрывает один флаг-байт. Бит `i` флаг-байта: 1 -
/// совпадение (или литеральная последовательность - см.
/// `TokenGroupWriter::push_literal_run`), 0 - одиночный литерал. Тела
/// токенов идут сразу после флаг-байта подряд, без собственных тегов -
/// это убирает 100%-й оверхед старого формата "[tag, literal]" на каждый
/// литерал: раньше литерал стоил 2 байта (тег + сам байт), теперь -
/// 1 байт данных плюс 1/8 байта амортизированно на бит флага, то есть
/// ~1.125 байта вместо 2.
///
/// На данных с редкими совпадениями (уже сжатые игровые ассеты - текстуры,
/// звук) это меняет всё: раньше LZSS почти удваивал размер файла, теперь
/// раздувает его всего на ~12%. Для по-настоящему несжимаемых данных этого
/// всё ещё недостаточно - см. `INCOMPRESSIBLE_RATIO_THRESHOLD` (не сжимать
/// файл вообще) и `MIN_LITERAL_RUN_LEN` (не платить даже эти ~12% за длинные
/// подряд идущие несжимаемые участки) ниже.
const GROUP_SIZE: usize = 8;

/// Окно 32 КиБ - тот же размер, что использует zip/DEFLATE (тот случай,
/// когда наша константа уже совпадала и трогать её не нужно).
const WINDOW_SIZE: usize = 32 * 1024;

/// Верхняя граница длины совпадения - используем весь диапазон, который
/// вообще способен представить формат токена (см. `MAX_LENGTH`): это и
/// есть те самые 258 байт максимальной длины совпадения из zip/DEFLATE.
const LOOKAHEAD_SIZE: usize = MAX_LENGTH;

const OUTPUT_FLUSH_SIZE: usize = 512 * 1024;

/// Ленивую проверку имеет смысл делать только для сравнительно КОРОТКИХ
/// совпадений - см. подробное объяснение в предыдущей версии файла.
const MAX_LAZY_MATCH_LEN: usize = 32;

/// Размер "первого блока", на котором проверяется, окупается ли сжатие
/// вообще (см. `INCOMPRESSIBLE_RATIO_THRESHOLD`). Равен `WINDOW_SIZE` НЕ
/// случайно: `SlidingWindow::trim_if_needed` подрезает историю только
/// после того, как пройденная дистанция превысит `window_size * 3` (см.
/// `slide_trigger` в utils.rs), поэтому пока пробная позиция не вышла за
/// пределы одного окна, все прочитанные для пробы байты гарантированно
/// ещё живы в буфере - и мы можем взять их оттуда напрямую для режима
/// "сохранить как есть", не читая файл заново.
const SAMPLE_BLOCK_SIZE: usize = WINDOW_SIZE;

/// Если сжатый пробный блок занимает НЕ МЕНЬШЕ этой доли от размера самого
/// блока (т.е. LZSS выигрывает меньше ~25%), считаем файл практически
/// несжимаемым (уже упакованный архив, медиа-ассет и т.п.) и сохраняем
/// его целиком как есть, не тратя время на разбор оставшихся мегабайт на
/// токены. Тот же принцип, что и режим STORE в ZIP: пробуем сжать,
/// сравниваем с оригиналом, при отсутствии выигрыша не сжимаем вообще.
/// По умолчанию 0.75 - подобрано эмпирически, при необходимости можно
/// изменить.
const INCOMPRESSIBLE_RATIO_THRESHOLD: f64 = 0.8;

/// Минимальная длина ПОДРЯД идущих литералов, при которой выгодно
/// записать их одним "сырым" блоком (`TokenGroupWriter::push_literal_run`)
/// вместо обычных отдельных литеральных токенов. Сырой блок стоит
/// `1 бит флага + 2 байта (offset=0, признак) + 2 байта (run_len) + N байт`
/// против `N бит флагов + N байт` у N отдельных литералов:
///   N + 4.125 < N * 1.125  =>  4.125 < 0.125*N  =>  N > 33
/// Отсюда порог 34 - при такой и большей длине сырой блок ГАРАНТИРОВАННО
/// дешевле поштучной записи; при меньшей - наоборот дороже (постоянные
/// 4 байта накладных расходов не успевают окупиться).
const MIN_LITERAL_RUN_LEN: usize = 34;

/// Тело потока - результат разбора на токены LZSS (обычный случай).
const MODE_COMPRESSED: u8 = 0;
/// Тело потока - исходные байты без изменений (сработала эвристика
/// "несжимаемо", см. `INCOMPRESSIBLE_RATIO_THRESHOLD`).
const MODE_STORED: u8 = 1;


pub struct LzssCompressor;


/// Копит до `GROUP_SIZE` токенов, прежде чем сбросить флаг-байт + их тела
/// в выходной буфер. Работает как маленький промежуточный буфер поверх
/// уже существующего `out_buf` (который сам ограничен `OUTPUT_FLUSH_SIZE`
/// и периодически сбрасывается в `Artifact`) - размер этого буфера
/// тривиален (максимум `GROUP_SIZE * (2 + MIN_LITERAL_RUN_LEN)` байт тел
/// в худшем случае + 1 байт флага), поэтому отдельного учёта в
/// MemoryBudget не требует.
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
        debug_assert!(offset > 0, "offset=0 зарезервирован под литеральную последовательность");
        self.flags |= 1 << self.count;
        self.bodies.extend_from_slice(&(offset as u16).to_le_bytes());
        // "- MIN_MATCH_LEN" - см. комментарий у константы `MAX_LENGTH`.
        self.bodies.push((length - MIN_MATCH_LEN) as u8);
        self.advance(out);
    }

    /// Записывает ПОДРЯД идущие литералы ОДНИМ телом вместо `bytes.len()`
    /// отдельных токенов - см. `MIN_LITERAL_RUN_LEN`. Использует тот же
    /// бит флага, что и совпадение (1), но помечает себя служебным
    /// значением `offset == 0` - настоящее совпадение никогда не может
    /// иметь нулевой offset (расстояние назад всегда >= 1), поэтому это
    /// безопасный признак "здесь не совпадение, а сырой блок литералов":
    /// на каждый из `bytes.len()` байт НЕ тратится собственный бит флага
    /// или тег - ровно то, что и просил убрать "метку токена".
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

    /// Сбрасывает текущую (возможно неполную) группу. Неполная группа
    /// безопасна ТОЛЬКО в самом конце потока: decompress останавливается
    /// по `original_size` раньше, чем попытается прочитать несуществующие
    /// "хвостовые" токены - тот же принцип, что и с нулевым паддингом
    /// последнего байта в BitWriter Хаффмана. Вызывать эту функцию где-то,
    /// кроме истинного конца файла, НЕЛЬЗЯ: декодер всегда читает ровно
    /// `GROUP_SIZE` бит флага и остановится раньше только благодаря
    /// исчерпанию `original_size` - искусственная граница группы посреди
    /// файла его десинхронизирует (декодер примет несуществующие
    /// "хвостовые" токены за настоящие и начнёт читать тела следующей,
    /// уже другой, группы как их данные).
    fn flush(&mut self, out: &mut Vec<u8>) {
        if self.count > 0 {
            out.push(self.flags);
            out.extend_from_slice(&self.bodies);
            self.flags = 0;
            self.count = 0;
            self.bodies.clear();
        }
    }

    /// Сколько байт заняла бы группа, если бы её сейчас пришлось сбросить
    /// (1 байт флагов + уже накопленные тела) - НЕ мутирует состояние.
    /// Нужно только для оценки размера пробного блока в эвристике
    /// "не сжимать несжимаемое": реальный `flush()` здесь недопустим
    /// именно потому, что мы ещё не на конце файла (см. комментарий выше).
    fn pending_len_if_flushed(&self) -> usize {
        if self.count > 0 { 1 + self.bodies.len() } else { 0 }
    }
}


/// Сбрасывает накопленный буфер подряд идущих литералов `pending` - одним
/// "сырым" блоком, если его длина оправдывает накладные расходы
/// (`MIN_LITERAL_RUN_LEN`), иначе как обычные отдельные литеральные
/// токены (для короткой последовательности сырой блок был бы ДОРОЖЕ, а
/// не дешевле - см. вывод порога у константы).
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
    fn compress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
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
        // Буфер ПОДРЯД идущих литералов, ожидающих решения "поштучно или
        // одним сырым блоком" - см. `flush_pending_literals`. Ограничен
        // `u16::MAX` (шириной поля run_len), то есть константой, не
        // зависящей от размера файла.
        let mut pending_literals: Vec<u8> = Vec::new();
        let mut pos: u64 = 0;

        // --- Эвристика 1: "не сжимать несжимаемое" (как режим STORE в ZIP) ---
        // Пробно прогоняем через LZSS первый блок (не больше одного окна -
        // см. комментарий у `SAMPLE_BLOCK_SIZE`) и НИКУДА не пишем
        // результат, пока не сравним его размер с размером самого блока:
        // решение "сжимать/не сжимать" ещё не принято, а выходной артефакт
        // пока даже не создан.
        let sample_limit = (SAMPLE_BLOCK_SIZE as u64).min(original_size);
        while pos < sample_limit {
            match compress_step(&mut artifact, &mut window, &mut chain, &mut group, &mut pending_literals, &mut out_buf, pos)? {
                Some(new_pos) => pos = new_pos,
                None => break, // файл закончился раньше конца пробного блока
            }
        }

        // ВАЖНО: ни группу, ни буфер литералов здесь принудительно НЕ
        // сбрасываем (см. `TokenGroupWriter::flush`) - иначе на границе
        // пробного блока в середине файла возникла бы неполная группа,
        // которую декодер не сможет отличить от полной, и поток
        // рассинхронизируется. Для оценки размера сэмпла достаточно
        // добавить размер ещё не сброшенных группы и буфера литералов, не
        // трогая их.
        let sample_len = pos;
        let sample_compressed_len = out_buf.len()
            + group.pending_len_if_flushed()
            + pending_literals_cost_estimate(&pending_literals);
        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "compressed");

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

            // Остаток файла (если он больше одного блока) - потоково, как есть.
            while let Some(chunk) = artifact.next_chunk()? {
                output.write_chunk_from(&chunk)?;
            }

            return Ok(output);
        }

        // Сжатие того стоит - дописываем уже посчитанные (сжатые) байты
        // сэмпла и продолжаем ровно с той же позиции, тем же окном/цепочкой
        // и тем же буфером литералов, что и при пробном проходе (никакой
        // повторной работы).
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

        // Сбрасываем то, что осталось: сначала буфер литералов (решает,
        // сырой блок или поштучно - см. `flush_pending_literals`), затем
        // незавершённую последнюю группу - иначе до 7 токенов в самом
        // конце файла потерялись бы, оставшись в `group.bodies`.
        flush_pending_literals(&mut pending_literals, &mut group, &mut out_buf);
        group.flush(&mut out_buf);
        if !out_buf.is_empty() {
            output.write_chunk_from(&out_buf)?;
        }

        Ok(output)
    }

    fn decompress(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "decompressed");

        let mut reader = BufferedArtifactReader::new(&mut artifact);
        let mode = reader.read_u8()?;
        let original_size = u64::from_le_bytes(reader.read_exact(8)?.try_into().unwrap());

        if mode == MODE_STORED {
            // Файл был сохранён как есть - копируем оставшиеся байты без
            // разбора на токены. Читаем через `reader` (а не `artifact`
            // напрямую): часть данных уже могла осесть в его внутреннем
            // буфере при чтении заголовка, и должна попасть в вывод.
            let mut copied: u64 = 0;
            while copied < original_size {
                let want = OUTPUT_FLUSH_SIZE.min((original_size - copied) as usize);
                let chunk = reader.read_exact(want)?;
                output.write_chunk_from(&chunk)?;
                copied += want as u64;
            }
            return Ok(output);
        }

        if mode != MODE_COMPRESSED {
            return Err(AppError::CorruptArchive(format!("LZSS: неизвестный режим потока: {mode}")));
        }

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

                    if offset == 0 {
                        // Литеральная последовательность (см.
                        // `TokenGroupWriter::push_literal_run`) - никакой
                        // отдельной пометки на каждый байт, просто длина
                        // и сами байты подряд.
                        let run_len = u16::from_le_bytes(reader.read_exact(2)?.try_into().unwrap()) as usize;
                        let bytes = reader.read_exact(run_len)?;
                        for byte in bytes {
                            window.push_back(byte);
                            out_buf.push(byte);
                        }
                        produced += run_len as u64;
                    } else {
                        // "+ MIN_MATCH_LEN" - обратное преобразование к тому,
                        // что делает `TokenGroupWriter::push_match` при записи.
                        let length = reader.read_u8()? as usize + MIN_MATCH_LEN;

                        if offset > window.len() {
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
                    }
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

/// Один шаг сжатия начиная с позиции `pos`: один токен - литерал (идёт в
/// буфер `pending_literals`, окончательное решение "поштучно или сырым
/// блоком" принимается позже, см. `flush_pending_literals`) либо
/// совпадение (с учётом ленивой проверки, немедленно сбрасывающее
/// накопленные литералы перед собой). Возвращает новую позицию, либо
/// `None`, если входные данные закончились.
///
/// Вынесено в отдельную функцию, чтобы один и тот же код (включая тонкую
/// логику ленивого сопоставления) использовался и для пробного сжатия
/// первого блока (эвристика "не сжимать несжимаемое"), и для сжатия всего
/// остального файла - без дублирования и без риска, что эти два места
/// незаметно разойдутся при будущих правках.
fn compress_step(
    artifact: &mut Artifact,
    window: &mut SlidingWindow,
    chain: &mut HashChain,
    group: &mut TokenGroupWriter,
    pending_literals: &mut Vec<u8>,
    out_buf: &mut Vec<u8>,
    pos: u64,
) -> Result<Option<u64>, AppError> {
    window.ensure_available(artifact, pos, LOOKAHEAD_SIZE + 2)?;
    let available = window.available_after(pos);
    if available == 0 {
        return Ok(None);
    }

    let max_len = LOOKAHEAD_SIZE.min(available).min(MAX_LENGTH);
    let (offset, length) = find_longest_match(
        window, chain, pos, WINDOW_SIZE as u64, max_len, MIN_MATCH_LEN, MAX_CHAIN_LEN,
    );

    chain.insert(window, pos);

    if length >= MIN_MATCH_LEN && length < MAX_LAZY_MATCH_LEN && available > 1 {
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
        // Совпадение прерывает (если была) серию литералов - решаем её
        // судьбу ПРЯМО СЕЙЧАС, а не после: если оставить в буфере, к
        // моменту следующего решения к ней могли бы приплюсоваться байты
        // уже ПОСЛЕ этого совпадения, что перепутало бы порядок вывода.
        flush_pending_literals(pending_literals, group, out_buf);

        group.push_match(offset, length, out_buf);

        for i in 1..length {
            let p = pos + i as u64;
            if window.available_after(p) >= 3 {
                chain.insert(window, p);
            }
        }

        new_pos = pos + length as u64;
    } else {
        pending_literals.push(window.byte_at(pos));
        maybe_flush_overflowing_run(pending_literals, group, out_buf);
        new_pos = pos + 1;
    }

    window.trim_if_needed(new_pos);
    Ok(Some(new_pos))
}

/// Буфер литералов ограничен шириной поля `run_len` (`u16`) - если он
/// достиг предела, сбрасываем его немедленно (гарантированно как сырой
/// блок, раз уж он такой длинный), чтобы не заставлять его расти
/// бесконечно на длинных несжимаемых участках. Это единственное место,
/// где размер `pending_literals` в принципе может достигать нескольких
/// десятков килобайт, - и он жёстко ограничен константой `u16::MAX`,
/// не зависящей от размера файла.
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
