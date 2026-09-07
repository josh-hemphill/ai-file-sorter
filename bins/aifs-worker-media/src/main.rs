//! Isolated media-tag worker. Reads tags and returns evidence; never writes files.

use aifs_protocol::worker::WorkerKind;

fn main() {
    if let Err(error) =
        aifs_worker_runtime::run_stdio(WorkerKind::Media, &["media_tags"], |root, entry| {
            Ok(aifs_extractors::extract_entry(root, entry))
        })
    {
        eprintln!("aifs-worker-media: {error}");
        std::process::exit(1);
    }
}
