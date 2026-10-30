// SUP-006: no unsafe Rust in this crate; all unsafe comes from audited deps.
#![forbid(unsafe_code)]

/// Stack for the thread driving the runtime and for its workers (TST-BRK-IN-008): the JSON
/// parsers recurse per nesting level, and a deeply nested document (up to the 1000 levels
/// Elasticsearch accepts) overflowed the default 8 MiB / 2 MiB stacks. Stacks are reserved,
/// not committed, so this does not raise resident memory.
const MAIN_STACK: usize = 256 << 20;
const WORKER_STACK: usize = 64 << 20;

fn main() {
    let runner = std::thread::Builder::new()
        .name("main".into())
        .stack_size(MAIN_STACK)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(WORKER_STACK)
                .build()
                .expect("tokio runtime")
                .block_on(async_main())
        })
        .expect("spawn the main thread");
    if runner.join().is_err() {
        std::process::exit(101);
    }
}

async fn async_main() {
    if let Err(error) = elasticdump_rs::run().await {
        let args: Vec<String> = std::env::args().collect();
        let msg = elasticdump_rs::endpoint::scrub_args(&format!("{error:#}"), &args);
        let msg = elasticdump_rs::secrets::scrub(&msg);
        let msg = elasticdump_rs::emfile_hint(msg);
        eprintln!("Error: {msg}");
        std::process::exit(elasticdump_rs::exit_code(&error));
    }
}
