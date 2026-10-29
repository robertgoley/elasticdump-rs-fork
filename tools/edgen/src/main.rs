//! edgen CLI. See lib.rs and README.md.
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use edgen::{Format, GenOpts, Ids, PROFILES};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "edgen",
    version,
    about = "Seeded, byte-reproducible test data for elasticdump-rs"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Fmt {
    Source,
    Elasticdump,
    Bulk,
}

#[derive(Clone, Copy, ValueEnum)]
enum IdMode {
    Det,
    Uuid,
    None,
}

#[derive(Clone, Copy, ValueEnum, PartialEq)]
enum Compress {
    None,
    Gzip,
    Zstd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate documents.
    Gen {
        #[arg(long, default_value = "logs")]
        profile: String,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long)]
        count: u64,
        #[arg(long, default_value_t = 0)]
        start: u64,
        #[arg(long, value_enum, default_value = "elasticdump")]
        format: Fmt,
        /// Target index (default edgen-<profile>).
        #[arg(long)]
        index: Option<String>,
        #[arg(long, value_enum, default_value = "det")]
        ids: IdMode,
        /// Percent of documents with a routing value.
        #[arg(long, default_value_t = 10)]
        routing_pct: u32,
        /// `wide`: number of fields (<= 10000).
        #[arg(long, default_value_t = 1000)]
        fields: usize,
        /// `huge`: size range in MiB.
        #[arg(long, default_value_t = 1)]
        huge_min_mb: usize,
        #[arg(long, default_value_t = 10)]
        huge_max_mb: usize,
        /// `edge`: include the file-only cases (deep 1000/10000, 10/90 MB docs).
        #[arg(long)]
        edge_file_only: bool,
        /// `edge`: include the cases that write to other indices.
        #[arg(long)]
        edge_index_cases: bool,
        #[arg(long, value_enum, default_value = "none")]
        compress: Compress,
        #[arg(long)]
        level: Option<i32>,
        /// Start a new output file after this many (uncompressed) bytes.
        #[arg(long)]
        split_bytes: Option<u64>,
        /// Output file (default stdout).
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        threads: Option<usize>,
    },
    /// Print `{"settings","mappings"}` for a profile.
    Mapping {
        #[arg(long, default_value = "logs")]
        profile: String,
        /// es | os (flat_object, OpenSearch >= 2.7) | os1
        #[arg(long, default_value = "es")]
        engine: String,
        #[arg(long, default_value_t = 1000)]
        fields: usize,
    },
    /// List the edge-case catalogue with expected outcomes.
    Cases,
    /// Convert an external corpus to elasticdump format with deterministic ids.
    Convert {
        /// rally | clf | loghub
        #[arg(long)]
        kind: String,
        /// Dataset name (feeds the id).
        #[arg(long)]
        dataset: String,
        /// File name used in the id (default: input file name).
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        index: String,
        #[arg(long)]
        max_docs: Option<u64>,
        /// Input file (.gz/.zst ok); `-` or none = stdin (pipe bz2 through bzcat).
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "none")]
        compress: Compress,
    },
}

struct Sink {
    base: Option<PathBuf>,
    compress: Compress,
    level: Option<i32>,
    split: Option<u64>,
    part: usize,
    written: u64,
    w: Option<Box<dyn Write>>,
    files: Vec<PathBuf>,
}

fn part_name(base: &Path, n: usize) -> PathBuf {
    let name = base.file_name().unwrap().to_string_lossy().to_string();
    let (stem, ext) = match name.find('.') {
        Some(p) => (name[..p].to_string(), name[p..].to_string()),
        None => (name.clone(), String::new()),
    };
    base.with_file_name(format!("{stem}-{n:05}{ext}"))
}

impl Sink {
    fn open(&mut self) -> Result<()> {
        let raw: Box<dyn Write> = match &self.base {
            None => Box::new(std::io::BufWriter::with_capacity(
                1 << 20,
                std::io::stdout().lock(),
            )),
            Some(b) => {
                let p = if self.split.is_some() {
                    part_name(b, self.part)
                } else {
                    b.clone()
                };
                let f =
                    std::fs::File::create(&p).with_context(|| format!("create {}", p.display()))?;
                self.files.push(p);
                Box::new(std::io::BufWriter::with_capacity(1 << 20, f))
            }
        };
        self.w = Some(match self.compress {
            Compress::None => raw,
            Compress::Gzip => Box::new(flate2::write::GzEncoder::new(
                raw,
                flate2::Compression::new(self.level.unwrap_or(6) as u32),
            )),
            Compress::Zstd => Box::new(
                zstd::stream::write::Encoder::new(raw, self.level.unwrap_or(3))?.auto_finish(),
            ),
        });
        Ok(())
    }
    fn close(&mut self) -> Result<()> {
        if let Some(mut w) = self.w.take() {
            w.flush()?;
            drop(w); // finishes gzip/zstd frames
        }
        Ok(())
    }
    /// `buf` holds whole records; split only between records.
    fn write(&mut self, buf: &[u8]) -> Result<()> {
        if self.w.is_none() {
            self.open()?;
        }
        match self.split {
            None => self.w.as_mut().unwrap().write_all(buf)?,
            Some(limit) => {
                let mut rest = buf;
                while !rest.is_empty() {
                    let end = memchr_nl(rest).map_or(rest.len(), |p| p + 1);
                    let (rec, tail) = rest.split_at(end);
                    if self.written > 0 && self.written + rec.len() as u64 > limit {
                        self.close()?;
                        self.part += 1;
                        self.written = 0;
                        self.open()?;
                    }
                    self.w.as_mut().unwrap().write_all(rec)?;
                    self.written += rec.len() as u64;
                    rest = tail;
                }
            }
        }
        Ok(())
    }
}

