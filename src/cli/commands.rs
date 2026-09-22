use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::PathBuf;

use crate::algorithms::{CipherId, CompressionId, FecId, PipelineSettings};
use crate::error::AppError;


const MAX_KEY_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub enum CLICommand {
    Pack {
        source_path: PathBuf,
        target_archive_path: PathBuf,
        settings: PipelineSettings,
        encode_key: Vec<u8>,
    },
    Unpack {
        source_path: PathBuf,
        target_unpack_path: PathBuf,
        decode_key: Vec<u8>,
    },
    ShowArchiveInnerStructure {
        archive_path: PathBuf,
    },
    Help,
    Version,
}

pub const HELP: &str = r#"RustArch - многопоточный архиватор

ИСПОЛЬЗОВАНИЕ:
    RustArch pack <SOURCE> <ARCHIVE> [OPTIONS]
    RustArch unpack <ARCHIVE> <DESTINATION> [OPTIONS]
    RustArch list <ARCHIVE>
    RustArch help

КОМАНДА pack:
    -c, --compression <ALGORITHM>   none | rle | huffman | lzss | deflate
                                    По умолчанию: deflate
        --cipher <ALGORITHM>        none | xor. По умолчанию: none
        --fec <ALGORITHM>           none | reed-solomon. По умолчанию: none
        --key <TEXT>                XOR-ключ как UTF-8 строка
        --key-hex <HEX>             XOR-ключ в hex, например 01:02:ff
        --key-file <PATH>           Прочитать бинарный XOR-ключ из файла

    Для постоянных ключей предпочтительнее --key-file: значения --key и
    --key-hex могут сохраняться в истории команд и быть видны другим процессам.

КОМАНДА unpack:
        --key <TEXT>                Ключ для зашифрованного архива
        --key-hex <HEX>             Ключ в hex
        --key-file <PATH>           Прочитать ключ из файла

ОБЩИЕ ОПЦИИ:
    -h, --help                      Показать эту справку
    -V, --version                   Показать версию
    --                              Конец опций; нужен для путей, начинающихся с '-'

ПРИМЕРЫ:
    RustArch pack ./data backup.rarc -c lzss
    RustArch pack ./data backup.rarc -c deflate --cipher xor --key-hex deadbeef
    RustArch unpack backup.rarc ./restored --key-hex deadbeef
    RustArch list backup.rarc
"#;


pub fn print_help() {
    print!("{HELP}");
}

pub fn parse_args<I, T>(args: I) -> Result<CLICommand, AppError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    if args.is_empty() {
        return Ok(CLICommand::Help);
    }

    let command = args.remove(0);
    match command.to_str() {
        Some("pack") | Some("p") => parse_pack(args),
        Some("unpack") | Some("u") => parse_unpack(args),
        Some("list") | Some("ls") | Some("l") => parse_list(args),
        Some("help") | Some("-h") | Some("--help") => Ok(CLICommand::Help),
        Some("version") | Some("-V") | Some("--version") => Ok(CLICommand::Version),
        Some(other) => Err(usage_error(format!("неизвестная команда '{other}'"))),
        None => Err(usage_error(
            "имя команды не является валидной UTF-8 строкой",
        )),
    }
}

fn parse_pack(args: Vec<OsString>) -> Result<CLICommand, AppError> {
    let mut positionals = Vec::new();
    let mut compression = None;
    let mut cipher = None;
    let mut fec = None;
    let mut key = None;
    let mut options_enabled = true;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if options_enabled && arg.as_os_str() == OsStr::new("--") {
            options_enabled = false;
            index += 1;
            continue;
        }

        if options_enabled {
            if let Some((name, inline_value)) = split_option(arg)? {
                if name == "-h" || name == "--help" {
                    return Ok(CLICommand::Help);
                }
                if name == "-V" || name == "--version" {
                    return Ok(CLICommand::Version);
                }
                if !matches!(
                    name,
                    "-c" | "--compression"
                        | "--cipher"
                        | "--fec"
                        | "--key"
                        | "--key-hex"
                        | "--key-file"
                ) {
                    return Err(usage_error(format!("неизвестная опция pack: {name}")));
                }

                let value = option_value(&args, &mut index, name, inline_value)?;
                match name {
                    "-c" | "--compression" => {
                        set_once(&mut compression, parse_compression(value)?, "compression")?
                    }
                    "--cipher" => set_once(&mut cipher, parse_cipher(value)?, "cipher")?,
                    "--fec" => set_once(&mut fec, parse_fec(value)?, "fec")?,
                    "--key" => set_once(&mut key, KeySource::Text(value.to_os_string()), "key")?,
                    "--key-hex" => {
                        let text = value.to_str().ok_or_else(|| {
                            usage_error("значение --key-hex не является UTF-8 строкой")
                        })?;
                        set_once(&mut key, KeySource::Bytes(parse_hex_key(text)?), "key")?;
                    }
                    "--key-file" => {
                        set_once(&mut key, KeySource::File(PathBuf::from(value)), "key")?
                    }
                    _ => unreachable!(),
                }
                index += 1;
                continue;
            }
        }

        positionals.push(PathBuf::from(arg.as_os_str()));
        index += 1;
    }

    if positionals.len() != 2 {
        return Err(usage_error("pack ожидает два пути: <SOURCE> <ARCHIVE>"));
    }

    let cipher = cipher.unwrap_or(CipherId::NoCipher);
    let encode_key = load_key(key)?;
    validate_pack_key(cipher, &encode_key)?;

    Ok(CLICommand::Pack {
        source_path: positionals.remove(0),
        target_archive_path: positionals.remove(0),
        settings: PipelineSettings {
            compression: compression.unwrap_or(CompressionId::Deflate),
            cipher,
            fec: fec.unwrap_or(FecId::NoFec),
        },
        encode_key,
    })
}

