use anyhow::{Context, Result, anyhow, bail};
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs as tokio_fs,
    io::{AsyncWriteExt, BufWriter as TokioBufWriter, stdout as tokio_stdout},
};

/// Concrete, buffered sink writers. Keeping the file variant a concrete
/// `tokio_fs::File` (rather than a boxed `dyn AsyncWrite`) is what lets
/// `finalize` recover the underlying handle to `sync_all()` it before commit.
enum SinkWriter {
    Stdout(TokioBufWriter<tokio::io::Stdout>),
    File(TokioBufWriter<tokio_fs::File>),
}

impl SinkWriter {
    async fn write_all(&mut self, buffer: &[u8]) -> std::io::Result<()> {
        match self {
            SinkWriter::Stdout(writer) => writer.write_all(buffer).await,
            SinkWriter::File(writer) => writer.write_all(buffer).await,
        }
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        match self {
            SinkWriter::Stdout(writer) => writer.flush().await,
            SinkWriter::File(writer) => writer.flush().await,
        }
    }
}

enum OutputMode {
    Stdout,
    StagedFile {
        final_path: PathBuf,
        staging_path: PathBuf,
        overwrite: bool,
        /// Mode of a pre-existing destination being overwritten, preserved onto
        /// the staged file before commit. Always `None` off unix.
        prior_mode: Option<u32>,
    },
}

pub struct OutputTarget {
    writer: Option<SinkWriter>,
    mode: OutputMode,
}

impl OutputTarget {
    pub async fn write_all(&mut self, buffer: &[u8]) -> Result<()> {
        let r = self.writer_mut()?.write_all(buffer).await;
        r.with_context(|| self.write_failed())
    }

    pub async fn flush(&mut self) -> Result<()> {
        let r = self.writer_mut()?.flush().await;
        r.with_context(|| self.write_failed())
    }

    /// Names where a failed write went (TST-BRK-RES-003: ENOSPC on stdout or a full disk).
    fn write_failed(&self) -> String {
        match &self.mode {
            OutputMode::Stdout => "writing to stdout failed".to_string(),
            OutputMode::StagedFile { final_path, .. } => format!(
                "writing the output file '{}' failed (nothing is committed under that name)",
                final_path.display()
            ),
        }
    }

    pub async fn finalize(self) -> Result<()> {
        match self.seal().await? {
            Some(sealed) => sealed.commit().await,
            None => Ok(()),
        }
    }

    /// Flush and fsync a staged file without committing it, closing its
    /// handle (so many parts can be staged at once). Stdout is flushed and
    /// yields `None`.
    pub async fn seal(mut self) -> Result<Option<SealedOutput>> {
        let writer = self.writer.take();

        match self.mode {
            OutputMode::Stdout => {
                if let Some(mut writer) = writer {
                    if let Err(error) = writer.flush().await {
                        let error = anyhow::Error::from(error);
                        // A vanished pipe reader is not our problem to report;
                        // there is nothing left to flush to.
                        if is_broken_pipe(&error) {
                            return Ok(None);
                        }
                        return Err(error).context("Failed to flush stdout");
                    }
                }
                Ok(None)
            }
            OutputMode::StagedFile {
                final_path,
                staging_path,
                overwrite,
                prior_mode,
            } => {
                // Flush the buffer, then fsync the file's data to disk *before*
                // it is committed so a crash after commit cannot expose a
                // truncated "successful" output.
                let Some(SinkWriter::File(mut buffered)) = writer else {
                    remove_staging(&staging_path).await;
                    return Err(anyhow!(
                        "Staged output '{}' had no file writer to finalize",
                        staging_path.display()
                    ));
                };

                if let Err(error) = buffered.flush().await {
                    remove_staging(&staging_path).await;
                    return Err(error).with_context(|| {
                        format!("Failed to flush staged output '{}'", staging_path.display())
                    });
                }

                let file = buffered.into_inner();
                if let Err(error) = file.sync_all().await {
                    remove_staging(&staging_path).await;
                    return Err(error).with_context(|| {
                        format!("Failed to fsync staged output '{}'", staging_path.display())
                    });
                }
                drop(file);

                // Preserve the overwritten destination's permissions instead of
                // leaking the staging file's umask-default mode onto it.
                #[cfg(unix)]
                if let Some(mode) = prior_mode {
                    use std::os::unix::fs::PermissionsExt;
                    let permissions = std::fs::Permissions::from_mode(mode);
                    if let Err(error) = tokio_fs::set_permissions(&staging_path, permissions).await
                    {
                        remove_staging(&staging_path).await;
                        return Err(error).with_context(|| {
                            format!(
                                "Failed to preserve mode {mode:#o} on staged output '{}'",
                                staging_path.display()
                            )
                        });
                    }
                }
                #[cfg(not(unix))]
                let _ = prior_mode;

                Ok(Some(SealedOutput {
                    final_path,
                    staging_path,
                    overwrite,
                }))
            }
        }
    }

