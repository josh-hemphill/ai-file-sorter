//! Isolated vision worker. Reads EXIF and returns evidence; never writes files.

use aifs_protocol::worker::WorkerKind;

fn main() {
    if let Err(error) =
        aifs_worker_runtime::run_stdio(WorkerKind::Vision, &["exif"], |root, entry| {
            Ok(aifs_extractors::extract_image_entry(root, entry))
        })
    {
        eprintln!("aifs-worker-vision: {error}");
        std::process::exit(1);
    }
}
