use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// Size-bounded, self-rotating log file writer usable as a
/// `tracing_subscriber::fmt::MakeWriter`.
///
/// When the active file exceeds `max_bytes`, it is renamed to `.<1>` and
/// older backups shift up to `.<keep>`, then a fresh file is opened. All
/// writers share one `Mutex` so rotation and appends never interleave.
#[derive(Debug)]
pub struct RotatingLog {
    inner: Arc<Mutex<RotatingFile>>,
}

impl RotatingLog {
    pub fn new(path: PathBuf, max_bytes: u64, keep: usize) -> io::Result<Self> {
        let file = RotatingFile::open(&path, max_bytes, keep)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(file)),
        })
    }
}

#[derive(Debug)]
struct RotatingFile {
    path: PathBuf,
    file: File,
    size: u64,
    max_bytes: u64,
    keep: usize,
}

impl RotatingFile {
    fn open(path: &Path, max_bytes: u64, keep: usize) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path: path.to_path_buf(),
            file,
            size,
            max_bytes: max_bytes.max(1),
            keep: keep.max(1),
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        let path_str = self.path.display().to_string();
        let backup = |i: usize| format!("{path_str}.{i}");

        let oldest = backup(self.keep);
        let _ = std::fs::remove_file(&oldest);

        for i in (1..self.keep).rev() {
            let from = backup(i);
            let to = backup(i + 1);
            if Path::new(&from).exists() {
                let _ = std::fs::rename(&from, &to);
            }
        }

        self.file.flush()?;
        // Rename the live file to .1, then reopen a fresh one at the active
        // path. The old handle (now attached to the renamed inode) is dropped
        // when we replace it below.
        if Path::new(&self.path).exists() {
            std::fs::rename(&self.path, backup(1))?;
        }
        let new_file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        self.file = new_file;
        self.size = 0;
        Ok(())
    }

    fn write_inner(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size + buf.len() as u64 > self.max_bytes
            && (buf.len() as u64) < self.max_bytes
        {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.size += n as u64;
        Ok(n)
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_inner(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// One shared handle handed to tracing for every log line.
#[derive(Debug)]
pub struct RotatingLogWriter {
    inner: Arc<Mutex<RotatingFile>>,
}

impl io::Write for RotatingLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self.lock();
        guard.write_inner(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.lock().flush()
    }
}

impl RotatingLogWriter {
    fn lock(&self) -> MutexGuard<'_, RotatingFile> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RotatingLog {
    type Writer = RotatingLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RotatingLogWriter {
            inner: self.inner.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "cc-proxy-rotating-{}-{}-{name}",
            std::process::id(),
            nanos
        ))
    }

    #[test]
    fn test_rotation_shifts_backups() {
        let path = tmp_path("rotate");
        let mut rf = RotatingFile::open(&path, 64, 3).expect("open");

        for i in 0..20 {
            rf.write_inner(format!("line {i:04}\n").as_bytes()).expect("write");
        }

        assert!(path.exists(), "active file must exist");
        assert!(Path::new(&format!("{}.1", path.display())).exists(), ".1 must exist");
        assert!(Path::new(&format!("{}.2", path.display())).exists(), ".2 must exist");
        assert!(Path::new(&format!("{}.3", path.display())).exists(), ".3 must exist");
        assert!(!Path::new(&format!("{}.4", path.display())).exists(), ".4 must not exist");

        let active_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        assert!(active_size <= 64, "active file should be small, got {active_size}");
    }

    #[test]
    fn test_rotation_drops_oldest() {
        let path = tmp_path("rotate-drop");
        let mut rf = RotatingFile::open(&path, 64, 2).expect("open");

        for i in 0..40 {
            rf.write_inner(format!("line {i:04}\n").as_bytes()).expect("write");
        }

        assert!(Path::new(&format!("{}.1", path.display())).exists());
        assert!(Path::new(&format!("{}.2", path.display())).exists());
        assert!(!Path::new(&format!("{}.3", path.display())).exists());
    }

    #[test]
    fn test_writer_impl_through_make_writer() {
        let path = tmp_path("make-writer");
        let log = RotatingLog::new(path.clone(), 128, 2).expect("new");
        let mut writer = tracing_subscriber::fmt::MakeWriter::make_writer(&log);

        for i in 0..50 {
            writeln!(writer, "event {i}").expect("write");
        }
        writer.flush().expect("flush");

        assert!(path.exists());
        assert!(Path::new(&format!("{}.1", path.display())).exists());
    }

    #[test]
    fn test_single_line_larger_than_max_writes_through() {
        let path = tmp_path("big-line");
        let mut rf = RotatingFile::open(&path, 64, 2).expect("open");
        let big = vec![b'x'; 500];
        rf.write_inner(&big).expect("write");
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        assert_eq!(size, 500, "oversized line must pass through without rotating");
    }
}