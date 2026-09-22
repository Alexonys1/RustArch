//! Модуль `reed_solomon` - код Рида-Соломона над GF(256), реализующий
//! интерфейс [`ErrorCorrectionCode`] проекта.
//!
//! # Поле
//!
//! GF(256) строится над характеристикой 2 с порождающим многочленом
//! x^8 + x^4 + x^3 + x^2 + 1 = 0x11D = 285 (тот же многочлен, что
//! используется в QR-кодах и большинстве классических реализаций RS).
//! Элемент 2 - примитивный элемент поля (порождает все 255 ненулевых
//! элементов при последовательном возведении в степень), поэтому таблица
//! логарифмов строится именно по степеням двойки: `exp[i] = 2^i`, для
//! `i` от 0 до 255 (255-я степень уже равна 1 - цикл замыкается, значение
//! дублируется для степеней с 256 по 510, чтобы умножение/деление в поле
//! не требовало операции `% 255` на каждый вызов - см. `Gf256::new`).
//!
//! # Локализация и исправление ошибок
//!
//! - Синдромы вычисляются прямым вычислением кодового слова в точках
//!   `2^0 .. 2^(nsym-1)` (`decode_block`).
//! - Многочлен-локатор ошибок Λ(x) находится алгоритмом
//!   Берлекэмпа-Мэсси (`berlekamp_massey`) - классическая итеративная
//!   форма, восстанавливающая кратчайший регистр сдвига с обратной
//!   связью, порождающий известные синдромы.
//! - Корни Λ(x) (а с ними и позиции ошибок) ищутся перебором Ченя
//!   (`find_errors`) - полным перебором всех `n` возможных позиций
//!   кодового слова, т.к. `n <= 255` и перебор тривиально дёшев.
//! - Амплитуды (величины) найденных ошибок считаются по формуле Форни
//!   (`correct_errors`): `Y_l = X_l * Ω(X_l^-1) / Λ'(X_l^-1)`, где Ω(x) -
//!   многочлен-оценщик ошибок (`S(x)*Λ(x) mod x^nsym`), а Λ'(x) -
//!   формальная производная локатора.
//!
//! # Формат блоков
//!
//! Каждый полный блок использует профиль RS(255,223): 223 информационных
//! символа и 32 символа чётности, что позволяет исправить до 16 ошибочных
//! символов. Последний блок кодируется в укороченной форме: к оставшимся
//! `1..=222` информационным символам добавляются те же 32 символа чётности.
//! Благодаря этому длина исходных данных однозначно выводится из длины
//! закодированного [`Artifact`], и отдельный заголовок размера не требуется.
//! Параметры поля и корни порождающего многочлена остаются внутренним
//! форматом RustArch; совместимость с битовым представлением CCSDS здесь
//! не заявляется.
//!
//! # Память
//!
//! Блоки RS независимы друг от друга (как и блоки Хэмминга), поэтому
//! файл обрабатывается потоково через `Artifact::read_chunk_to`/`write_chunk_from`
//! блок за блоком, без чтения файла в память целиком. Буферы одного
//! блока (`k`/`n <= 255` байт) выделяются ОДИН РАЗ перед циклом и
//! переиспользуются на каждой итерации; таблицы поля (`Gf256`) и
//! порождающий многочлен строятся один раз на файл, а не на блок.
//! Единственный буфер, чей размер не завязан на один блок, - это
//! `out_buf`, накопитель перед сбросом в артефакт, ограниченный константой
//! `OUTPUT_FLUSH_SIZE` независимо от размера файла.

use crate::error::AppError;
use crate::archiver::Artifact;
use super::FecId;

use super::{ErrorCorrectionCode, FecReport};


/// Порождающий многочлен GF(256): x^8 + x^4 + x^3 + x^2 + 1 = 285.
const GF_GENZ_ERATOR_POLY: u16 = 0x11D;