fn memchr_nl(b: &[u8]) -> Option<usize> {
    b.iter().position(|c| *c == b'\n')
}

fn threads(t: Option<usize>) -> usize {
    t.unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| n.get()))
}

fn main() {
    if let Err(e) = run() {
        eprintln!("edgen: error: {e:#}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Gen {
            profile,
            seed,
            count,
            start,
            format,
            index,
            ids,
            routing_pct,
            fields,
            huge_min_mb,
            huge_max_mb,
            edge_file_only,
            edge_index_cases,
            compress,
            level,
            split_bytes,
            out,
            threads: t,
        } => {
            let mut o = GenOpts::new(&profile, seed);
            o.format = match format {
                Fmt::Source => Format::Source,
                Fmt::Elasticdump => Format::Elasticdump,
                Fmt::Bulk => Format::Bulk,
            };
            if let Some(i) = index {
                o.index = i;
            }
            o.ids = match ids {
                IdMode::Det => Ids::Det,
                IdMode::Uuid => Ids::Uuid,
                IdMode::None => Ids::None,
            };
            o.routing_pct = routing_pct;
            o.params.fields = fields;
            o.params.huge_min = huge_min_mb << 20;
            o.params.huge_max = huge_max_mb << 20;
            o.edge_file_only = edge_file_only;
            o.edge_index_cases = edge_index_cases;
            if split_bytes.is_some() && out.is_none() {
                bail!("--split-bytes needs --out");
            }
            let mut sink = Sink {
                base: out,
                compress,
                level,
                split: split_bytes,
                part: 0,
                written: 0,
                w: None,
                files: vec![],
            };
            let t0 = std::time::Instant::now();
            edgen::generate(&o, start, count, threads(t), &mut |b, _| sink.write(b))?;
            sink.close()?;
            let s = t0.elapsed().as_secs_f64();
            eprintln!(
                "edgen: profile={profile} seed={seed} start={start} count={count} in {s:.2}s ({:.0} docs/s){}",
                count as f64 / s.max(1e-9),
                if sink.files.len() > 1 {
                    format!(", {} files", sink.files.len())
                } else {
                    String::new()
                }
            );
        }
        Cmd::Mapping {
            profile,
            engine,
            fields,
        } => {
            if !PROFILES.contains(&profile.as_str()) {
                bail!("unknown profile {profile}");
            }
            if !["es", "os", "os1"].contains(&engine.as_str()) {
                bail!("--engine must be es, os or os1");
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&edgen::mapping::mapping(&profile, &engine, fields))?
            );
        }
        Cmd::Cases => {
            let v: Vec<_> = edgen::edge::CASES.iter().map(|c| serde_json::json!({"case": c.name, "expect": c.expect, "file_only": c.file_only, "index_case": c.index_case})).collect();
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Cmd::Convert {
            kind,
            dataset,
            file,
            index,
            max_docs,
            input,
            out,
            compress,
        } => {
            let fname = file.unwrap_or_else(|| {
                input
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map_or("-".into(), |f| f.to_string_lossy().to_string())
            });
            let reader: Box<dyn std::io::BufRead> = match &input {
                None => Box::new(std::io::BufReader::new(std::io::stdin().lock())),
                Some(p) if p.as_os_str() == "-" => {
                    Box::new(std::io::BufReader::new(std::io::stdin().lock()))
                }
                Some(p) => {
                    let f =
                        std::fs::File::open(p).with_context(|| format!("open {}", p.display()))?;
                    let n = p.to_string_lossy();
                    if n.ends_with(".gz") {
                        Box::new(std::io::BufReader::new(flate2::read::MultiGzDecoder::new(
                            f,
                        )))
                    } else if n.ends_with(".zst") {
                        Box::new(std::io::BufReader::new(zstd::stream::read::Decoder::new(
                            f,
                        )?))
                    } else if n.ends_with(".bz2") {
                        bail!("bz2: pipe through `bzcat` and pass --input - --file {fname}");
                    } else {
                        Box::new(std::io::BufReader::with_capacity(1 << 20, f))
                    }
                }
            };
            let mut sink = Sink {
                base: out,
                compress,
                level: None,
                split: None,
                part: 0,
                written: 0,
                w: None,
                files: vec![],
            };
            sink.open()?;
            let st = {
                let w = sink.w.as_mut().unwrap();
                edgen::convert::Converter::new()
                    .run(&kind, &dataset, &fname, &index, max_docs, reader, w)?
            };
            sink.close()?;
            eprintln!(
                "edgen convert: kind={kind} dataset={dataset} file={fname} lines={} written={} skipped={}",
                st.lines, st.written, st.skipped
            );
        }
    }
    Ok(())
}