    pub async fn abort(mut self) {
        self.close_writer();

        if let OutputMode::StagedFile { staging_path, .. } = &self.mode {
            remove_staging(staging_path).await;
        }
    }

    fn writer_mut(&mut self) -> Result<&mut SinkWriter> {
        self.writer
            .as_mut()
            .ok_or_else(|| anyhow!("Output writer is no longer available"))
    }

    fn close_writer(&mut self) {
        self.writer.take();
    }
}

/// A fully written, fsynced staged file awaiting its atomic commit.
pub struct SealedOutput {
    final_path: PathBuf,
    staging_path: PathBuf,
    overwrite: bool,
}

impl SealedOutput {
    pub fn final_path(&self) -> &Path {
        &self.final_path
    }

    /// Whether committing may replace an existing file (`--overwrite`).
    pub fn replaces(&self) -> bool {
        self.overwrite
    }

    /// Move the staged file into place (rename with `--overwrite`, otherwise
    /// a race-free hard link that refuses to clobber a file created meanwhile).
    pub async fn commit(self) -> Result<()> {
        let Self {
            final_path,
            staging_path,
            overwrite,
        } = self;
        let commit_result = if overwrite {
            tokio_fs::rename(&staging_path, &final_path)
                .await
                .with_context(|| {
                    format!(
                        "Failed to move staged output '{}' into place at '{}'",
                        staging_path.display(),
                        final_path.display()
                    )
                })
        } else {
            match tokio_fs::hard_link(&staging_path, &final_path).await {
                Ok(()) => tokio_fs::remove_file(&staging_path).await.with_context(|| {
                    format!(
                        "Failed to remove staged output '{}' after linking it into place at '{}'",
                        staging_path.display(),
                        final_path.display()
                    )
                }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(anyhow!(
                    "Output file '{}' was created while the dump was running. Use --overwrite to replace it.",
                    final_path.display()
                )),
                Err(e) => {
                    log::debug!("hard_link commit unsupported ({e}); falling back to rename");
                    tokio_fs::rename(&staging_path, &final_path)
                        .await
                        .with_context(|| {
                            format!(
                                "Failed to move staged output '{}' into place at '{}'",
                                staging_path.display(),
                                final_path.display()
                            )
                        })
                }
            }
        };

        if let Err(error) = commit_result {
            remove_staging(&staging_path).await;
            return Err(error);
        }

        sync_parent_dir(&final_path).await;
        Ok(())
    }

    /// Discard the staged file.
    pub async fn discard(self) {
        remove_staging(&self.staging_path).await;
    }
}

/// Returns `true` if any error in the chain is an [`std::io::Error`] with
/// [`std::io::ErrorKind::BrokenPipe`].
pub fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == std::io::ErrorKind::BrokenPipe)
    })
}

/// Remove a staged output file, ignoring a missing file and warning on anything
/// else. Shared by `finalize`'s error paths and `abort`.
async fn remove_staging(staging_path: &Path) {
    if let Err(error) = tokio_fs::remove_file(staging_path).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            log::warn!(
                "Failed to remove staged output '{}': {}",
                staging_path.display(),
                error
            );
        }
    }
}

/// Best-effort fsync of the destination's parent directory so the rename/link
/// that committed the file is itself durable. Failures are logged, not fatal.
#[cfg(unix)]
async fn sync_parent_dir(final_path: &Path) {
    let parent = match final_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let display = parent.display().to_string();
    let result = tokio::task::spawn_blocking(move || {
        std::fs::File::open(&parent).and_then(|d| d.sync_all())
    })
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => log::warn!("Failed to fsync output directory '{display}': {error}"),
        Err(error) => log::warn!("Output directory fsync task for '{display}' failed: {error}"),
    }
}

#[cfg(not(unix))]
async fn sync_parent_dir(_final_path: &Path) {}

