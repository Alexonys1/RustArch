use std::io;

use crate::error::AppError;
use crate::archiver::Artifact;
use crate::archiver::memory_budget::BudgetGuard; // поправьте путь под свою иерархию

// ---------------------------------------------------------------------
// SlidingWindow — ограниченный по памяти буфер входных данных
// ---------------------------------------------------------------------

/// Читаем вход сравнительно крупными блоками, но не позволяем размеру
/// `Artifact::chunk_size` (по умолчанию 512 КиБ) раздувать окно.
const INPUT_READ_SIZE: usize = 64 * 1024;

/// Держит в памяти ТОЛЬКО то, что реально может понадобиться алгоритму:
/// до `window_size` байт уже пройденной "истории" позади текущей позиции
/// и какое-то количество байт упреждающего просмотра впереди. Размер
/// буфера НЕ зависит от размера обрабатываемого файла - в этом всё дело:
/// раньше LZSS читал файл целиком в `Vec<u8>`, из-за чего 2.7 ГБ файл
/// занимал 2.7 ГБ оперативной памяти только под входные данные.
/// Держит в памяти ТОЛЬКО то, что реально может понадобиться алгоритму:
/// до `window_size` байт уже пройденной "истории" позади текущей позиции
/// и какое-то количество байт упреждающего просмотра впереди. Размер
/// буфера НЕ зависит от размера обрабатываемого файла.
///
/// Хранилище — непрерывный `Vec<u8>` (не `VecDeque`). Оно компактируется
/// через `copy_within` только перед refill, поэтому `slice_from()` всегда
/// возвращает настоящий срез `&[u8]` для 8-байтового сравнения.
pub struct SlidingWindow {
    buffer: Vec<u8>,
    /// Абсолютная позиция в потоке, которой соответствует buffer[0].
    base_pos: u64,
    exhausted: bool,
    window_size: usize,
    capacity: usize,
    _guard: BudgetGuard,
}

impl SlidingWindow {
    pub fn new(window_size: usize, lookahead_size: usize) -> Result<Self, AppError> {
        let capacity = window_size
            .checked_add(INPUT_READ_SIZE)
            .and_then(|v| v.checked_add(lookahead_size))
            .ok_or_else(|| AppError::Compression(
                "LZ: переполнение при вычислении размера скользящего окна".into(),
            ))?;
        let mut guard = BudgetGuard::default();
        if !guard.try_grow(capacity as i64) {
            return Err(AppError::Compression(format!(
                "LZSS: не удалось зарезервировать {capacity} байт под скользящее окно - бюджет памяти исчерпан"
            )));
        }
        Ok(Self {
            buffer: Vec::with_capacity(capacity),
            base_pos: 0,
            exhausted: false,
            window_size,
            capacity,
            _guard: guard,
        })
    }

    pub fn ensure_available(&mut self, artifact: &mut Artifact, pos: u64, want: usize) -> io::Result<()> {
        if self.available_after(pos) < want && !self.exhausted {
            self.compact_for(pos);
        }

        while !self.exhausted && self.available_after(pos) < want {
            let free = self.capacity.saturating_sub(self.buffer.len());
            if free == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "LZ: недостаточная ёмкость скользящего окна",
                ));
            }

            let to_read = free.min(INPUT_READ_SIZE);
            let old_len = self.buffer.len();
            self.buffer.resize(old_len + to_read, 0);
            let read = artifact.read_chunk_to(&mut self.buffer[old_len..])?;
            self.buffer.truncate(old_len + read);
            if read == 0 {
                self.exhausted = true;
            }
        }
        Ok(())
    }

    fn compact_for(&mut self, pos: u64) {
        let keep_from = pos.saturating_sub(self.window_size as u64);
        if keep_from <= self.base_pos {
            return;
        }

        let drop_count = (keep_from - self.base_pos) as usize;
        self.buffer.copy_within(drop_count.., 0);
        self.buffer.truncate(self.buffer.len() - drop_count);
        self.base_pos = keep_from;
    }

    pub fn available_after(&self, pos: u64) -> usize {
        let local = (pos - self.base_pos) as usize;
        self.buffer.len().saturating_sub(local)
    }

    pub fn byte_at(&self, pos: u64) -> u8 {
        self.buffer[(pos - self.base_pos) as usize]
    }

    /// Непрерывный срез от позиции `pos` до конца буферизованных данных -
    /// именно он и делает возможным быстрое блочное сравнение совпадений
    /// в `common_prefix_len` вместо побайтового цикла.
    pub fn slice_from(&self, pos: u64) -> &[u8] {
        let local = (pos - self.base_pos) as usize;
        &self.buffer[local..]
    }

    pub fn trim_if_needed(&mut self, pos: u64) {
        // Не двигаем память на горячем пути каждого токена. История
        // компактируется один раз непосредственно перед refill.
        debug_assert!(pos >= self.base_pos);
    }
}

