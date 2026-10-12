//! macOS download staging keeps protected-folder access off WebKit's UI thread.
//!
//! WKDownload grants a sandbox extension synchronously when its destination
//! callback returns. Granting one for ~/Downloads stalled the app's main thread
//! in the observed export hang. Download into a private temporary directory,
//! then publish the completed file into Downloads on a worker.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
#[cfg(target_os = "macos")]
use tauri::{webview::DownloadEvent, AppHandle, Runtime};
use tempfile::{NamedTempFile, TempDir};

struct PendingDownload {
    directory: TempDir,
    name: OsString,
}

#[derive(Default)]
struct DownloadBatch {
    files: Vec<PendingDownload>,
    finished: usize,
    failed: bool,
}

#[derive(Default)]
pub(crate) struct DownloadManager {
    pending: Mutex<HashMap<String, DownloadBatch>>,
}

impl DownloadManager {
    fn start(&self, url: &str, destination: &mut PathBuf) -> io::Result<()> {
        let name = destination
            .file_name()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Missing download filename")
            })?
            .to_os_string();
        let mut builder = tempfile::Builder::new();
        builder.prefix("lotion-rs-download-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let directory = builder.tempdir()?;
        *destination = directory.path().join(&name);
        self.pending
            .lock()
            .expect("download lock poisoned")
            .entry(url.to_string())
            .or_default()
            .files
            .push(PendingDownload { directory, name });
        Ok(())
    }

    fn finish(&self, url: &str, success: bool) -> Option<DownloadBatch> {
        let mut pending = self.pending.lock().expect("download lock poisoned");
        let batch = pending.get_mut(url)?;
        batch.finished += 1;
        batch.failed |= !success;
        // Wry on macOS reports no completed path or download ID. If identical
        // URLs overlap, wait for all of them so an in-progress file cannot be
        // published. A failure discards the batch rather than exposing a partial.
        if batch.finished == batch.files.len() {
            pending.remove(url)
        } else {
            None
        }
    }
}

impl DownloadBatch {
    fn publish(self, directory: &Path) -> io::Result<Vec<PathBuf>> {
        if self.failed {
            return Err(io::Error::other("Download did not complete"));
        }
        std::fs::create_dir_all(directory)?;
        let mut paths = Vec::new();
        for download in self.files {
            let mut source = File::open(download.directory.path().join(&download.name))?;
            // Copy into a hidden sibling first; only expose a complete file.
            let mut publication = tempfile::Builder::new()
                .prefix(".lotion-download-")
                .tempfile_in(directory)?;
            io::copy(&mut source, publication.as_file_mut())?;
            #[cfg(target_os = "macos")]
            preserve_download_metadata(&source, publication.as_file())?;
            publication.as_file().sync_all()?;
            paths.push(publish_without_overwrite(
                publication,
                directory,
                &download.name,
            )?);
        }
        Ok(paths)
    }
}