/// Порог сброса накопленного выходного буфера в артефакт - ограничивает
/// пиковую дополнительную память константой, а не размером файла.
const OUTPUT_FLUSH_SIZE: usize = 512 * 1024;
const DATA_SYMBOLS: usize = 223;
const PARITY_SYMBOLS: usize = 32;
const CODEWORD_SYMBOLS: usize = DATA_SYMBOLS + PARITY_SYMBOLS;


/// Поле Галуа GF(256) с примитивным элементом 2. Таблицы строятся один
/// раз на вызов `encode`/`decode` (256 итераций, пренебрежимо быстро) и
/// переиспользуются для всех блоков файла.
struct Gf256 {
    /// Удвоенная (512 вместо 256) таблица степеней двойки - позволяет
    /// умножать без операции `% 255`: сумма двух логарифмов (каждый
    /// 0..=254) не превышает 508, что помещается в удвоенную таблицу.
    exp: [u8; 512],
    /// Таблица логарифмов по основанию 2. `log[0]` не используется
    /// (логарифм нуля не определён - умножение/деление на 0 всегда
    /// обрабатывается отдельной веткой).
    log: [u8; 256],
}

impl Gf256 {
    fn new() -> Self {
        let mut exp = [0u8; 512];
        let mut log = [0u8; 256];

        let mut x: u16 = 1;
        for i in 0..255usize {
            exp[i] = x as u8;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= GF_GENZ_ERATOR_POLY;
            }
        }
        // 2^255 = 2^0 = 1 - цикл замкнулся; дублируем таблицу дальше, чтобы
        // exp[la + lb] всегда было в диапазоне без явного "% 255".
        for i in 255..512 {
            exp[i] = exp[i - 255];
        }

        Self { exp, log }
    }

    #[inline]
    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            0
        } else {
            self.exp[self.log[a as usize] as usize + self.log[b as usize] as usize]
        }
    }

    #[inline]
    fn div(&self, a: u8, b: u8) -> u8 {
        debug_assert!(b != 0, "деление на ноль в GF(256)");
        if a == 0 {
            0
        } else {
            let la = self.log[a as usize] as i32;
            let lb = self.log[b as usize] as i32;
            let mut diff = la - lb;
            if diff < 0 {
                diff += 255;
            }
            self.exp[diff as usize]
        }
    }

    /// `a^n` в поле. Показатель `n` может быть отрицательным (даёт
    /// обратный элемент в соответствующей степени) - используется при
    /// построении порождающего многочлена и в переборе Ченя.
    #[inline]
    fn pow(&self, a: u8, n: i32) -> u8 {
        if a == 0 {
            return if n == 0 { 1 } else { 0 };
        }
        let l = self.log[a as usize] as i64;
        let mut e = (l * n as i64).rem_euclid(255);
        if e < 0 {
            e += 255;
        }
        self.exp[e as usize]
    }
}


/// Умножение многочленов (свёртка) - используется только для построения
/// порождающего многочлена (один раз на файл, degree <= 255) и для
/// вычисления Ω(x) при декодировании одного блока. Оба многочлена и
/// результат хранятся С МЛАДШЕГО коэффициента ПЕРВЫМ (poly[0] - свободный
/// член) - т.е. как раз то представление, в котором работает `S(x)`,
/// `Λ(x)` и `Ω(x)` в этом модуле.
fn poly_mul(gf: &Gf256, a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; a.len() + b.len() - 1];
    for (i, &ai) in a.iter().enumerate() {
        if ai == 0 {
            continue;
        }
        for (j, &bj) in b.iter().enumerate() {
            if bj == 0 {
                continue;
            }
            result[i + j] ^= gf.mul(ai, bj);
        }
    }
    result
}

/// Значение многочлена (МЛАДШИЙ коэффициент первым, poly[0] - свободный
/// член) в точке `x`.
fn poly_eval_ascending(gf: &Gf256, poly: &[u8], x: u8) -> u8 {
    let mut y = 0u8;
    let mut x_pow = 1u8;
    for &c in poly {
        y ^= gf.mul(c, x_pow);
        x_pow = gf.mul(x_pow, x);
    }
    y
}

