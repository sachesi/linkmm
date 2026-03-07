use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gio;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::Game;

// ── Archive extensions we recognise as mod archives ──────────────────────────

const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "rar", "7z", "tar", "gz", "bz2", "xz"];

// ── Public entry-point ────────────────────────────────────────────────────────

/// Build the Downloads page.
///
/// Shows a list of archive files that the user has placed (or that a future
/// downloader will place) in the configured downloads directory.  Provides
/// actions to hide already-installed archives and to clean the cache.
pub fn build_downloads_page(
    _game: Option<&Game>,
    config: Rc<RefCell<AppConfig>>,
) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let title_widget = adw::WindowTitle::new("Downloads", "");
    header.set_title_widget(Some(&title_widget));

    // "Hide Installed" toggle
    let hide_btn = gtk4::ToggleButton::new();
    hide_btn.set_icon_name("view-filter-symbolic");
    hide_btn.set_tooltip_text(Some("Hide Installed"));
    header.pack_end(&hide_btn);

    // "Clean Cache" button
    let clean_btn = gtk4::Button::new();
    clean_btn.set_icon_name("user-trash-symbolic");
    clean_btn.set_tooltip_text(Some("Clean Cache"));
    header.pack_end(&clean_btn);

    toolbar_view.add_top_bar(&header);

    // Scrollable content container
    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.set_vexpand(true);

    let hide_installed = Rc::new(RefCell::new(false));

    refresh_content(&content, &config, *hide_installed.borrow());

    toolbar_view.set_content(Some(&content));

    // Wire "Hide Installed" toggle
    {
        let content_c = content.clone();
        let config_c = Rc::clone(&config);
        let hide_c = Rc::clone(&hide_installed);
        hide_btn.connect_toggled(move |btn| {
            *hide_c.borrow_mut() = btn.is_active();
            refresh_content(&content_c, &config_c, *hide_c.borrow());
        });
    }

    // Wire "Clean Cache" button
    {
        let content_c = content.clone();
        let config_c = Rc::clone(&config);
        let hide_c = Rc::clone(&hide_installed);
        clean_btn.connect_clicked(move |btn| {
            show_clean_cache_dialog(btn, &config_c, &content_c, &hide_c);
        });
    }

    toolbar_view.upcast()
}

// ── Content rendering ─────────────────────────────────────────────────────────

fn refresh_content(container: &gtk4::Box, config: &Rc<RefCell<AppConfig>>, hide_installed: bool) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let downloads_dir = config.borrow().downloads_dir();

    // Ensure the directory exists so the page is immediately useful
    if !downloads_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&downloads_dir) {
            log::warn!("Could not create downloads directory: {e}");
        }
    }

    if !downloads_dir.exists() {
        let status = adw::StatusPage::builder()
            .title("No Downloads Directory")
            .description(
                "Set your app data directory in Preferences so that downloads have a place to live.",
            )
            .icon_name("folder-download-symbolic")
            .build();
        status.set_vexpand(true);
        container.append(&status);
        return;
    }

    let installed_archives: Vec<String> = config.borrow().installed_archives.clone();
    let entries = scan_downloads(&downloads_dir);

    let visible: Vec<&DownloadEntry> = if hide_installed {
        entries
            .iter()
            .filter(|e| !installed_archives.contains(&e.name))
            .collect()
    } else {
        entries.iter().collect()
    };

    if visible.is_empty() {
        let description = if hide_installed && !entries.is_empty() {
            "All downloaded mods have been installed.\nToggle \u{201c}Hide Installed\u{201d} to show them."
        } else {
            "Downloaded mod archives will appear here once you place them in the downloads folder."
        };
        let status = adw::StatusPage::builder()
            .title("No Downloads")
            .description(description)
            .icon_name("folder-download-symbolic")
            .build();

        // "Open Folder" button so the user can drop files in easily
        let open_btn = gtk4::Button::with_label("Open Downloads Folder");
        open_btn.add_css_class("pill");
        open_btn.set_halign(gtk4::Align::Center);
        let dir_clone = downloads_dir.clone();
        open_btn.connect_clicked(move |_| {
            open_in_file_manager(&dir_clone);
        });
        status.set_child(Some(&open_btn));
        status.set_vexpand(true);
        container.append(&status);
        return;
    }

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::None);

    for entry in &visible {
        let row = build_entry_row(entry, &installed_archives, config, container, hide_installed);
        list_box.append(&row);
    }

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(900);
    clamp.set_child(Some(&list_box));
    clamp.set_margin_top(12);
    clamp.set_margin_bottom(12);
    clamp.set_margin_start(12);
    clamp.set_margin_end(12);

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
    scrolled.set_child(Some(&clamp));

    container.append(&scrolled);
}

