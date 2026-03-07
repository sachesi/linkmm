use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
pub struct ActiveDownload {
    pub file_name: String,
    pub downloaded: u64,
    pub total: u64,
}

fn active_download_cell() -> &'static Mutex<Option<ActiveDownload>> {
    static ACTIVE: OnceLock<Mutex<Option<ActiveDownload>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

pub fn set_active(file_name: String) {
    if let Ok(mut state) = active_download_cell().lock() {
        *state = Some(ActiveDownload {
            file_name,
            downloaded: 0,
            total: 0,
        });
    }
}

pub fn update_progress(downloaded: u64, total: u64) {
    if let Ok(mut state) = active_download_cell().lock() {
        if let Some(active) = state.as_mut() {
            active.downloaded = downloaded;
            active.total = total;
        }
    }
}

pub fn clear_active() {
    if let Ok(mut state) = active_download_cell().lock() {
        *state = None;
    }
}

pub fn current() -> Option<ActiveDownload> {
    active_download_cell()
        .lock()
        .ok()
        .and_then(|state| state.clone())
}