/// Значение многочлена, заданного СТАРШИМ коэффициентом первым (обычный
/// порядок байт кодового слова: `codeword[0]` - самый старший разряд),
/// в точке `x` - метод Горнера. Используется только для вычисления
/// синдромов из принятого кодового слова.
fn poly_eval_descending(gf: &Gf256, poly: &[u8], x: u8) -> u8 {
    let mut y = poly[0];
    for &c in &poly[1..] {
        y = gf.mul(y, x) ^ c;
    }
    y
}

/// Формальная производная многочлена (младший коэффициент первым). Над
/// полем характеристики 2 выживают только члены с НЕЧЁТНОЙ степенью
/// (чётные коэффициенты производной всегда умножаются на 0 mod 2).
fn poly_formal_derivative(poly: &[u8]) -> Vec<u8> {
    if poly.len() <= 1 {
        return vec![0];
    }
    let mut result = vec![0u8; poly.len() - 1];
    let mut i = 1;
    while i < poly.len() {
        result[i - 1] = poly[i];
        i += 2;
    }
    result
}

/// Порождающий многочлен RS: произведение (x - α^i) = (x + α^i) для
/// i = 0..nsym-1 (характеристика 2). Строится один раз на файл.
fn build_genzerator(gf: &Gf256, nsym: usize) -> Vec<u8> {
    // Здесь удобнее многочлен СО СТАРШИМ коэффициентом первым (так же,
    // как в классическом описании алгоритма кодирования ниже) - но
    // конкретный порядок неважен, т.к. это единственное место, где
    // `genz` используется именно в этом порядке (`rs_encode_block`).
    let mut g = vec![1u8];
    for i in 0..nsym {
        g = poly_mul_descending(gf, &g, &[1, gf.pow(2, i as i32)]);
    }
    g
}

/// То же умножение многочленов, но со старшим коэффициентом первым -
/// нужно только для построения порождающего многочлена и кодирования
/// (см. `build_genzerator`, `rs_encode_block`), где традиционно принят
/// именно такой порядок (совпадает с порядком байт в самом файле:
/// первый байт данных - самый значимый).
fn poly_mul_descending(gf: &Gf256, a: &[u8], b: &[u8]) -> Vec<u8> {
    poly_mul(gf, a, b)
}

/// Кодирует один блок из `data.len()` байт данных, дописывая
/// `genz.len() - 1` байт чётности. Реализация - классическое
/// "систематическое" кодирование через деление многочлена данных
/// (сдвинутого на `nsym` разрядов) на порождающий многочлен: остаток
/// деления и есть чётность. Работает целиком в одном буфере длиной
/// `k + nsym` (не более 255 байт) - без аллокаций, пропорциональных
/// размеру файла.
fn rs_encode_block(gf: &Gf256, data: &[u8], r#genz: &[u8]) -> Vec<u8> {
    let nsym = r#genz.len() - 1;
    let mut msg_out = vec![0u8; data.len() + nsym];
    msg_out[..data.len()].copy_from_slice(data);

    for i in 0..data.len() {
        let coef = msg_out[i];
        if coef != 0 {
            for (j, &g) in r#genz.iter().enumerate() {
                msg_out[i + j] ^= gf.mul(g, coef);
            }
        }
    }

    // В процессе деления область данных в `msg_out` перезаписывается
    // промежуточными значениями - восстанавливаем её оригинальными
    // байтами (нужен только остаток - последние `nsym` байт).
    msg_out[..data.len()].copy_from_slice(data);
    msg_out
}