fn parse_unpack(args: Vec<OsString>) -> Result<CLICommand, AppError> {
    let mut positionals = Vec::new();
    let mut key = None;
    let mut options_enabled = true;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if options_enabled && arg.as_os_str() == OsStr::new("--") {
            options_enabled = false;
            index += 1;
            continue;
        }

        if options_enabled {
            if let Some((name, inline_value)) = split_option(arg)? {
                if name == "-h" || name == "--help" {
                    return Ok(CLICommand::Help);
                }
                if name == "-V" || name == "--version" {
                    return Ok(CLICommand::Version);
                }
                if !matches!(name, "--key" | "--key-hex" | "--key-file") {
                    return Err(usage_error(format!("неизвестная опция unpack: {name}")));
                }

                let value = option_value(&args, &mut index, name, inline_value)?;
                match name {
                    "--key" => set_once(&mut key, KeySource::Text(value.to_os_string()), "key")?,
                    "--key-hex" => {
                        let text = value.to_str().ok_or_else(|| {
                            usage_error("значение --key-hex не является UTF-8 строкой")
                        })?;
                        set_once(&mut key, KeySource::Bytes(parse_hex_key(text)?), "key")?;
                    }
                    "--key-file" => {
                        set_once(&mut key, KeySource::File(PathBuf::from(value)), "key")?
                    }
                    _ => unreachable!(),
                }
                index += 1;
                continue;
            }
        }

        positionals.push(PathBuf::from(arg.as_os_str()));
        index += 1;
    }

    if positionals.len() != 2 {
        return Err(usage_error(
            "unpack ожидает два пути: <ARCHIVE> <DESTINATION>",
        ));
    }

    Ok(CLICommand::Unpack {
        source_path: positionals.remove(0),
        target_unpack_path: positionals.remove(0),
        decode_key: load_key(key)?,
    })
}

fn parse_list(args: Vec<OsString>) -> Result<CLICommand, AppError> {
    if args.len() == 1
        && (args[0].as_os_str() == OsStr::new("-h") || args[0].as_os_str() == OsStr::new("--help"))
    {
        return Ok(CLICommand::Help);
    }
    if args.len() == 1
        && (args[0].as_os_str() == OsStr::new("-V")
            || args[0].as_os_str() == OsStr::new("--version"))
    {
        return Ok(CLICommand::Version);
    }

    let archive_path = match args.as_slice() {
        [path] if path.to_str().is_some_and(|text| text.starts_with('-')) => {
            return Err(usage_error(format!(
                "неизвестная опция list: {}",
                path.to_string_lossy()
            )));
        }
        [path] => path.clone(),
        [separator, path] if separator.as_os_str() == OsStr::new("--") => path.clone(),
        _ => {
            return Err(usage_error("list ожидает один путь: <ARCHIVE>"));
        }
    };
    if archive_path.as_os_str().is_empty() {
        return Err(usage_error("list ожидает один путь: <ARCHIVE>"));
    }
    Ok(CLICommand::ShowArchiveInnerStructure {
        archive_path: PathBuf::from(archive_path),
    })
}

fn split_option(arg: &OsStr) -> Result<Option<(&str, Option<&OsStr>)>, AppError> {
    let Some(text) = arg.to_str() else {
        if arg.as_encoded_bytes().first() == Some(&b'-') {
            return Err(usage_error("имя опции не является валидной UTF-8 строкой"));
        }
        return Ok(None);
    };

    if !text.starts_with('-') || text == "-" {
        return Ok(None);
    }

    if let Some((name, value)) = text.split_once('=') {
        Ok(Some((name, Some(OsStr::new(value)))))
    } else {
        Ok(Some((text, None)))
    }
}