/// What a non-regular file is, for an error message.
fn kind_of(file_type: &std::fs::FileType) -> &'static str {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if file_type.is_fifo() {
            return "a FIFO (named pipe)";
        }
        if file_type.is_char_device() {
            return "a character device";
        }
        if file_type.is_block_device() {
            return "a block device";
        }
        if file_type.is_socket() {
            return "a socket";
        }
    }
    if file_type.is_dir() {
        return "a directory";
    }
    "not a regular file"
}

fn not_regular(path: &Path, kind: &str) -> anyhow::Error {
    anyhow!(
        "--output '{}' is {kind}, not a regular file. Output files are written to a hidden staging file next to it and then committed atomically, which needs a regular file; to stream into a pipe or device use --output $ and redirect stdout",
        path.display()
    )
}

/// L-644 (TST-BRK-CLI-005): refuse an output path that a staged, atomic
/// commit cannot handle, with a message naming the problem, before anything
/// is written:
///
/// * no file name (`/`, `.`, `dir/..`) or an existing directory;
/// * a symbolic link (dangling or not). Decision: neither the link nor its
///   target is replaced (a rename would replace the link, a write-through
///   the target); pass the target path to write there;
/// * a FIFO, device or socket (including `/dev/stdout`: use `--output $`);
/// * a parent that does not exist or is not a directory.
///
/// `..` components are fine (the path is used as given). An existing regular
/// file is checked separately (`--overwrite`). Checked once, on `--output`
/// (lib.rs `validate`); split parts and anything that changes during the run
/// are left to the commit: a directory fails it (all-or-nothing), a symlink
/// that appears is never written through (see the race test below).
pub fn check_output_path(path: &Path) -> Result<()> {
    let shown = path.display();
    if path.as_os_str().is_empty() {
        bail!("--output must not be empty");
    }
    if ["/dev/stdout", "/dev/fd/1", "/proc/self/fd/1"]
        .iter()
        .any(|p| path == Path::new(p))
    {
        bail!("--output '{shown}': use --output $ to write to stdout");
    }
    if path.file_name().is_none() {
        bail!("--output '{shown}' must name a file, not a directory");
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(path)
                .map(|t| t.display().to_string())
                .unwrap_or_else(|_| "?".to_string());
            return match std::fs::metadata(path) {
                Ok(t) if !t.is_file() => Err(not_regular(path, kind_of(&t.file_type()))),
                Ok(_) => Err(anyhow!(
                    "--output '{shown}' is a symbolic link (to '{target}'); refusing to replace the link or its target. Pass the target path itself to write there, or remove the link"
                )),
                Err(_) => Err(anyhow!(
                    "--output '{shown}' is a symbolic link to '{target}', which does not exist; refusing to replace the link or create its target. Pass the target path itself, or remove the link"
                )),
            };
        }
        Ok(meta) if meta.is_dir() => {
            bail!(
                "--output '{shown}' is a directory; name a file inside it (for example '{}')",
                path.join("dump.json").display()
            );
        }
        Ok(meta) if !meta.is_file() => return Err(not_regular(path, kind_of(&meta.file_type()))),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => {
            bail!("--output '{shown}': a parent of it is a file, not a directory");
        }
        Err(e) => return Err(e).with_context(|| format!("cannot inspect --output '{shown}'")),
    }
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    match std::fs::metadata(parent) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => bail!(
            "--output '{shown}': '{}' is not a directory",
            parent.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match dangling_link_in(parent) {
            Some((link, target)) => bail!(
                "--output '{shown}': '{}' is a symbolic link to '{}', which does not exist (create the target directory or fix the link)",
                link.display(),
                target.display()
            ),
            None => bail!(
                "--output '{shown}': the directory '{}' does not exist (create it first)",
                parent.display()
            ),
        },
        Err(e) => Err(e).with_context(|| {
            format!(
                "--output '{shown}': cannot access the directory '{}'",
                parent.display()
            )
        }),
    }
}

/// L-644a: symlinked directories in the parent path of `--output` are
/// followed (only a symlink as the final component is refused). When the
/// parent is missing, name the first dangling link on the way so the error
/// says why, instead of a generic "does not exist".
fn dangling_link_in(parent: &Path) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let mut ancestors: Vec<&Path> = parent
        .ancestors()
        .filter(|a| !a.as_os_str().is_empty())
        .collect();
    ancestors.reverse();
    ancestors.into_iter().find_map(|a| {
        let is_link = std::fs::symlink_metadata(a)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_link && std::fs::metadata(a).is_err() {
            let target = std::fs::read_link(a).unwrap_or_default();
            Some((a.to_path_buf(), target))
        } else {
            None
        }
    })
}