/// Алгоритм Берлекэмпа-Мэсси: по синдромам `S_0..S_{nsym-1}` находит
/// многочлен-локатор ошибок Λ(x) (коэффициенты от МЛАДШЕЙ степени к
/// старшей, `Λ[0] = 1`) минимальной степени, порождающий эти синдромы.
/// Возвращает `(Λ, L)`, где `L` - степень локатора (при успешном
/// декодировании - число реальных ошибок в блоке).
///
/// Классическая итеративная форма (см., например, Lin & Costello,
/// "Error Control Coding"): на каждом шаге считается невязка
/// (discrepancy) между текущим приближением локатора и очередным
/// синдромом; если она ненулевая и текущая длина регистра `L` не может
/// её объяснить, локатор обновляется поправкой на предыдущее
/// "опорное" приближение `B(x)`.
fn berlekamp_massey(gf: &Gf256, syndromes: &[u8]) -> (Vec<u8>, usize) {
    let nsym = syndromes.len();

    // `lambda`/`b` заведомо не короче нужного: при корректируемых входных
    // данных степень локатора никогда не превышает `nsym / 2`, но при
    // сильно "испорченных" (заведомо неисправимых) синдромах
    // промежуточные сдвиги регистра теоретически могли бы захотеть выйти
    // за пределы массива - явные границы в цикле ниже (`upper`) на этот
    // случай просто отбрасывают то, что не помещается, вместо паники;
    // итоговая степень `L` в таких случаях всё равно превысит `nsym / 2`
    // и блок будет корректно помечен неисправимым чуть выше по стеку.
    let mut lambda = vec![0u8; nsym + 1];
    let mut b = vec![0u8; nsym + 1];
    lambda[0] = 1;
    b[0] = 1;

    let mut l: usize = 0;
    let mut m: usize = 1;
    let mut b_coef: u8 = 1;

    for n in 0..nsym {
        let mut delta = syndromes[n];
        for i in 1..=l {
            delta ^= gf.mul(lambda[i], syndromes[n - i]);
        }

        if delta == 0 {
            m += 1;
        } else if 2 * l <= n {
            let prev_lambda = lambda.clone();
            let scale = gf.div(delta, b_coef);
            let upper = b.len().min(lambda.len().saturating_sub(m));
            for j in 0..upper {
                lambda[j + m] ^= gf.mul(scale, b[j]);
            }
            l = n + 1 - l;
            b = prev_lambda;
            b_coef = delta;
            m = 1;
        } else {
            let scale = gf.div(delta, b_coef);
            let upper = b.len().min(lambda.len().saturating_sub(m));
            for j in 0..upper {
                lambda[j + m] ^= gf.mul(scale, b[j]);
            }
            m += 1;
        }
    }

    lambda.truncate(l + 1);
    (lambda, l)
}


/// Перебор Ченя: полный перебор всех `n` позиций кодового слова в поисках
/// корней локатора Λ(x). Для позиции `idx` соответствующий "локатор
/// ошибки" `X_l = α^(n-1-idx)` (степень падает слева направо - позиция 0
/// это самый старший разряд кодового слова), а корень, который мы ищем, -
/// это `X_l^-1`. Возвращает по каждой найденной ошибке тройку
/// `(idx, X_l^-1, X_l)` - оба значения нужны дальше для формулы Форни.
fn find_errors(gf: &Gf256, lambda: &[u8], n: usize) -> Vec<(usize, u8, u8)> {
    let mut result = Vec::new();
    for idx in 0..n {
        let exponent = (n - 1 - idx) as i32;
        let root = gf.pow(2, -exponent);
        if poly_eval_ascending(gf, lambda, root) == 0 {
            let x_l = gf.pow(2, exponent);
            result.push((idx, root, x_l));
        }
    }
    result
}


/// Результат декодирования одного блока.
enum BlockStatus {
    /// Ошибок не было (все синдромы нулевые).
    Ok,
    /// Ошибки были и исправлены; после исправления синдромы повторно
    /// проверены и обнулились.
    Corrected,
    /// Либо число ошибок заведомо превышает гарантированную границу
    /// (`2*L > nsym`), либо перебор Ченя не подтвердил заявленную степень
    /// локатора, либо (защита от тихого искажения данных) синдромы после
    /// "исправления" не обнулились - код Рида-Соломона, как и любой блочный
    /// код, в принципе не может НАДЁЖНО обнаруживать ошибки сверх своей
    /// собственной корректирующей способности (он может ошибочно
    /// "исправить" блок в другое валидное на вид кодовое слово); повторная
    /// проверка синдромов - лучшая практическая защита от этого, но не
    /// абсолютная гарантия.
    Uncorrectable,
}


