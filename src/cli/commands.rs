use crate::algorithms::PipelineSettings;


pub enum CLICommand {
    Pack {
        source_path: String,
        target_archive_path: String,
        settings: PipelineSettings,
        encode_key: Vec<u8>,
    },

    Unpack {
        source_path: String,
        target_unpack_path: String,
        decode_key: Vec<u8>,
    },
    
    ShowArchiveInnerStructure {
        archive_path: String,
    },
    
    Help,
}
