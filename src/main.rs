mod cli;
mod error;
mod algorithms;
mod archiver;

use crate::cli::{CLICommand, run_pack, run_unpack};
use crate::algorithms::{CipherId, CompressionId, FecId, PipelineSettings};


fn main() {
    /*let args: Vec<String> = std::env::args().skip(1).collect();

    let cli_command = match parse_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Ошибка: {e}");
            print_help();
            std::process::exit(1);
        }
    };
     */

    // ========== Для теста: ==========
    let cli_command = CLICommand::Pack {
        source_path: r"C:/Games/Battlefield 2142 Novgames RST".into(), // Важно, что эти относительные пути именно строки,
        target_archive_path: r"./test_data_for_removing/test_data.arch".into(), // которые можно менять
        settings: PipelineSettings {
            compression: CompressionId::NoCompression,
            cipher: CipherId::Xor,
            fec: FecId::NoFec,
        },
        encode_key: vec![1_u8, 2, 3, 4],
    };

    /*
    let cli_command = CLICommand::Unpack {
        source_path: r"tests/test_data.arch".into(),
        target_unpack_path: "tests/kal".into(),
        decode_key: vec![5, 5, 2, 3, 1, 43],
    };
     */


    let start = std::time::Instant::now();
    let archive_result = match cli_command {
        CLICommand::Pack {
            source_path,
            target_archive_path,
            settings,
            encode_key,
        } => run_pack(&source_path, &target_archive_path, settings, encode_key),

        CLICommand::Unpack {
            source_path,
            target_unpack_path,
            decode_key,
        } => run_unpack(&source_path, &target_unpack_path, &decode_key),

        _ => todo!(),
    };
    let elapsed = start.elapsed();
    println!("\n===> TOTAL TIME: {}ms", elapsed.as_millis());

    if let Err(e) = archive_result {
        eprintln!("Ошибка: {e}");
        std::process::exit(1);
    }
}