/// Пытается найти и исправить ошибки в одном блоке (`codeword`, длина
/// `k + nsym`) по алгоритму Берлекэмпа-Мэсси (локализация) и Форни
/// (амплитуды). Исправляет `codeword` на месте.
fn decode_block(gf: &Gf256, codeword: &mut [u8], nsym: usize) -> BlockStatus {
    let syndromes: Vec<u8> = (0..nsym)
        .map(|i| poly_eval_descending(gf, codeword, gf.pow(2, i as i32)))
        .collect();

    if syndromes.iter().all(|&s| s == 0) {
        // Нулевой синдром подтверждает принадлежность принятого слова коду,
        // но принципиально не отличает исходное слово от другого допустимого
        // кодового слова. По принятому решению отдельная CRC здесь не хранится.
        return BlockStatus::Ok;
    }

    let (lambda, errors_count) = berlekamp_massey(gf, &syndromes);
    if errors_count == 0 || 2 * errors_count > nsym {
        return BlockStatus::Uncorrectable;
    }

    let errors = find_errors(gf, &lambda, codeword.len());
    if errors.len() != errors_count {
        // Перебор Ченя нашёл не столько корней, сколько подразумевает
        // степень локатора - реальных ошибок больше, чем код способен
        // достоверно описать.
        return BlockStatus::Uncorrectable;
    }

    // Ω(x) = (S(x) * Λ(x)) mod x^nsym - многочлен-оценщик ошибок.
    let omega: Vec<u8> = poly_mul(gf, &syndromes, &lambda)
        .into_iter()
        .take(nsym)
        .collect();
    let lambda_deriv = poly_formal_derivative(&lambda);

    for (idx, root, x_l) in &errors {
        let omega_val = poly_eval_ascending(gf, &omega, *root);
        let deriv_val = poly_eval_ascending(gf, &lambda_deriv, *root);
        if deriv_val == 0 {
            return BlockStatus::Uncorrectable;
        }
        // Формула Форни: Y_l = X_l * Ω(X_l^-1) / Λ'(X_l^-1).
        let magnitude = gf.mul(*x_l, gf.div(omega_val, deriv_val));
        codeword[*idx] ^= magnitude;
    }

    // Защита от тихого искажения данных (см. `BlockStatus::Uncorrectable`) -
    // повторно проверяем синдромы уже исправленного слова.
    let check_ok = (0..nsym)
        .all(|i| poly_eval_descending(gf, codeword, gf.pow(2, i as i32)) == 0);

    if check_ok {
        BlockStatus::Corrected
    } else {
        BlockStatus::Uncorrectable
    }
}


/// Читает из артефакта данные, ПОЛНОСТЬЮ заполняя переданный буфер
/// (переиспользуемый вызывающим кодом на каждой итерации - здесь нет
/// аллокации per-block).
fn fill_exact(artifact: &mut Artifact, buf: &mut [u8]) -> Result<(), AppError> {
    let mut filled = 0;
    while filled < buf.len() {
        let read = artifact.read_chunk_to(&mut buf[filled..])?;
        if read == 0 {
            return Err(AppError::Fec(
                "Reed-Solomon: неожиданный конец потока при чтении блока".to_string(),
            ));
        }
        filled += read;
    }
    Ok(())
}


pub struct ReedSolomonCode;

impl ErrorCorrectionCode for ReedSolomonCode {
    fn encode(&self, mut artifact: Artifact) -> Result<Artifact, AppError> {
        let gf = Gf256::new();
        let genz = build_genzerator(&gf, PARITY_SYMBOLS);

        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "fec_encoded");

        // Буфер ровно на один блок данных - выделяется один раз и
        // переиспользуется на каждой итерации, память не зависит от
        // размера файла.
        let mut data_buf = [0u8; DATA_SYMBOLS];
        let mut filled = 0usize;
        let mut out_buf: Vec<u8> = Vec::with_capacity(OUTPUT_FLUSH_SIZE);