// ---------------------------------------------------------------------
// HashChain — поиск совпадений без HashMap/SipHash
// ---------------------------------------------------------------------

/// Размер таблицы голов хэш-цепочек. Фиксированная константа - в отличие
/// от `HashMap<[u8;3], Vec<usize>>`, эта структура не растёт вместе с
/// количеством уникальных 3-байтовых префиксов в файле.
const HASH_BITS: u32 = 16;
const HASH_SIZE: usize = 1 << HASH_BITS;

/// Значение-метка "цепочка закончилась" (u64::MAX никогда не встретится
/// как реальная позиция в файле).
const NONE: u64 = u64::MAX;

/// Классическая схема хэш-цепочек (как в zlib): `head[hash]` - самая
/// свежая позиция с данным хэшем, `prev[pos % window_size]` - позиция
/// предыдущего вхождения ТОГО ЖЕ хэша, что и в позиции `pos`. Обе таблицы
/// фиксированного размера (`HASH_SIZE` + `window_size`), поэтому память
/// ограничена константой независимо от размера входного файла - в отличие
/// от `HashMap<[u8;3], Vec<usize>>`, где на файле с частыми повторами
/// количество и суммарная длина бакетов росли неограниченно.
///
/// Дополнительный выигрыш: обычный `HashMap` в Rust использует SipHash -
/// специально медленный (криптостойкий) хэшер. Здесь используется простое
/// мультипликативное хэширование трёх байт, на порядки быстрее на горячем
/// пути, вызываемом на каждую позицию файла.
pub struct HashChain {
    head: Vec<u64>,
    prev: Vec<u64>,
    window_size: usize,
    _guard: BudgetGuard,
}

impl HashChain {
    pub fn new(window_size: usize) -> Result<Self, AppError> {
        let bytes = (HASH_SIZE + window_size) * std::mem::size_of::<u64>();
        let mut guard = BudgetGuard::default();
        if !guard.try_grow(bytes as i64) {
            return Err(AppError::Compression(format!(
                "LZSS: не удалось зарезервировать {bytes} байт под хэш-таблицу - бюджет памяти исчерпан"
            )));
        }
        Ok(Self {
            head: vec![NONE; HASH_SIZE],
            prev: vec![NONE; window_size],
            window_size,
            _guard: guard,
        })
    }

    fn hash3(b0: u8, b1: u8, b2: u8) -> usize {
        let v = (b0 as u32) | ((b1 as u32) << 8) | ((b2 as u32) << 16);
        // Мультипликативное хэширование (Кнут) - быстро, без коллизий-ловушек
        // для обычных данных, и не требует криптостойкости SipHash.
        ((v.wrapping_mul(2_654_435_761)) >> (32 - HASH_BITS)) as usize
    }

