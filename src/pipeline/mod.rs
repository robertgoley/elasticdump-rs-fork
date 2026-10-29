//! Generic source → sink plumbing shared by every transfer direction.

pub mod file_source;
pub mod json_split;
pub mod sink;
pub mod window;

use std::path::PathBuf;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub use sink::{FileSink, Sink};
pub use window::DocWindow;

/// Copy documents from a file (or stdin) into `sink`, honouring
/// `--offset`/`--size`. Returns the number of documents written.
pub async fn file_to_sink(
    input: Option<PathBuf>,
    limit: usize,
    mut window: DocWindow,
    mut sink: Sink,
    cancel: CancellationToken,
) -> Result<u64> {
    let mut rx = file_source::spawn_file_source(input, limit, 4, cancel.clone());
    let mut written = 0u64;
    let result: Result<()> = async {
        if window.is_exhausted() {
            return Ok(());
        }
        while let Some(batch) = rx.recv().await {
            let batch = batch?;
            let (slice, docs) = window.apply(&batch.buf, batch.docs);
            if docs > 0 {
                if let Err(e) = sink.write_all(slice).await {
                    if sink.is_stdout() && crate::output::is_broken_pipe(&e) {
                        log::info!("stdout closed by reader; stopping early");
                        return Ok(());
                    }
                    return Err(e);
                }
                written += docs;
            }
            if window.is_exhausted() {
                break;
            }
        }
        Ok(())
    }
    .await;
    cancel.cancel(); // stop the reader thread if we ended early
    match result {
        Ok(()) => {
            if let Err(e) = sink.flush().await {
                if !(sink.is_stdout() && crate::output::is_broken_pipe(&e)) {
                    sink.abort().await;
                    return Err(e);
                }
            }
            sink.finalize().await?;
            Ok(written)
        }
        Err(e) => {
            sink.abort().await;
            Err(e)
        }
    }
}