#[cfg(target_os = "macos")]
fn preserve_download_metadata(source: &File, destination: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // Preserve WebKit's quarantine/origin attributes when publishing the file.
    // Copying bytes alone would lose those macOS download protections. XATTR
    // copies no file permissions, so the destination retains its private mode.
    let result = unsafe {
        libc::fcopyfile(
            source.as_raw_fd(),
            destination.as_raw_fd(),
            std::ptr::null_mut(),
            libc::COPYFILE_XATTR,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn publish_without_overwrite(
    mut file: NamedTempFile,
    directory: &Path,
    name: &OsStr,
) -> io::Result<PathBuf> {
    let original = Path::new(name);
    for number in 0..=u32::MAX {
        let candidate = if number == 0 {
            name.to_os_string()
        } else {
            let mut candidate = original.file_stem().unwrap_or(name).to_os_string();
            candidate.push(format!(" ({number})"));
            if let Some(extension) = original.extension() {
                candidate.push(".");
                candidate.push(extension);
            }
            candidate
        };
        let path = directory.join(candidate);
        match file.persist_noclobber(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                file = error.file;
            }
            Err(error) => return Err(error.error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "No unused download filename",
    ))
}

#[cfg(target_os = "macos")]
pub(crate) fn handle_download<R: Runtime>(
    app: &AppHandle<R>,
    downloads: &DownloadManager,
    event: DownloadEvent<'_>,
) -> bool {
    match event {
        DownloadEvent::Requested { url, destination } => {
            if let Err(error) = downloads.start(url.as_str(), destination) {
                log::error!("Could not prepare download: {error}");
                crate::window_controller::toast(
                    app,
                    "Download failed. Could not prepare the file.",
                );
                return false;
            }
        }
        DownloadEvent::Finished { url, success, .. } => {
            if let Some(batch) = downloads.finish(url.as_str(), success) {
                let app = app.clone();
                std::thread::spawn(move || {
                    let result = dirs::download_dir()
                        .ok_or_else(|| {
                            io::Error::new(io::ErrorKind::NotFound, "Downloads directory not found")
                        })
                        .and_then(|directory| batch.publish(&directory));
                    let message = match result {
                        Ok(paths) => {
                            let names: Vec<_> = paths
                                .iter()
                                .filter_map(|path| path.file_name())
                                .map(|name| name.to_string_lossy())
                                .collect();
                            format!("Downloaded {} to Downloads", names.join(", "))
                        }
                        Err(error) => {
                            log::error!("Could not save completed download: {error}");
                            "Download failed. Could not save the file to Downloads.".to_string()
                        }
                    };
                    let handle = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        crate::window_controller::toast(&handle, &message);
                    });
                });
            }
        }
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn published_download_retains_quarantine_metadata_and_private_permissions() {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let downloads = DownloadManager::default();
        let mut staged = directory.path().join("export.zip");
        downloads
            .start("https://example.test/export", &mut staged)
            .unwrap();
        std::fs::write(&staged, b"complete export").unwrap();
        let source = File::open(&staged).unwrap();
        let attribute = std::ffi::CString::new("com.apple.quarantine").unwrap();
        let value = b"0081;00000000;LotionTest;00000000-0000-0000-0000-000000000000";
        let result = unsafe {
            libc::fsetxattr(
                source.as_raw_fd(),
                attribute.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                0,
            )
        };
        assert_eq!(result, 0, "{}", io::Error::last_os_error());
        let mut original_metadata = [0_u8; 256];
        let original_length = unsafe {
            libc::fgetxattr(
                source.as_raw_fd(),
                attribute.as_ptr(),
                original_metadata.as_mut_ptr().cast(),
                original_metadata.len(),
                0,
                0,
            )
        };
        assert!(original_length >= 0, "{}", io::Error::last_os_error());
        drop(source);
        let paths = downloads
            .finish("https://example.test/export", true)
            .unwrap()
            .publish(directory.path())
            .unwrap();
        let published = File::open(&paths[0]).unwrap();
        let mut metadata = [0_u8; 256];
        let length = unsafe {
            libc::fgetxattr(
                published.as_raw_fd(),
                attribute.as_ptr(),
                metadata.as_mut_ptr().cast(),
                metadata.len(),
                0,
                0,
            )
        };
        assert!(length >= 0, "{}", io::Error::last_os_error());
        assert_eq!(
            &metadata[..length as usize],
            &original_metadata[..original_length as usize]
        );
        assert_eq!(
            published.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn complete_download_is_staged_then_published_and_cleaned_up() {
        let directory = tempfile::tempdir().unwrap();
        let downloads = DownloadManager::default();
        let original = directory.path().join("Résumé export.zip");
        let mut staged = original.clone();
        downloads
            .start("https://example.test/export", &mut staged)
            .unwrap();
        assert_ne!(staged.parent(), original.parent());
        assert_eq!(staged.file_name(), original.file_name());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(staged.parent().unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        std::fs::write(&staged, b"complete export").unwrap();
        let batch = downloads
            .finish("https://example.test/export", true)
            .unwrap();
        assert_eq!(
            batch.publish(directory.path()).unwrap(),
            vec![original.clone()]
        );
        assert_eq!(std::fs::read(original).unwrap(), b"complete export");
        assert!(!staged.exists());
    }

    #[test]
    fn existing_downloads_are_never_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("export.zip");
        std::fs::write(&original, b"original file").unwrap();
        let downloads = DownloadManager::default();
        let mut staged = original.clone();
        downloads
            .start("https://example.test/export", &mut staged)
            .unwrap();
        std::fs::write(staged, b"new export").unwrap();
        let batch = downloads
            .finish("https://example.test/export", true)
            .unwrap();
        let paths = batch.publish(directory.path()).unwrap();
        assert_eq!(paths, vec![directory.path().join("export (1).zip")]);
        assert_eq!(std::fs::read(original).unwrap(), b"original file");
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"new export");
    }

    #[test]
    fn failed_download_does_not_publish_a_partial_file() {
        let directory = tempfile::tempdir().unwrap();
        let downloads = DownloadManager::default();
        let mut staged = directory.path().join("export.zip");
        downloads
            .start("https://example.test/export", &mut staged)
            .unwrap();
        std::fs::write(&staged, b"partial").unwrap();
        let batch = downloads
            .finish("https://example.test/export", false)
            .unwrap();
        assert!(batch.publish(directory.path()).is_err());
        assert!(!staged.exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn overlapping_identical_urls_wait_for_all_completions() {
        let directory = tempfile::tempdir().unwrap();
        let downloads = DownloadManager::default();
        let url = "https://example.test/export";
        let mut first = directory.path().join("export.zip");
        let mut second = first.clone();
        downloads.start(url, &mut first).unwrap();
        downloads.start(url, &mut second).unwrap();
        std::fs::write(&first, b"first complete export").unwrap();
        assert!(downloads.finish(url, true).is_none());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        std::fs::write(&second, b"second complete export").unwrap();
        let paths = downloads
            .finish(url, true)
            .unwrap()
            .publish(directory.path())
            .unwrap();
        assert_eq!(paths.len(), 2);
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"first complete export");
        assert_eq!(std::fs::read(&paths[1]).unwrap(), b"second complete export");
        assert!(!first.exists() && !second.exists());
    }

    #[test]
    fn one_failed_duplicate_cannot_publish_partial_siblings() {
        let directory = tempfile::tempdir().unwrap();
        let downloads = DownloadManager::default();
        let url = "https://example.test/export";
        let mut first = directory.path().join("export.zip");
        let mut second = first.clone();
        downloads.start(url, &mut first).unwrap();
        downloads.start(url, &mut second).unwrap();
        std::fs::write(&first, b"complete").unwrap();
        std::fs::write(&second, b"partial").unwrap();
        assert!(downloads.finish(url, false).is_none());
        assert!(downloads
            .finish(url, true)
            .unwrap()
            .publish(directory.path())
            .is_err());
        assert!(!first.exists() && !second.exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn publication_failure_cleans_up_the_staged_download() {
        let directory = tempfile::tempdir().unwrap();
        let not_a_directory = directory.path().join("file");
        std::fs::write(&not_a_directory, b"existing").unwrap();
        let downloads = DownloadManager::default();
        let mut staged = directory.path().join("export.zip");
        downloads
            .start("https://example.test/export", &mut staged)
            .unwrap();
        std::fs::write(&staged, b"complete").unwrap();
        let batch = downloads
            .finish("https://example.test/export", true)
            .unwrap();
        assert!(batch.publish(&not_a_directory).is_err());
        assert!(!staged.exists());
        assert_eq!(std::fs::read(not_a_directory).unwrap(), b"existing");
    }
}