    /// Индексирует позицию `pos` по её 3-байтовому префиксу. Вызывающий
    /// код обязан убедиться, что байты `pos..pos+3` доступны в окне.
    pub fn insert(&mut self, window: &SlidingWindow, pos: u64) {
        if window.available_after(pos) < 3 {
            return;
        }
        let (b0, b1, b2) = (window.byte_at(pos), window.byte_at(pos + 1), window.byte_at(pos + 2));
        let h = Self::hash3(b0, b1, b2);
        let slot = (pos as usize) % self.window_size;
        self.prev[slot] = self.head[h];
        self.head[h] = pos;
    }

    /// Итератор по цепочке кандидатов для позиции `pos`, от самого свежего
    /// к самому старому, останавливающийся на границе окна `window_start`.
    fn candidates(&self, window: &SlidingWindow, pos: u64, window_start: u64) -> impl Iterator<Item = u64> + '_ {
        let (b0, b1, b2) = (window.byte_at(pos), window.byte_at(pos + 1), window.byte_at(pos + 2));
        let h = Self::hash3(b0, b1, b2);
        let mut cur = self.head[h];
        let window_size = self.window_size;
        let prev = &self.prev;
        std::iter::from_fn(move || {
            if cur == NONE || cur < window_start {
                return None;
            }
            let result = cur;
            cur = prev[(cur as usize) % window_size];
            Some(result)
        })
    }
}

/// Сравнивает две последовательности байт и возвращает длину общего
/// префикса (не больше `max_len`). Сравнивает по 8 байт за раз через XOR
/// вместо побайтового цикла - на длинных совпадениях (частых в
/// структурированных бинарных данных: повторяющиеся блоки, выровненные
/// нулевые области) это даёт на порядок меньше итераций, чем цикл
/// "сравнили байт - сдвинулись на один". Тот же приём используется в
/// zlib-ng/lz4 для ускорения именно этого горячего пути.
fn common_prefix_len(a: &[u8], b: &[u8], max_len: usize) -> usize {
    let limit = max_len.min(a.len()).min(b.len());
    let mut len = 0;

    while len + 8 <= limit {
        let wa = u64::from_le_bytes(a[len..len + 8].try_into().unwrap());
        let wb = u64::from_le_bytes(b[len..len + 8].try_into().unwrap());
        let diff = wa ^ wb;
        if diff != 0 {
            // Благодаря from_le_bytes trailing_zeros/8 — номер первого
            // несовпадающего байта на любой endian-архитектуре.
            return len + (diff.trailing_zeros() / 8) as usize;
        }
        len += 8;
    }

    while len < limit && a[len] == b[len] {
        len += 1;
    }

    len
}

/// Если найдено совпадение такой длины или больше - прекращаем перебор
/// цепочки кандидатов немедленно (аналог `nice_match` в zlib). Более
/// длинное совпадение технически возможно, но искать его среди оставшихся
/// кандидатов почти никогда не окупается: выигрыш в сжатии исчезающе мал
/// по сравнению со стоимостью полного прохода по цепочке.
const NICE_MATCH_LEN: usize = 128;
const GOOD_MATCH_LEN: usize = 32;

