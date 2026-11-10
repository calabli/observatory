//! Codex rollout files come in two shapes: `rollout-*.jsonl` and the compressed
//! `rollout-*.jsonl.zst`.
//!
//! Codex's `[features] local_thread_store_compression` (and the app-server
//! `rollout/compress` call, which that switch does not control) compresses rollouts
//! that have not been touched for 7 days into a same-named `.jsonl.zst`
//! (single-frame zstd, mtime preserved) and deletes the original; when a conversation
//! continues on a compressed session, Codex first decompresses it back to `.jsonl`
//! and appends. Session management, usage import and historical bucketing migration
//! all read and write through here, and both shapes are transparent to them.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use crate::config::atomic_write;
use crate::error::AppError;

const COMPRESSED_SUFFIX: &str = ".zst";
const PLAIN_SUFFIX: &str = ".jsonl";
const READER_CAPACITY: usize = 1 << 20;
/// Unrelated to the level Codex compresses with; only used when CC Switch rewrites a compressed file in place (historical bucketing migration).
const WRITE_LEVEL: i32 = 3;

fn file_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

/// Whether the path is in compressed form (`*.jsonl.zst`).
pub fn is_compressed(path: &Path) -> bool {
    file_name(path).is_some_and(|name| {
        name.strip_suffix(COMPRESSED_SUFFIX)
            .is_some_and(|plain| plain.ends_with(PLAIN_SUFFIX))
    })
}

/// Whether the path is a rollout file (`*.jsonl` or `*.jsonl.zst`), based on the extension alone.
pub fn is_rollout_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") || is_compressed(path)
}

/// The plain-form path with `.zst` removed; a plain path is returned unchanged.
pub fn logical_path(path: &Path) -> PathBuf {
    match file_name(path)
        .filter(|_| is_compressed(path))
        .and_then(|name| name.strip_suffix(COMPRESSED_SUFFIX))
    {
        Some(plain) => path.with_file_name(plain),
        None => path.to_path_buf(),
    }
}

/// The path of the other form: `x.jsonl` <-> `x.jsonl.zst`. Returns `None` when the path is not a rollout file.
pub fn sibling(path: &Path) -> Option<PathBuf> {
    if is_compressed(path) {
        return Some(logical_path(path));
    }
    if !is_rollout_file(path) {
        return None;
    }
    let name = file_name(path)?;
    Some(path.with_file_name(format!("{name}{COMPRESSED_SUFFIX}")))
}

/// Read line by line; compressed files are decompressed transparently.
pub fn open_reader(path: &Path) -> io::Result<Box<dyn BufRead + Send>> {
    reader_from_file(path, File::open(path)?)
}

/// Same as [`open_reader`], but uses a file the caller already opened (for callers that also need metadata from the same handle).
pub fn reader_from_file(path: &Path, file: File) -> io::Result<Box<dyn BufRead + Send>> {
    if is_compressed(path) {
        let decoder = zstd::stream::read::Decoder::new(file)?;
        Ok(Box::new(BufReader::with_capacity(READER_CAPACITY, decoder)))
    } else {
        Ok(Box::new(BufReader::with_capacity(READER_CAPACITY, file)))
    }
}

/// Read the whole file (UTF-8); compressed files are decompressed first.
pub fn read_to_string(path: &Path) -> io::Result<String> {
    if !is_compressed(path) {
        return std::fs::read_to_string(path);
    }
    let mut text = String::new();
    open_reader(path)?.read_to_string(&mut text)?;
    Ok(text)
}

/// Write back atomically; a compressed path is encoded to zstd first, keeping the file in its original form.
///
/// The compressed form also keeps the original mtime: the session list does not decompress
/// the tail, so the last activity time of a compressed session is its mtime (Codex preserved
/// it while compressing too). Rewriting metadata must not make an old session look freshly active.
pub fn write_rollout(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    if !is_compressed(path) {
        return atomic_write(path, bytes);
    }
    let modified = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok();
    let encoded =
        zstd::stream::encode_all(bytes, WRITE_LEVEL).map_err(|error| AppError::io(path, error))?;
    atomic_write(path, &encoded)?;
    if let Some(modified) = modified {
        File::options()
            .write(true)
            .open(path)
            .and_then(|file| file.set_times(std::fs::FileTimes::new().set_modified(modified)))
            .map_err(|error| AppError::io(path, error))?;
    }
    Ok(())
}

