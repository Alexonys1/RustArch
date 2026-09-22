use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::Path;
use std::thread;

use crate::error::AppError;
use crate::archiver::ArchivedArtifactEntry;
use super::one_file::unpack_file;


const MAX_PARALLELISM: usize = usize::MAX;


pub fn unpack_entries_parallel(
    entries: Vec<ArchivedArtifactEntry>,
    archive_path: &Path,
    output_dir: &Path,
    decode_key: &[u8],
) -> Result<(), AppError>
{
    let groups_of_entries = group_entries_for_workers(&entries);

    thread::scope(|scope| {
        let mut workers = Vec::with_capacity(groups_of_entries.len());

        for entry_group in &groups_of_entries {
            workers.push(
                scope.spawn(|| {
                    unpack_entry_group(entry_group, archive_path, output_dir, decode_key)
                }),
            );
        }

        for worker in workers {
            worker
                .join()
                .map_err(|_| AppError::Compression("Паника в рабочем потоке".into()))??;
        }

        Ok(())
    })
}


fn unpack_entry_group(
    entry_group: &[&ArchivedArtifactEntry],
    archive_path: &Path,
    output_dir: &Path,
    decode_key: &[u8],
) -> Result<(), AppError> {
    for &entry in entry_group {
        unpack_file(entry, archive_path, output_dir, decode_key)?;
    }
    Ok(())
}


fn group_entries_for_workers(entries: &[ArchivedArtifactEntry]) -> Vec<Vec<&ArchivedArtifactEntry>> {
    if entries.is_empty() {
        return vec![];
    }

    let number_of_workers = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(entries.len())
        .min(MAX_PARALLELISM);

    let mut sorted: Vec<&ArchivedArtifactEntry> = entries.iter().collect();
    sorted.sort_unstable_by_key(|e| Reverse(entry_weight(e)));

    let mut groups: Vec<Vec<&ArchivedArtifactEntry>> =
        (0..number_of_workers).map(|_| Vec::new()).collect();
    let mut loads: BinaryHeap<Reverse<(u64, usize)>> =
        (0..number_of_workers).map(|i| Reverse((0, i))).collect();

    for entry in sorted {
        let Reverse((load, worker_idx)) = loads.pop().unwrap();
        groups[worker_idx].push(entry);
        loads.push(Reverse((load + entry_weight(entry), worker_idx)));
    }

    groups
}


fn entry_weight(entry: &ArchivedArtifactEntry) -> u64 {
    entry.stored_size.max(1)
}
