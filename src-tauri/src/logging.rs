use std::io::Write;
use std::path::PathBuf;

/// Preprost dnevnik v %APPDATA%\com.jbj.mailhub\mailhub.log.
/// Uporablja se samo za odpravljanje težav (prijava, sinhronizacija).
pub fn log_line(message: impl AsRef<str>) {
    let Some(dir) = log_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("mailhub.log");
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 2_000_000 {
            let _ = std::fs::remove_file(&path);
        }
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(
            file,
            "[{}] {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            message.as_ref()
        );
    }
}

pub fn log_dir() -> Option<PathBuf> {
    std::env::var("APPDATA")
        .ok()
        .map(|base| PathBuf::from(base).join("com.jbj.mailhub"))
}