/// Ищет самое длинное совпадение для данных, начинающихся в `pos`.
/// Возвращает (offset, length); (0, 0), если совпадения длиной
/// >= `min_match_len` не нашлось.
pub fn find_longest_match(
    window: &SlidingWindow,
    chain: &HashChain,
    pos: u64,
    window_size: u64,
    max_len: usize,
    min_match_len: usize,
    max_chain_len: usize,
) -> (usize, usize)
{
    if max_len < min_match_len || window.available_after(pos) < min_match_len {
        return (0, 0);
    }

    let window_start = pos.saturating_sub(window_size);
    let current = window.slice_from(pos);
    let mut best_len = 0usize;
    let mut best_offset = 0usize;

    // Самый частый случай на низкоэнтропийных данных — серия одного
    // байта. Для offset=1 длинное совпадение можно принять без обхода
    // хэш-цепочки.
    if pos > window_start && window.byte_at(pos - 1) == current[0] {
        let len = common_prefix_len(window.slice_from(pos - 1), current, max_len);
        if len >= min_match_len {
            best_len = len;
            best_offset = 1;
            if len >= NICE_MATCH_LEN || len == max_len {
                return (best_offset, best_len);
            }
        }
    }

    for (checked, cand_pos) in chain.candidates(window, pos, window_start).enumerate() {
        if checked >= max_chain_len {
            break;
        }

        // Аналог good_match из zlib: после хорошего совпадения оставляем
        // только четверть исходного бюджета цепочки.
        if best_len >= GOOD_MATCH_LEN && checked >= max_chain_len.div_ceil(4) {
            break;
        }
        if cand_pos >= pos || (best_offset == 1 && cand_pos + 1 == pos) {
            continue;
        }

        let candidate = window.slice_from(cand_pos);
        // Отбрасываем коллизии 16-битного хэша и кандидаты, которые уже
        // не могут улучшить найденную длину.
        if candidate[0] != current[0]
            || candidate[1] != current[1]
            || candidate[2] != current[2]
            || (best_len > 0 && candidate[best_len] != current[best_len])
        {
            continue;
        }

        let len = common_prefix_len(candidate, current, max_len);

        if len > best_len {
            best_len = len;
            best_offset = (pos - cand_pos) as usize;
            if best_len >= NICE_MATCH_LEN || best_len == max_len {
                break; // "достаточно длинное" совпадение - дальше не ищем
            }
        }
    }

    if best_len >= min_match_len {
        (best_offset, best_len)
    } else {
        (0, 0)
    }
}

// ---------------------------------------------------------------------
// BufferedArtifactReader — читает токены декомпрессии пачками, а не по
// 1-2 байта напрямую из Artifact (который на File-состоянии делает
// seek()+read() НА КАЖДЫЙ такой вызов - это и был отдельный источник
// торможения, не связанный с памятью).
// ---------------------------------------------------------------------

pub struct BufferedArtifactReader<'a> {
    artifact: &'a mut Artifact,
    buf: Vec<u8>,
    pos: usize,
    end: usize,
}

impl<'a> BufferedArtifactReader<'a> {
    pub fn new(artifact: &'a mut Artifact) -> Self {
        Self {
            artifact,
            buf: vec![0; INPUT_READ_SIZE],
            pos: 0,
            end: 0,
        }
    }

    fn fill(&mut self, need: usize) -> Result<(), AppError> {
        while self.end - self.pos < need {
            if self.pos > 0 {
                self.buf.copy_within(self.pos..self.end, 0);
                self.end -= self.pos;
                self.pos = 0;
            }

            let missing = need - (self.end - self.pos);
            if self.buf.len() - self.end < missing {
                self.buf.resize((self.end + missing).max(INPUT_READ_SIZE), 0);
            }

            let read = self.artifact.read_chunk_to(&mut self.buf[self.end..])?;
            if read == 0 {
                return Err(AppError::CorruptArchive(
                    "LZ: неожиданный конец потока при чтении токена".to_string(),
                ));
            }
            self.end += read;
        }
        Ok(())
    }

    pub fn read_u8(&mut self) -> Result<u8, AppError> {
        self.fill(1)?;
        let value = self.buf[self.pos];
        self.pos += 1;
        Ok(value)
    }

    pub fn read_u16_le(&mut self) -> Result<u16, AppError> {
        self.fill(2)?;
        let value = u16::from_le_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        Ok(value)
    }

