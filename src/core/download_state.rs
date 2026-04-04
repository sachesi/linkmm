use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
pub struct ActiveDownload {
    pub file_name: String,
    pub downloaded: u64,
    pub total: u64,
    /// Set to `true` when the user requests cancelling this download.
    pub cancel_requested: bool,
}

fn active_download_cell() -> &'static Mutex<HashMap<u64, ActiveDownload>> {
    static ACTIVE: OnceLock<Mutex<HashMap<u64, ActiveDownload>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_download_id() -> u64 {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn set_active(file_name: String) -> u64 {
    let id = next_download_id();
    if let Ok(mut state) = active_download_cell().lock() {
        state.insert(
            id,
            ActiveDownload {
                file_name,
                downloaded: 0,
                total: 0,
                cancel_requested: false,
            },
        );
    }
    id
}

pub fn update_progress(download_id: u64, downloaded: u64, total: u64) {
    if let Ok(mut state) = active_download_cell().lock()
        && let Some(active) = state.get_mut(&download_id)
    {
        active.downloaded = downloaded;
        active.total = total;
    }
}

pub fn clear_active(download_id: u64) {
    if let Ok(mut state) = active_download_cell().lock() {
        state.remove(&download_id);
    }
}

pub fn request_cancel(download_id: u64) {
    if let Ok(mut state) = active_download_cell().lock()
        && let Some(active) = state.get_mut(&download_id)
    {
        active.cancel_requested = true;
    }
}

pub fn is_cancel_requested(download_id: u64) -> bool {
    active_download_cell()
        .lock()
        .ok()
        .and_then(|state| {
            state
                .get(&download_id)
                .map(|active| active.cancel_requested)
        })
        .unwrap_or(false)
}

pub fn all_active() -> Vec<(u64, ActiveDownload)> {
    let mut active = active_download_cell()
        .lock()
        .ok()
        .map(|state| {
            state
                .iter()
                .map(|(id, download)| (*id, download.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    active.sort_by_key(|(id, _)| *id);
    active
}

#[allow(dead_code)]
pub fn current() -> Option<ActiveDownload> {
    all_active().last().map(|(_, active)| active.clone())
}

#[cfg(test)]
fn clear_all_for_tests() {
    if let Ok(mut state) = active_download_cell().lock() {
        state.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_multiple_active_downloads_without_clobbering() {
        clear_all_for_tests();
        let first_id = set_active("first.zip".to_string());
        let second_id = set_active("second.zip".to_string());

        update_progress(first_id, 10, 100);
        update_progress(second_id, 40, 80);

        let latest = current().expect("expected an active download");
        assert_eq!(latest.file_name, "second.zip");
        assert_eq!(latest.downloaded, 40);
        assert_eq!(latest.total, 80);

        clear_active(second_id);
        let remaining = current().expect("expected first download to remain active");
        assert_eq!(remaining.file_name, "first.zip");
        assert_eq!(remaining.downloaded, 10);
        assert_eq!(remaining.total, 100);

        clear_active(first_id);
        assert!(current().is_none());
    }

    #[test]
    fn cancel_request_is_tracked_per_download() {
        clear_all_for_tests();
        let id = set_active("cancel-me.zip".to_string());
        assert!(!is_cancel_requested(id));
        request_cancel(id);
        assert!(is_cancel_requested(id));
        clear_active(id);
        assert!(!is_cancel_requested(id));
    }
}
