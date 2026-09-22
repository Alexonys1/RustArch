use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

///! Я понимаю, что тесты должны быть платформо-независимыми и всё такое. Но они существенно упрощают нашу жизнь для отладки.
// TODO: Поэтому, поменяй пути, если кто-то вообще читает этот код.
pub const TEST1: PackUnpackTest = PackUnpackTest::new(r".\test_data\test1", "test1.arch");
pub const TEST2: PackUnpackTest = PackUnpackTest::new(r".\test_data\test2", "docs2.arch");


pub struct PackUnpackTest {
    source: &'static str,
    archive_filename: &'static str,
}


impl PackUnpackTest {
    pub const fn new(source: &'static str, archive_filename: &'static str) -> PackUnpackTest {
        PackUnpackTest { source, archive_filename }
    }

    pub fn get_source_path(&self) -> PathBuf {
        PathBuf::from(self.source)
    }

    pub fn get_temp_dir_path(&self) -> PathBuf {
        get_temp_dir_path()
            .join(Path::new(self.source).file_name().unwrap())
    }

    pub fn get_unpack_path(&self) -> PathBuf {
        self.get_temp_dir_path()
            .join(Path::new("unpacked"))
    }

    pub fn get_target_archive_path(&self) -> PathBuf {
        self.get_temp_dir_path()
            .join(self.archive_filename)
    }
}


pub fn get_temp_dir_path() -> PathBuf {
    static TEMP_DIR_PATH: OnceLock<PathBuf> = OnceLock::new();

    TEMP_DIR_PATH.get_or_init(|| {
        let home = env::var("USERPROFILE").unwrap();
        PathBuf::from(home)
            .join("RustroverProjects")
            .join("rustarch")
            .join("test_data_for_removing")
    }).clone()
}