// ── Row builder ───────────────────────────────────────────────────────────────

fn build_entry_row(
    entry: &DownloadEntry,
    installed_archives: &[String],
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: bool,
) -> adw::ActionRow {
    let is_installed = installed_archives.contains(&entry.name);

    let row = adw::ActionRow::builder()
        .title(&entry.name)
        .subtitle(&format_size(entry.size_bytes))
        .build();

    // "Installed" badge
    if is_installed {
        let badge = gtk4::Label::new(Some("Installed"));
        badge.add_css_class("success");
        badge.add_css_class("caption");
        badge.set_valign(gtk4::Align::Center);
        row.add_suffix(&badge);
    }

    // Delete / remove archive button
    let delete_btn = gtk4::Button::new();
    delete_btn.set_icon_name("user-trash-symbolic");
    delete_btn.set_tooltip_text(Some("Remove archive"));
    delete_btn.set_valign(gtk4::Align::Center);
    delete_btn.add_css_class("flat");
    delete_btn.add_css_class("destructive-action");

    let path_c = entry.path.clone();
    let name_c = entry.name.clone();
    let config_c = Rc::clone(config);
    let container_c = container.clone();
    delete_btn.connect_clicked(move |_| {
        if let Err(e) = std::fs::remove_file(&path_c) {
            log::error!("Failed to remove archive \"{}\": {e}", name_c);
        } else {
            let mut cfg = config_c.borrow_mut();
            cfg.installed_archives.retain(|a| a != &name_c);
            cfg.save();
            drop(cfg);
            refresh_content(&container_c, &config_c, hide_installed);
        }
    });

    row.add_suffix(&delete_btn);
    row
}

// ── Clean cache dialog ────────────────────────────────────────────────────────

fn show_clean_cache_dialog(
    anchor: &gtk4::Button,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: &Rc<RefCell<bool>>,
) {
    let dialog = adw::AlertDialog::new(
        Some("Clean Download Cache?"),
        Some(
            "All downloaded archive files will be permanently deleted.\n\
             Installed mods in your library will not be affected.",
        ),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("clean", "Clean Cache");
    dialog.set_response_appearance("clean", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let config_c = Rc::clone(config);
    let container_c = container.clone();
    let hide_c = Rc::clone(hide_installed);
    dialog.connect_response(None, move |_, response| {
        if response == "clean" {
            delete_all_archives(&config_c);
            refresh_content(&container_c, &config_c, *hide_c.borrow());
        }
    });

    // Walk up to find a parent Window
    let parent = anchor
        .root()
        .and_then(|r| r.downcast::<gtk4::Window>().ok());
    dialog.present(parent.as_ref());
}

fn delete_all_archives(config: &Rc<RefCell<AppConfig>>) {
    let downloads_dir = config.borrow().downloads_dir();
    if let Ok(entries) = std::fs::read_dir(&downloads_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
                .unwrap_or_default();
            if ARCHIVE_EXTENSIONS.contains(&ext.as_str()) {
                if let Err(e) = std::fs::remove_file(&path) {
                    log::error!("Failed to delete {:?}: {e}", path);
                }
            }
        }
    }
    // Clear the installed-archives tracking list as well
    let mut cfg = config.borrow_mut();
    cfg.installed_archives.clear();
    cfg.save();
}

// ── Filesystem helpers ────────────────────────────────────────────────────────

#[derive(Clone)]
struct DownloadEntry {
    name: String,
    path: PathBuf,
    size_bytes: u64,
}

fn scan_downloads(dir: &Path) -> Vec<DownloadEntry> {
    let mut entries = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(dir) {
        for item in read_dir.flatten() {
            let path = item.path();
            if !path.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
                .unwrap_or_default();
            if !ARCHIVE_EXTENSIONS.contains(&ext.as_str()) {
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            entries.push(DownloadEntry {
                name,
                path,
                size_bytes,
            });
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1_024;
    const MB: u64 = 1_024 * KB;
    const GB: u64 = 1_024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn open_in_file_manager(path: &Path) {
    let file = gio::File::for_path(path);
    let uri = file.uri();
    if let Err(e) =
        gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>)
    {
        log::error!("Failed to open folder in file manager: {e}");
    }
}