        while let Some(chunk) = artifact.next_chunk()? {
            let mut chunk_pos = 0;
            while chunk_pos < chunk.len() {
                let take = (DATA_SYMBOLS - filled).min(chunk.len() - chunk_pos);
                data_buf[filled..filled + take].copy_from_slice(&chunk[chunk_pos..chunk_pos + take]);
                filled += take;
                chunk_pos += take;

                if filled == DATA_SYMBOLS {
                    let codeword = rs_encode_block(&gf, &data_buf, &genz);
                    out_buf.extend_from_slice(&codeword);
                    filled = 0;

                    if out_buf.len() >= OUTPUT_FLUSH_SIZE {
                        output.write_chunk_from(&out_buf)?;
                        out_buf.clear();
                    }
                }
            }
        }

        if filled > 0 {
            // Укороченный RS-блок сохраняет ровно оставшиеся данные и
            // стандартное число символов чётности, без нулевого дополнения.
            let codeword = rs_encode_block(&gf, &data_buf[..filled], &genz);
            out_buf.extend_from_slice(&codeword);
        }

        if !out_buf.is_empty() {
            output.write_chunk_from(&out_buf)?;
        }

        Ok(output)
    }

    fn decode(&self, mut artifact: Artifact) -> Result<(Artifact, FecReport), AppError> {
        let encoded_size = artifact.get_payload_size();
        let full_blocks = encoded_size / CODEWORD_SYMBOLS;
        let shortened_size = encoded_size % CODEWORD_SYMBOLS;

        if shortened_size != 0 && shortened_size <= PARITY_SYMBOLS {
            return Err(AppError::Fec(format!(
                "Reed-Solomon: некорректная длина потока {encoded_size}: остаток блока {shortened_size} должен быть 0 или {}..={} байт",
                PARITY_SYMBOLS + 1,
                CODEWORD_SYMBOLS - 1,
            )));
        }

        let blocks_count = full_blocks + usize::from(shortened_size != 0);

        let gf = Gf256::new();

        let mut output = Artifact::new_with_temp_file_suffix(&artifact, "fec_decoded");
        let mut report = FecReport::default();

        let mut out_buf: Vec<u8> = Vec::with_capacity(OUTPUT_FLUSH_SIZE);
        let mut block_buf = vec![0u8; CODEWORD_SYMBOLS];

        for block_index in 0..blocks_count {
            let block_size = if block_index < full_blocks {
                CODEWORD_SYMBOLS
            } else {
                shortened_size
            };
            let data_size = block_size - PARITY_SYMBOLS;
            block_buf.resize(block_size, 0);
            fill_exact(&mut artifact, &mut block_buf)?;

            let status = decode_block(&gf, &mut block_buf, PARITY_SYMBOLS);
            report.blocks_processed += 1;
            match status {
                BlockStatus::Ok => {}
                BlockStatus::Corrected => report.blocks_corrected += 1,
                BlockStatus::Uncorrectable => {
                    return Err(AppError::Fec(format!(
                        "Reed-Solomon: блок {} из {} содержит неисправимые ошибки",
                        block_index + 1,
                        blocks_count,
                    )));
                }
            }

            out_buf.extend_from_slice(&block_buf[..data_size]);

            if out_buf.len() >= OUTPUT_FLUSH_SIZE {
                output.write_chunk_from(&out_buf)?;
                out_buf.clear();
            }
        }

        if !out_buf.is_empty() {
            output.write_chunk_from(&out_buf)?;
        }

        Ok((output, report))
    }

    fn id(&self) -> FecId {
        FecId::ReedSolomon
    }
}