/// Capture the mode of a pre-existing destination that is about to be
/// overwritten, so it can be re-applied to the staged file before commit.
#[cfg(unix)]
fn capture_prior_mode(final_path: &Path, overwrite: bool) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    if overwrite {
        // symlink_metadata: a link is refused up front; never copy a
        // target's mode onto a file replacing the link (race)
        std::fs::symlink_metadata(final_path)
            .ok()
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.permissions().mode())
    } else {
        None
    }
}

#[cfg(not(unix))]
fn capture_prior_mode(_final_path: &Path, _overwrite: bool) -> Option<u32> {
    None
}

/// Create an output target: stdout when `path` is `None`, otherwise a staged
/// file committed atomically by [`OutputTarget::finalize`].
pub async fn create_output_target(path: Option<&Path>, overwrite: bool) -> Result<OutputTarget> {
    log::debug!("Setting up output writer");

    let Some(path) = path else {
        log::debug!("Using stdout for output");
        let stdout = tokio_stdout();
        return Ok(OutputTarget {
            writer: Some(SinkWriter::Stdout(TokioBufWriter::new(stdout))),
            mode: OutputMode::Stdout,
        });
    };
    {
        log::debug!("Using file for output: {}", path.display());
        let final_path = path.to_path_buf();

        if !overwrite && final_path.exists() {
            return Err(anyhow!(
                "File `{}` already exists, quitting (use --overwrite to replace it)",
                path.display()
            ));
        }

        let prior_mode = capture_prior_mode(&final_path, overwrite);

        let staging_path = build_staging_path(&final_path)?;
        let mut open_options = tokio_fs::OpenOptions::new();
        open_options.write(true).create_new(true);

        let file = match open_options.open(&staging_path).await {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                let dir = staging_path.parent().unwrap_or_else(|| Path::new("."));
                return Err(e).with_context(|| {
                    format!(
                        "cannot write --output '{}': no permission to create files in the directory '{}' (the output is staged there and committed atomically)",
                        final_path.display(),
                        dir.display()
                    )
                });
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "Failed to create staged output file '{}' for '{}'",
                        staging_path.display(),
                        final_path.display()
                    )
                });
            }
        };

        log::debug!(
            "Output file staged successfully at {}",
            staging_path.display()
        );
        Ok(OutputTarget {
            writer: Some(SinkWriter::File(TokioBufWriter::new(file))),
            mode: OutputMode::StagedFile {
                final_path,
                staging_path,
                overwrite,
                prior_mode,
            },
        })
    }
}

