//! Isolated document worker. Reads PDF/Office/text and returns evidence; never writes files.

use aifs_protocol::worker::WorkerKind;

fn main() {
    if let Err(error) =
        aifs_worker_runtime::run_stdio(WorkerKind::Document, &["document_text"], |root, entry| {
            Ok(aifs_extractors::extract_document_entry(root, entry))
        })
    {
        eprintln!("aifs-worker-document: {error}");
        std::process::exit(1);
    }
}
