use std::io;

use crate::error::AppError;
use crate::archiver::Artifact;
use crate::archiver::memory_budget::BudgetGuard; // поправьте путь под свою иерархию

// ---------------------------------------------------------------------
// SlidingWindow — ограниченный по памяти буфер входных данных
// ---------------------------------------------------------------------

/// Насколько буферу позволяется вырасти, прежде чем сработает подрезка
/// спереди. Взято с запасом (x3 от WINDOW_SIZE), чтобы подрезка происходила
/// не на каждый байт, а раз в WINDOW_SIZE*2 обработанных байт - тогда
/// суммарная стоимость подрезок амортизируется до O(1) на байт, а не
/// становится квадратичной.
fn slide_trigger(window_size: usize) -> usize {
    window_size * 3
}

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
/// Хранилище - обычный непрерывный `Vec<u8>` (а не `VecDeque`): подрезка
/// спереди (`trim_if_needed`) обходится так же амортизированно дёшево
/// (`drain` пачкой, а не поэлементно), но непрерывность памяти даёт
/// главное - `slice_from()` возвращает настоящий срез `&[u8]`, который
/// можно сравнивать блоками по 8 байт в `common_prefix_len`, а не только
/// по одному байту, как было бы с кольцевым буфером `VecDeque`.
pub struct SlidingWindow {
    buffer: Vec<u8>,
    /// Абсолютная позиция в потоке, которой соответствует buffer[0].
    base_pos: u64,
    exhausted: bool,
    window_size: usize,
    _guard: BudgetGuard,
}

impl SlidingWindow {
    pub fn new(window_size: usize, lookahead_size: usize) -> Result<Self, AppError> {
        let capacity = slide_trigger(window_size) + lookahead_size;
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
            _guard: guard,
        })
    }

    pub fn ensure_available(&mut self, artifact: &mut Artifact, pos: u64, want: usize) -> io::Result<()> {
        while !self.exhausted && self.available_after(pos) < want {
            match artifact.read_next_chunk()? {
                Some(chunk) => self.buffer.extend_from_slice(&chunk),
                None => self.exhausted = true,
            }
        }
        Ok(())
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
        let local = (pos - self.base_pos) as usize;
        let trigger = slide_trigger(self.window_size);
        if local > trigger {
            let drop_count = local - self.window_size;
            self.buffer.drain(0..drop_count); // пачкой, не по одному элементу
            self.base_pos += drop_count as u64;
        }
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
        let wa = u64::from_ne_bytes(a[len..len + 8].try_into().unwrap());
        let wb = u64::from_ne_bytes(b[len..len + 8].try_into().unwrap());
        let diff = wa ^ wb;
        if diff != 0 {
            // trailing_zeros/8 - номер первого несовпадающего байта внутри
            // слова. Корректно для little-endian (x86_64/ARM); для
            // переносимости на big-endian здесь нужна была бы отдельная
            // ветка с leading_zeros.
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
    let mut best_len = 0usize;
    let mut best_offset = 0usize;

    for (checked, cand_pos) in chain.candidates(window, pos, window_start).enumerate() {
        if checked >= max_chain_len {
            break;
        }

        let len = common_prefix_len(window.slice_from(cand_pos), window.slice_from(pos), max_len);

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
}

impl<'a> BufferedArtifactReader<'a> {
    pub fn new(artifact: &'a mut Artifact) -> Self {
        Self { artifact, buf: Vec::new(), pos: 0 }
    }

    pub fn read_exact(&mut self, n: usize) -> Result<Vec<u8>, AppError> {
        while self.buf.len() - self.pos < n {
            if self.pos > 0 {
                self.buf.drain(0..self.pos);
                self.pos = 0;
            }
            match self.artifact.read_next_chunk()? {
                Some(chunk) => self.buf.extend_from_slice(&chunk),
                None => {
                    return Err(AppError::CorruptArchive(
                        "LZSS: неожиданный конец потока при чтении токена".to_string(),
                    ))
                }
            }
        }
        let result = self.buf[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(result)
    }

    pub fn read_u8(&mut self) -> Result<u8, AppError> {
        Ok(self.read_exact(1)?[0])
    }
}


pub fn flush_if_needed(out_buf: &mut Vec<u8>, output: &mut Artifact, output_flush_size: usize) -> Result<(), AppError> {
    if out_buf.len() >= output_flush_size {
        output.write_chunk(out_buf)?;
        out_buf.clear();
    }
    Ok(())
}