#[cfg(test)]
mod tests {
    use std::fs;
    use std::num::NonZeroUsize;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn source_artifact(data: &[u8]) -> (Artifact, PathBuf) {
        let id = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rustarch_rs_{}_{}.bin",
            std::process::id(),
            id,
        ));
        fs::write(&path, data).unwrap();
        let mut artifact = Artifact::from_file(&path).unwrap();
        artifact.chunk_size = NonZeroUsize::new(17).unwrap();
        (artifact, path)
    }

    fn read_all(mut artifact: Artifact) -> Vec<u8> {
        let mut result = Vec::new();
        let mut buffer = [0u8; 19];
        loop {
            let read = artifact.read_chunk_to(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            result.extend_from_slice(&buffer[..read]);
        }
        result
    }

    fn test_data(size: usize) -> Vec<u8> {
        (0..size)
            .map(|index| ((index * 73 + index / 7 + 19) % 256) as u8)
            .collect()
    }

    #[test]
    fn artifact_round_trip_uses_full_and_shortened_blocks() {
        let cases = [
            (0, 0),
            (1, 33),
            (222, 254),
            (223, 255),
            (224, 288),
            (445, 509),
            (446, 510),
        ];

        for (source_size, encoded_size) in cases {
            let source = test_data(source_size);
            let (artifact, path) = source_artifact(&source);
            let encoded = ReedSolomonCode.encode(artifact).unwrap();
            fs::remove_file(path).unwrap();

            assert_eq!(encoded.get_payload_size(), encoded_size);
            let (decoded, report) = ReedSolomonCode.decode(encoded).unwrap();
            assert_eq!(read_all(decoded), source);
            assert_eq!(report.blocks_corrected, 0);
            assert_eq!(report.blocks_uncorrectable, 0);
        }
    }

    #[test]
    fn corrects_up_to_sixteen_symbol_errors() {
        for source_size in [1, DATA_SYMBOLS - 1, DATA_SYMBOLS] {
            for errors_count in 1..=16 {
                let source = test_data(source_size);
                let (artifact, path) = source_artifact(&source);
                let mut encoded = ReedSolomonCode.encode(artifact).unwrap();
                fs::remove_file(path).unwrap();

                let bytes = encoded.next_mut_chunk_from_memory().unwrap().unwrap();
                for (index, byte) in bytes.iter_mut().take(errors_count).enumerate() {
                    *byte ^= (index as u8).wrapping_add(1);
                }
                encoded.rewind_reading();

                let (decoded, report) = ReedSolomonCode.decode(encoded).unwrap();
                assert_eq!(read_all(decoded), source);
                assert_eq!(report.blocks_processed, 1);
                assert_eq!(report.blocks_corrected, 1);
                assert_eq!(report.blocks_uncorrectable, 0);
            }
        }
    }

    #[test]
    fn rejects_uncorrectable_block() {
        let source = test_data(DATA_SYMBOLS);
        let (artifact, path) = source_artifact(&source);
        let mut encoded = ReedSolomonCode.encode(artifact).unwrap();
        fs::remove_file(path).unwrap();

        let bytes = encoded.next_mut_chunk_from_memory().unwrap().unwrap();
        for (index, byte) in bytes.iter_mut().take(17).enumerate() {
            *byte ^= (index as u8).wrapping_add(1);
        }
        encoded.rewind_reading();

        assert!(matches!(
            ReedSolomonCode.decode(encoded),
            Err(AppError::Fec(_)),
        ));
    }

    #[test]
    fn rejects_stream_too_short_for_parity() {
        for size in 1..=PARITY_SYMBOLS {
            let bytes = vec![0u8; size];
            let (artifact, path) = source_artifact(&bytes);
            let result = ReedSolomonCode.decode(artifact);
            fs::remove_file(path).unwrap();
            assert!(matches!(result, Err(AppError::Fec(_))), "size={size}");
        }
    }

    #[test]
    fn rejects_truncated_codeword() {
        let source = test_data(DATA_SYMBOLS);
        let (artifact, path) = source_artifact(&source);
        let encoded = ReedSolomonCode.encode(artifact).unwrap();
        fs::remove_file(path).unwrap();

        let mut truncated = read_all(encoded);
        truncated.pop();
        let (artifact, path) = source_artifact(&truncated);
        let result = ReedSolomonCode.decode(artifact);
        fs::remove_file(path).unwrap();

        assert!(matches!(result, Err(AppError::Fec(_))));
    }
}
