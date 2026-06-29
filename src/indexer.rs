use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct LogIndexer {
    pub file_path: PathBuf,
    pub offsets: Arc<RwLock<Vec<u64>>>,
    // Keeps the temporary file alive if we are reading from stdin
    _temp_file: Option<tempfile::NamedTempFile>,
}

impl LogIndexer {
    /// Create a new LogIndexer for a log file
    pub fn new_file(path: PathBuf) -> Self {
        Self {
            file_path: path,
            offsets: Arc::new(RwLock::new(Vec::new())),
            _temp_file: None,
        }
    }

    /// Check if this indexer is reading from standard input
    pub fn is_stdin(&self) -> bool {
        self._temp_file.is_some()
    }

    /// Create a new LogIndexer for stdin, caching data in a temp file
    pub fn new_stdin() -> std::io::Result<Self> {
        let temp_file = tempfile::Builder::new()
            .prefix("taillight-stdin-")
            .suffix(".log")
            .tempfile()?;
        let file_path = temp_file.path().to_path_buf();
        Ok(Self {
            file_path,
            offsets: Arc::new(RwLock::new(Vec::new())),
            _temp_file: Some(temp_file),
        })
    }

    /// Start the background task to index the source
    pub fn start_indexing(&self) {
        let offsets = Arc::clone(&self.offsets);
        let path = self.file_path.clone();
        
        if self._temp_file.is_some() {
            // Stdin mode
            let temp_file_path = path.clone();
            tokio::spawn(async move {
                if let Ok(tokio_file) = tokio::fs::OpenOptions::new()
                    .write(true)
                    .open(&temp_file_path)
                    .await
                {
                    let _ = start_indexing_stdin(tokio_file, offsets).await;
                }
            });
        } else {
            // File mode
            tokio::spawn(async move {
                let _ = start_indexing_file(path, offsets).await;
            });
        }
    }

    /// Read a single line at the given offset
    pub fn read_line(&self, offset: u64) -> std::io::Result<String> {
        let lines = self.read_lines(&[offset])?;
        if let Some(line) = lines.into_iter().next() {
            Ok(line)
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "No line read"))
        }
    }

    /// Read lines at the given offsets
    pub fn read_lines(&self, line_offsets: &[u64]) -> std::io::Result<Vec<String>> {
        if line_offsets.is_empty() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.file_path)?;
        let mut reader = BufReader::new(file);
        let mut lines = Vec::with_capacity(line_offsets.len());
        
        for &offset in line_offsets {
            reader.seek(SeekFrom::Start(offset))?;
            let mut line = String::new();
            reader.read_line(&mut line)?;
            // Trim newline characters from the end
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            lines.push(line);
        }
        Ok(lines)
    }

    /// Get the current total line count synchronously
    pub fn line_count(&self) -> usize {
        self.offsets.read().unwrap().len()
    }
}

async fn start_indexing_file(
    path: PathBuf,
    offsets: Arc<RwLock<Vec<u64>>>,
) -> std::io::Result<()> {
    let mut file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => return Err(e),
    };
    
    let mut current_offset = 0u64;
    let mut buffer = vec![0u8; 65536];
    
    // Initial indexing pass
    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        
        let mut offsets_lock = offsets.write().unwrap();
        if current_offset == 0 && offsets_lock.is_empty() && n > 0 {
            offsets_lock.push(0);
        }
        
        for i in 0..n {
            let pos = current_offset + i as u64;
            if buffer[i] == b'\n' {
                offsets_lock.push(pos + 1);
            }
        }
        current_offset += n as u64;
    }
    
    // Tail loop
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        
        let metadata = tokio::fs::metadata(&path).await;
        if let Ok(meta) = metadata {
            let len = meta.len();
            if len < current_offset {
                // File was truncated / cleared
                {
                    let mut offsets_lock = offsets.write().unwrap();
                    offsets_lock.clear();
                    offsets_lock.push(0);
                }
                current_offset = 0;
                if let Ok(f) = tokio::fs::File::open(&path).await {
                    file = f;
                }
                continue;
            } else if len > current_offset {
                // Read newly appended bytes
                loop {
                    let n = file.read(&mut buffer).await?;
                    if n == 0 {
                        break;
                    }
                    let mut offsets_lock = offsets.write().unwrap();
                    for i in 0..n {
                        let pos = current_offset + i as u64;
                        if buffer[i] == b'\n' {
                            offsets_lock.push(pos + 1);
                        }
                    }
                    current_offset += n as u64;
                }
            }
        }
    }
}

async fn start_indexing_stdin(
    mut temp_file: tokio::fs::File,
    offsets: Arc<RwLock<Vec<u64>>>,
) -> std::io::Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut current_offset = 0u64;
    let mut buffer = vec![0u8; 65536];
    
    loop {
        let n = stdin.read(&mut buffer).await?;
        if n == 0 {
            break; // stdin closed
        }
        
        temp_file.write_all(&buffer[..n]).await?;
        temp_file.flush().await?;
        
        let mut offsets_lock = offsets.write().unwrap();
        if current_offset == 0 && offsets_lock.is_empty() && n > 0 {
            offsets_lock.push(0);
        }
        
        for i in 0..n {
            let pos = current_offset + i as u64;
            if buffer[i] == b'\n' {
                offsets_lock.push(pos + 1);
            }
        }
        current_offset += n as u64;
    }
    
    Ok(())
}