fn build_staging_path(final_path: &Path) -> Result<PathBuf> {
    let parent = final_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = final_path
        .file_name()
        .ok_or_else(|| anyhow!("Output path '{}' must name a file", final_path.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before UNIX_EPOCH")?
        .as_nanos();
    let staging_name = format!(
        ".{}.part-{}-{}",
        file_name.to_string_lossy(),
        std::process::id(),
        timestamp
    );

    Ok(parent.join(staging_name))
}

#[cfg(test)]
mod tests {
    use super::{OutputTarget, create_output_target};
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    /// Collects any `.{name}.part-...` staging siblings left in `dir`.
    fn staging_siblings(dir: &Path, file_name: &str) -> Vec<PathBuf> {
        let prefix = format!(".{file_name}.part-");
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.starts_with(&prefix))
                    .unwrap_or(false)
            })
            .collect()
    }

    async fn write_line(target: &mut OutputTarget, line: &[u8]) {
        target.write_all(line).await.unwrap();
    }

    #[tokio::test]
    async fn finalize_commits_staged_file_and_removes_staging() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("out.jsonl");

        let mut target = create_output_target(Some(&dest), false).await.unwrap();
        write_line(&mut target, b"line-1\n").await;
        target.finalize().await.unwrap();

        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "line-1\n");
        assert!(
            staging_siblings(dir.path(), "out.jsonl").is_empty(),
            "staging file should be gone after a successful commit"
        );
    }

    #[tokio::test]
    async fn finalize_without_overwrite_fails_if_destination_appeared_meanwhile() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("out.jsonl");

        // Destination is absent when the target is created.
        let mut target = create_output_target(Some(&dest), false).await.unwrap();
        write_line(&mut target, b"fresh\n").await;

        // A racing writer creates the destination while the dump is running.
        std::fs::write(&dest, b"pre-existing\n").unwrap();

        let error = target.finalize().await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("was created while the dump was running"),
            "unexpected error: {error}"
        );
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "pre-existing\n",
            "existing destination must not be clobbered by a non-overwrite commit"
        );
        assert!(
            staging_siblings(dir.path(), "out.jsonl").is_empty(),
            "staging file should be removed after a failed commit"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn finalize_with_overwrite_replaces_and_preserves_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let dest = dir.path().join("out.jsonl");
        std::fs::write(&dest, b"old\n").unwrap();
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600)).unwrap();

        let mut target = create_output_target(Some(&dest), true).await.unwrap();
        write_line(&mut target, b"new\n").await;
        target.finalize().await.unwrap();

        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "new\n");
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "destination mode should be preserved on overwrite"
        );
        assert!(
            staging_siblings(dir.path(), "out.jsonl").is_empty(),
            "staging file should be gone after a successful commit"
        );
    }

    /// L-644: what `check_output_path` refuses and accepts.
    #[cfg(unix)]
    #[test]
    fn output_path_checks() {
        use super::check_output_path;
        let dir = tempdir().unwrap();
        let d = dir.path();
        let msg = |p: &Path| check_output_path(p).unwrap_err().to_string();
        assert!(msg(d).contains("is a directory"), "{}", msg(d));
        assert!(msg(Path::new("/")).contains("must name a file"));
        assert!(msg(&d.join("sub/..")).contains("must name a file"));
        assert!(msg(Path::new("")).contains("must not be empty"));
        assert!(msg(Path::new("/dev/stdout")).contains("--output $"));
        assert!(msg(Path::new("/dev/null")).contains("character device"));
        let file = d.join("f.json");
        std::fs::write(&file, "x").unwrap();
        check_output_path(&file).unwrap();
        check_output_path(&d.join("new.json")).unwrap();
        check_output_path(&d.join("sub").join("..").join("new.json")).unwrap_err();
        std::fs::create_dir(d.join("sub")).unwrap();
        check_output_path(&d.join("sub").join("..").join("new.json")).unwrap();
        assert!(msg(&d.join("nope/x.json")).contains("does not exist"));
        assert!(msg(&file.join("x")).contains("not a directory"));
        let link = d.join("link.json");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(msg(&link).contains("symbolic link"), "{}", msg(&link));
        let dangling = d.join("dangling.json");
        std::os::unix::fs::symlink(d.join("nowhere"), &dangling).unwrap();
        assert!(
            msg(&dangling).contains("does not exist"),
            "{}",
            msg(&dangling)
        );
        let dlink = d.join("dlink");
        std::os::unix::fs::symlink(d.join("sub"), &dlink).unwrap();
        assert!(msg(&dlink).contains("a directory"), "{}", msg(&dlink));
        // a symlinked parent directory is fine: only the last component counts
        check_output_path(&dlink.join("x.json")).unwrap();
    }

    /// L-644 decision: if a symlink appears at the destination while the dump
    /// runs, its target is never written. Without --overwrite the hard-link
    /// commit fails and both stay; with --overwrite the rename replaces the
    /// link itself (never the target).
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_appearing_during_the_dump_never_replaces_its_target() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target.json");
        std::fs::write(&target, b"target\n").unwrap();
        for overwrite in [false, true] {
            let dest = dir.path().join(format!("out-{overwrite}.jsonl"));
            let mut t = create_output_target(Some(&dest), overwrite).await.unwrap();
            write_line(&mut t, b"new\n").await;
            std::os::unix::fs::symlink(&target, &dest).unwrap();
            let r = t.finalize().await;
            assert_eq!(std::fs::read(&target).unwrap(), b"target\n");
            if overwrite {
                r.unwrap();
                assert!(!dest.symlink_metadata().unwrap().file_type().is_symlink());
                assert_eq!(std::fs::read(&dest).unwrap(), b"new\n");
            } else {
                assert!(r.unwrap_err().to_string().contains("was created while"));
                assert!(dest.symlink_metadata().unwrap().file_type().is_symlink());
            }
            assert!(staging_siblings(dir.path(), &format!("out-{overwrite}.jsonl")).is_empty());
        }
    }

    #[tokio::test]
    async fn abort_removes_staging_and_keeps_existing_destination() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("out.jsonl");
        std::fs::write(&dest, b"keep-me\n").unwrap();

        let mut target = create_output_target(Some(&dest), true).await.unwrap();
        write_line(&mut target, b"partial\n").await;
        target.abort().await;

        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "keep-me\n",
            "existing destination must survive an abort"
        );
        assert!(
            staging_siblings(dir.path(), "out.jsonl").is_empty(),
            "staging file should be removed after abort"
        );
    }
}