    pub fn read_u64_le(&mut self) -> Result<u64, AppError> {
        self.fill(8)?;
        let value = u64::from_le_bytes(self.buf[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        Ok(value)
    }

    /// Добавляет ровно `n` байт в существующий выходной буфер без
    /// промежуточного Vec и без аллокации на каждый литеральный блок.
    pub fn append_exact(&mut self, mut n: usize, out: &mut Vec<u8>) -> Result<(), AppError> {
        out.reserve(n);
        while n > 0 {
            if self.pos == self.end {
                self.fill(1)?;
            }
            let take = n.min(self.end - self.pos);
            out.extend_from_slice(&self.buf[self.pos..self.pos + take]);
            self.pos += take;
            n -= take;
        }
        Ok(())
    }
}


/// Фиксированное кольцевое окно декодера. В отличие от VecDeque не
/// выполняет pop_front для каждого байта.
pub struct DecodeHistory {
    data: Vec<u8>,
    write: usize,
    len: usize,
}

impl DecodeHistory {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0);
        Self { data: vec![0; capacity], write: 0, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn push(&mut self, byte: u8) {
        self.data[self.write] = byte;
        self.write += 1;
        if self.write == self.data.len() {
            self.write = 0;
        }
        self.len = (self.len + 1).min(self.data.len());
    }

    pub fn extend(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.push(byte);
        }
    }

    pub fn copy_match(
        &mut self,
        offset: usize,
        length: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), AppError> {
        if offset == 0 || offset > self.len {
            return Err(AppError::CorruptArchive(
                "LZ: некорректный offset ссылки назад".to_string(),
            ));
        }

        out.reserve(length);
        for _ in 0..length {
            let index = (self.write + self.data.len() - offset) % self.data.len();
            let byte = self.data[index];
            self.push(byte);
            out.push(byte);
        }
        Ok(())
    }
}


pub fn flush_if_needed(out_buf: &mut Vec<u8>, output: &mut Artifact, output_flush_size: usize) -> Result<(), AppError> {
    if out_buf.len() >= output_flush_size {
        output.write_chunk_from(out_buf)?;
        out_buf.clear();
    }
    Ok(())
}



// =====================================================================
// ReverseChain — хэш-цепочка для ОБРАТНЫХ совпадений
// =====================================================================
//
// Отличие от HashChain: позиция `b` индексируется по ОБРАТНОМУ 3-грамму
// (window[b], window[b-1], window[b-2]), а не по прямому
// (window[b], window[b+1], window[b+2]). Это позволяет по 3-байтовому
// префиксу lookahead-а (input[pos..pos+3]) находить в окне такие `b`,
// что window[b]=input[pos], window[b-1]=input[pos+1], window[b-2]=input[pos+2],
// т.е. кандидатов на ОБРАТНОЕ совпадение:
//     window[a + i] == input[pos + L - 1 - i]   для i in 0..L,
// где a = b - L + 1.
//
// Память: те же два массива фиксированного размера, что и у HashChain
// (HASH_SIZE + window_size записей u64). Trim не нужен: старые позиции
// отсекаются на этапе candidates() по границе окна.
pub struct ReverseChain {
    head: Vec<u64>,
    prev: Vec<u64>,
    window_size: usize,
    _guard: BudgetGuard,
}

impl ReverseChain {
    pub fn new(window_size: usize) -> Result<Self, AppError> {
        let bytes = (HASH_SIZE + window_size) * std::mem::size_of::<u64>();
        let mut guard = BudgetGuard::default();
        if !guard.try_grow(bytes as i64) {
            return Err(AppError::Compression(format!(
                "LZSS: не удалось зарезервировать {bytes} байт под обратную хэш-таблицу - бюджет памяти исчерпан"
            )));
        }
        Ok(Self {
            head: vec![NONE; HASH_SIZE],
            prev: vec![NONE; window_size],
            window_size,
            _guard: guard,
        })
    }

    fn hash3(b0: u8, b1: u8, b2: u8) -> usize {
        let v = (b0 as u32) | ((b1 as u32) << 8) | ((b2 as u32) << 16);
        ((v.wrapping_mul(2_654_435_761)) >> (32 - HASH_BITS)) as usize
    }

