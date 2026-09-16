pub mod cli;
pub mod error;
pub mod archiver;
pub mod algorithms;
//mod gui;

use crate::cli::{ CLICommand, run_pack, run_unpack };
use crate::algorithms::{ CipherId, CompressionId, FecId, PipelineSettings };


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

    enum TestCLICommand { Pack, Unpack, }
    let test_choice = TestCLICommand::Pack; // !МЕНЯТЬ ЗДЕСЬ!

    let cli_command: CLICommand = match test_choice {
        TestCLICommand::Pack => CLICommand::Pack {
            source_path: r"C:\Games\Battlefield 2142 Novgames RST".into(), // Важно, что эти относительные пути именно строки,
            target_archive_path: r".\test_data_for_removing\study.arch".into(), // которые можно менять
            settings: PipelineSettings {
                compression: CompressionId::Huffman,
                cipher: CipherId::NoCipher,
                fec: FecId::NoFec,
            },
            encode_key: (1..=255).collect(), // Подбирать 255-БАЙТНЫЙ ключ полным перебором - это увлекательное дело!
        },

        TestCLICommand::Unpack => CLICommand::Unpack {
            source_path: r"test_data_for_removing/study.arch".into(),
            target_unpack_path: "test_data_for_removing/study".into(),
            decode_key: (1..=255).collect(),
        },

    };

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

    use std::os::windows::fs::MetadataExt;
    let archive_size_in_gb: f64 = std::fs::metadata(r"C:\Users\alex\RustroverProjects\RustArch\test_data_for_removing\study.arch")
        .unwrap().file_size() as f64 / 1024_f64.powi(3);

    println!("\n===> TOTAL TIME: {}ms", elapsed.as_millis());
    println!(  "===> TOTAL SIZE: {:.2}GB", archive_size_in_gb);

    if let Err(e) = archive_result {
        eprintln!("Ошибка: {e}");
        std::process::exit(1);
    }

    //let _ = gui::run();
}