/// Logical file name (with `.zst` removed). `path` may be a path string from any platform (stored verbatim in the cursor).
pub fn logical_file_name(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.strip_suffix(COMPRESSED_SUFFIX) {
        Some(plain) if plain.ends_with(PLAIN_SUFFIX) => plain,
        _ => name,
    }
}

/// When both forms of the same logical name exist (the moment a file is compressed or
/// decompressed) only the plain form is kept: it is the one Codex is writing, so it holds
/// at least as much content as the compressed one. The original order is preserved.
pub fn dedupe_siblings(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let plain: HashSet<PathBuf> = paths
        .iter()
        .filter(|path| !is_compressed(path))
        .cloned()
        .collect();
    paths
        .into_iter()
        .filter(|path| !is_compressed(path) || !plain.contains(&logical_path(path)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_both_forms() {
        let plain = Path::new("/x/rollout-a.jsonl");
        let zst = Path::new("/x/rollout-a.jsonl.zst");
        assert!(is_rollout_file(plain));
        assert!(is_rollout_file(zst));
        assert!(!is_compressed(plain));
        assert!(is_compressed(zst));
        assert!(!is_rollout_file(Path::new("/x/rollout-a.zst")));
        assert!(!is_rollout_file(Path::new("/x/notes.txt")));
        assert!(!is_compressed(Path::new("/x/rollout-a.zst")));
    }

    #[test]
    fn logical_and_sibling_paths() {
        let plain = PathBuf::from("/x/rollout-a.jsonl");
        let zst = PathBuf::from("/x/rollout-a.jsonl.zst");
        assert_eq!(logical_path(&zst), plain);
        assert_eq!(logical_path(&plain), plain);
        assert_eq!(sibling(&plain), Some(zst.clone()));
        assert_eq!(sibling(&zst), Some(plain));
        assert_eq!(sibling(Path::new("/x/a.txt")), None);
    }

    #[test]
    fn logical_file_name_handles_both_separators() {
        assert_eq!(
            logical_file_name("/a/b/rollout-x.jsonl.zst"),
            "rollout-x.jsonl"
        );
        assert_eq!(
            logical_file_name("C:\\a\\rollout-x.jsonl"),
            "rollout-x.jsonl"
        );
        assert_eq!(logical_file_name("rollout-x.zst"), "rollout-x.zst");
    }

    #[test]
    fn dedupe_keeps_plain_and_order() {
        let paths = vec![
            PathBuf::from("/x/b.jsonl.zst"),
            PathBuf::from("/x/a.jsonl.zst"),
            PathBuf::from("/x/a.jsonl"),
            PathBuf::from("/y/a.jsonl.zst"),
        ];
        assert_eq!(
            dedupe_siblings(paths),
            vec![
                PathBuf::from("/x/b.jsonl.zst"),
                PathBuf::from("/x/a.jsonl"),
                PathBuf::from("/y/a.jsonl.zst"),
            ]
        );
    }

    #[test]
    fn rewriting_compressed_rollout_keeps_mtime() {
        let temp = tempfile::tempdir().expect("tempdir");
        let zst = temp.path().join("rollout-a.jsonl.zst");
        write_rollout(&zst, b"{\"a\":1}\n").expect("write");
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        File::options()
            .write(true)
            .open(&zst)
            .expect("open")
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .expect("set mtime");

        write_rollout(&zst, b"{\"a\":2}\n").expect("rewrite");

        let modified = std::fs::metadata(&zst)
            .expect("meta")
            .modified()
            .expect("mtime");
        assert_eq!(modified, old);
        assert_eq!(read_to_string(&zst).expect("read"), "{\"a\":2}\n");
    }

    #[test]
    fn read_and_write_round_trip() {
        let temp = tempfile::tempdir().expect("tempdir");
        let zst = temp.path().join("rollout-a.jsonl.zst");
        let plain = temp.path().join("rollout-b.jsonl");
        let text = "{\"a\":1}\n{\"b\":2}\n";

        write_rollout(&zst, text.as_bytes()).expect("write zst");
        write_rollout(&plain, text.as_bytes()).expect("write plain");

        let raw = std::fs::read(&zst).expect("raw");
        assert_eq!(&raw[..4], &[0x28, 0xB5, 0x2F, 0xFD], "zstd magic");
        assert_eq!(read_to_string(&zst).expect("read zst"), text);
        assert_eq!(read_to_string(&plain).expect("read plain"), text);
        let lines: Vec<String> = open_reader(&zst)
            .expect("reader")
            .lines()
            .collect::<Result<_, _>>()
            .expect("lines");
        assert_eq!(lines, vec!["{\"a\":1}", "{\"b\":2}"]);
    }
}
