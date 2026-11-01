// SUP-006: no unsafe Rust in this crate; all unsafe comes from audited deps.
#![forbid(unsafe_code)]

/// Stack for the thread driving the runtime and for its workers (TST-BRK-IN-008): the JSON
/// parsers recurse per nesting level, and a deeply nested document (up to the 1000 levels
/// Elasticsearch accepts) overflowed the default 8 MiB / 2 MiB stacks. Stacks are reserved,
/// not committed, so this does not raise resident memory.
// Optional global allocators (L-802a); the default build uses the platform allocator.
#[cfg(all(feature = "jemalloc", feature = "mimalloc"))]
compile_error!("features `jemalloc` and `mimalloc` are mutually exclusive");
#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
#[cfg(all(feature = "mimalloc", not(feature = "jemalloc")))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const MAIN_STACK: usize = 256 << 20;
const WORKER_STACK: usize = 64 << 20;

fn main() {
    install_panic_hook();
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

/// The default panic report, with every registered secret and any
/// `user:password@` from the command line masked (D-4: passwords never
/// appear in panics either).
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let msg = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "Box<dyn Any>".into());
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        let at = info
            .location()
            .map(|l| format!(" at {l}"))
            .unwrap_or_default();
        let bt = std::backtrace::Backtrace::capture();
        let bt = match bt.status() {
            std::backtrace::BacktraceStatus::Captured => format!("stack backtrace:\n{bt}"),
            _ => "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace"
                .into(),
        };
        let args: Vec<String> = std::env::args().collect();
        eprintln!(
            "{}",
            panic_report(
                &format!("\nthread '{name}' panicked{at}:\n{msg}\n{bt}"),
                &args
            )
        );
    }));
}

/// A panic report with the registered secrets and the command line's
/// `user:password@` masked.
fn panic_report(report: &str, args: &[String]) -> String {
    let report = elasticdump_rs::endpoint::scrub_args(report, args);
    elasticdump_rs::secrets::scrub(&report)
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

#[cfg(test)]
mod tests {
    /// D-4: a panic message that carries a password prints it masked.
    #[test]
    fn panic_reports_are_scrubbed() {
        elasticdump_rs::secrets::register("FAKE-panic-pw-1");
        let args = vec![
            "elasticdump-rs".to_string(),
            "--input=http://u:FAKE-panic-url-2@h:9200/i".to_string(),
        ];
        let r = super::panic_report(
            "thread 'main' panicked at x.rs:1:1:\nboom FAKE-panic-pw-1 http://u:FAKE-panic-url-2@h:9200/i",
            &args,
        );
        assert!(!r.contains("FAKE"), "{r}");
        assert!(r.contains("boom ***") && r.contains("u:***@h:9200"), "{r}");
    }
}
