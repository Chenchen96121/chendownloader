use std::{
    io,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender},
        Mutex,
    },
    time::Duration,
};

use local_media_downloader_lib::infrastructure::output::{
    PublicationOperations, StdPublicationOperations,
};

/// Keep real file publication and simulate only the cross-volume boundary and
/// the moment at which a user cancels an in-progress copy.
pub struct PausedCrossVolumeCopy {
    pub entered: Sender<()>,
    pub resume: Mutex<Receiver<()>>,
    pub paused: AtomicBool,
}

impl PublicationOperations for PausedCrossVolumeCopy {
    fn move_no_replace(&self, source: &Path, destination: &Path) -> io::Result<()> {
        if source.file_name().is_some_and(|name| name == "stage.mp4") {
            return Err(io::Error::from_raw_os_error(17));
        }
        StdPublicationOperations.move_no_replace(source, destination)
    }

    fn copy_to_new_and_sync(&self, source: &Path, destination: &Path) -> io::Result<()> {
        StdPublicationOperations.copy_to_new_and_sync(source, destination)?;
        if !self.paused.swap(true, Ordering::AcqRel) {
            self.entered
                .send(())
                .map_err(|_| io::ErrorKind::BrokenPipe)?;
            self.resume
                .lock()
                .map_err(|_| io::ErrorKind::Other)?
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| io::ErrorKind::TimedOut)?;
        }
        Ok(())
    }

    fn remove_file_if_exists(&self, path: &Path) -> io::Result<()> {
        StdPublicationOperations.remove_file_if_exists(path)
    }
}
