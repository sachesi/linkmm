use std::cell::RefCell;

use gtk4::glib;
use gtk4::glib::clone::Downgrade;
use libadwaita as adw;

thread_local! {
    static ROOT_TOAST_OVERLAY: RefCell<Option<glib::WeakRef<adw::ToastOverlay>>> = const { RefCell::new(None) };
}

pub fn register_root_toast_overlay(overlay: &adw::ToastOverlay) {
    ROOT_TOAST_OVERLAY.with(|cell| {
        *cell.borrow_mut() = Some(overlay.downgrade());
    });
}

pub fn show_toast(message: &str) {
    ROOT_TOAST_OVERLAY.with(|cell| {
        if let Some(weak) = cell.borrow().as_ref()
            && let Some(overlay) = weak.upgrade()
        {
            let toast = adw::Toast::new(message);
            toast.set_timeout(4);
            overlay.add_toast(toast);
            return;
        }
        log::warn!("Toast dropped (no root overlay): {message}");
    });
}