fn option_value<'a>(
    args: &'a [OsString],
    index: &mut usize,
    name: &str,
    inline: Option<&'a OsStr>,
) -> Result<&'a OsStr, AppError> {
    if let Some(value) = inline {
        if value.is_empty() {
            return Err(usage_error(format!(
                "опция {name} получила пустое значение"
            )));
        }
        return Ok(value);
    }

    *index += 1;
    args.get(*index)
        .map(OsString::as_os_str)
        .ok_or_else(|| usage_error(format!("после {name} ожидается значение")))
}

fn set_once<T>(slot: &mut Option<T>, value: T, name: &str) -> Result<(), AppError> {
    if slot.replace(value).is_some() {
        return Err(usage_error(format!("опция {name} указана несколько раз")));
    }
    Ok(())
}

fn parse_compression(value: &OsStr) -> Result<CompressionId, AppError> {
    match normalized(value, "compression")?.as_str() {
        "none" | "store" => Ok(CompressionId::NoCompression),
        "rle" => Ok(CompressionId::RLE),
        "huffman" | "huff" => Ok(CompressionId::Huffman),
        "lzss" => Ok(CompressionId::LZSS),
        "deflate" => Ok(CompressionId::Deflate),
        other => Err(usage_error(format!(
            "неизвестный алгоритм сжатия '{other}'"
        ))),
    }
}

fn parse_cipher(value: &OsStr) -> Result<CipherId, AppError> {
    match normalized(value, "cipher")?.as_str() {
        "none" => Ok(CipherId::NoCipher),
        "xor" => Ok(CipherId::Xor),
        other => Err(usage_error(format!("неизвестный шифр '{other}'"))),
    }
}

fn parse_fec(value: &OsStr) -> Result<FecId, AppError> {
    match normalized(value, "fec")?.as_str() {
        "none" => Ok(FecId::NoFec),
        "reed-solomon" | "reed_solomon" | "rs" => Ok(FecId::ReedSolomon),
        "hamming7-4" | "hamming15-11" => Err(usage_error(
            "Hamming пока не подключён к pipeline; используйте none или reed-solomon",
        )),
        other => Err(usage_error(format!("неизвестный FEC '{other}'"))),
    }
}

fn normalized(value: &OsStr, option: &str) -> Result<String, AppError> {
    value
        .to_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| usage_error(format!("значение --{option} не является UTF-8 строкой")))
}

enum KeySource {
    Text(OsString),
    Bytes(Vec<u8>),
    File(PathBuf),
}

fn load_key(source: Option<KeySource>) -> Result<Vec<u8>, AppError> {
    let bytes = match source {
        None => Ok(Vec::new()),
        Some(KeySource::Text(text)) => text
            .into_string()
            .map(String::into_bytes)
            .map_err(|_| usage_error("значение --key не является UTF-8 строкой")),
        Some(KeySource::Bytes(bytes)) => Ok(bytes),
        Some(KeySource::File(path)) => fs::read(&path).map_err(|error| {
            AppError::CLIUsage(format!(
                "не удалось прочитать файл ключа '{}': {error}",
                path.display()
            ))
        }),
    }?;

    if bytes.len() > MAX_KEY_SIZE {
        return Err(usage_error(format!(
            "ключ слишком большой: {} байт; максимум {MAX_KEY_SIZE}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

fn parse_hex_key(text: &str) -> Result<Vec<u8>, AppError> {
    let digits: Vec<u8> = text
        .bytes()
        .filter(|byte| !matches!(byte, b':' | b'-' | b' ' | b'\t' | b'\r' | b'\n'))
        .collect();

    if digits.is_empty() || digits.len() % 2 != 0 {
        return Err(usage_error(
            "--key-hex должен содержать ненулевое чётное число hex-цифр",
        ));
    }

    digits
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(byte: u8) -> Result<u8, AppError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(usage_error(format!(
            "недопустимый символ в --key-hex: '{}'",
            char::from(byte)
        ))),
    }
}

fn validate_pack_key(cipher: CipherId, key: &[u8]) -> Result<(), AppError> {
    match (cipher, key.is_empty()) {
        (CipherId::Xor, true) => Err(usage_error(
            "для --cipher xor требуется --key, --key-hex или --key-file",
        )),
        (CipherId::NoCipher, false) => Err(usage_error(
            "ключ указан, но шифрование выключено; добавьте --cipher xor",
        )),
        _ => Ok(()),
    }
}

fn usage_error(message: impl Into<String>) -> AppError {
    AppError::CLIUsage(message.into())
}