    /// Индексирует позицию `pos` как КОНЕЦ обратного 3-грамма:
    /// хэшируется тройка (window[pos], window[pos-1], window[pos-2]).
    /// Требует pos >= 2 и чтобы байты pos-2..=pos были в буфере.
    ///
    /// Инвариант: компрессор вызывает insert() только для позиций,
    /// прошедших `SlidingWindow::trim_if_needed` (или находящихся в
    /// начале потока), поэтому `pos - 2 >= base_pos` здесь всегда
    /// выполняется и `byte_at(pos - 2)` не паникует.
    pub fn insert(&mut self, window: &SlidingWindow, pos: u64) {
        if pos < 2 {
            return;
        }
        if window.available_after(pos - 2) < 3 {
            return;
        }
        let b0 = window.byte_at(pos);
        let b1 = window.byte_at(pos - 1);
        let b2 = window.byte_at(pos - 2);
        let h = Self::hash3(b0, b1, b2);
        let slot = (pos as usize) % self.window_size;
        self.prev[slot] = self.head[h];
        self.head[h] = pos;
    }

    /// Итератор по кандидатам `b` (КОНЕЦ обратного региона в окне),
    /// упорядоченным от самых свежих к самым старым. Останавливается,
    /// когда очередная позиция уходит за границу `window_start`.
    fn candidates(
        &self,
        window: &SlidingWindow,
        pos: u64,
        window_start: u64,
    ) -> impl Iterator<Item = u64> + '_ {
        let (b0, b1, b2) = (
            window.byte_at(pos),
            window.byte_at(pos + 1),
            window.byte_at(pos + 2),
        );
        let h = Self::hash3(b0, b1, b2);
        let mut cur = self.head[h];
        let window_size = self.window_size;
        let prev = &self.prev;
        std::iter::from_fn(move || {
            if cur == NONE || cur < window_start {
                return None;
            }
            let result = cur;
            cur = prev[(cur as usize) % window_size];
            Some(result)
        })
    }

    /// No-op. Нужен для симметрии с API HashChain: устаревшие записи
    /// отфильтровываются в `candidates` по `window_start`, а `prev`
    /// использует модульную индексацию и перезаписывается естественным
    /// образом на каждом insert.
    pub fn trim_if_needed(&mut self, _pos: u64) {}
}


// =====================================================================
// НОВОЕ: поиск ОБРАТНЫХ совпадений (используется в lzss_new.rs)
// =====================================================================
//
// Всё выше этой черты - неизменная существующая реализация. Ниже -
// единственная добавленная функция, использующая уже существующую
// структуру `ReverseChain` (она была объявлена в файле раньше, но без
// собственного алгоритма поиска - только индексация). Симметрична
// `find_longest_match` выше, но:
//   - использует `ReverseChain` вместо `HashChain`;
//   - сравнивает байты окна в УБЫВАЮЩЕМ порядке индексов против входных
//     данных в ВОЗРАСТАЮЩЕМ порядке (см. схему в комментарии к
//     `ReverseChain`), а не два среза в одном и том же порядке;
//   - обязана дополнительно ограничивать длину физической границей
//     буфера ПОЗАДИ кандидата (`available_before`) - обратное совпадение,
//     в отличие от прямого, не может самоссылаться в ещё не
//     декодированные байты (весь исходный фрагмент лежит строго в уже
//     пройденной истории), поэтому у него нет права "занимать" данные
//     резервированием произвольной длины - оно жёстко ограничено тем,
//     что физически ещё есть в буфере слева от кандидата.

impl SlidingWindow {
    /// Сколько байт "истории" физически ещё присутствует в буфере ДО и
    /// ВКЛЮЧАЯ позицию `pos` (т.е. `pos - base_pos + 1`), с учётом уже
    /// выполненных подрезок `trim_if_needed`. Нужно только для поиска
    /// обратных совпадений - см. комментарий к `find_longest_reverse_match`.
    pub fn available_before(&self, pos: u64) -> usize {
        (pos.saturating_sub(self.base_pos) + 1) as usize
    }
}


