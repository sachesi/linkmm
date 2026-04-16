use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use gio;
use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::download_state;
use crate::core::games::Game;
use crate::core::installer::{
    ExtractedArchive, FomodConfig, InstallStrategy, detect_strategy_from_extracted,
    install_mod_from_archive_with_nexus, install_mod_from_extracted, parse_fomod_from_extracted,
    read_images_from_extracted,
};
use crate::ui::app_state::update_global_state;
use crate::ui::toast::show_toast as show_app_toast;

// ── Archive extensions ────────────────────────────────────────────────────────

/// All archive file types the Downloads page can clean from cache.
const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "rar", "7z", "tar", "gz", "bz2", "xz"];
/// Archive types that are currently installable by the app.
const INSTALLABLE_ARCHIVE_EXTENSIONS: &[&str] = &["zip", "rar", "7z", "tar", "gz", "bz2", "xz"];
const DOWNLOAD_PROGRESS_POLL_INTERVAL_MS: u64 = 200;
const STATUS_POPUP_HIDE_DELAY_MS: u64 = 900;

// ── Install status context ────────────────────────────────────────────────────

/// Widgets shared across the install flow for showing a progress popup and
/// graying out the downloads UI while an install is in progress.
#[derive(Clone)]
pub(crate) struct InstallStatusCtx {
    revealer: gtk4::Revealer,
    label: gtk4::Label,
    progress: gtk4::ProgressBar,
    // header bar controls to disable during install
    search_entry: gtk4::SearchEntry,
    hide_btn: gtk4::ToggleButton,
    clean_btn: gtk4::Button,
    refresh_btn: gtk4::Button,
    // main list (grayed while busy)
    list_container: gtk4::Box,
    /// `true` while a mod archive is being extracted/installed so the download
    /// progress poll timer does not trigger a disruptive list rebuild.
    is_installing: Rc<RefCell<bool>>,
    /// Progress bars for each in-progress download, keyed by download ID.
    /// Populated on every full refresh so the poll timer can update bars
    /// in-place without rebuilding the whole list on every progress tick.
    active_progress_bars: Rc<RefCell<HashMap<u64, gtk4::ProgressBar>>>,
    /// Shared cancellation flag.  Set to `true` by the cancel button; read
    /// by the extraction progress callback to abort the blocking task.
    /// Reset to `false` at the start of every new install session.
    cancel_flag: Arc<AtomicBool>,
    /// The "Cancel" button shown inside the status popup during extraction.
    cancel_btn: gtk4::Button,
}

// ── Public entry-point ────────────────────────────────────────────────────────

/// Build the Downloads page.
///
/// Shows a list of archive files in the configured downloads directory.
/// Archives arrive here either by manual placement or via NXM link handling
/// from the browser.  Provides actions to install, hide already-installed
/// archives, and clean the cache.
pub fn build_downloads_page(game: Option<&Game>, config: Rc<RefCell<AppConfig>>) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let title_widget = adw::WindowTitle::new("Downloads", "");
    header.set_title_widget(Some(&title_widget));

    let hide_btn = gtk4::ToggleButton::new();
    hide_btn.set_icon_name("view-filter-symbolic");
    hide_btn.set_tooltip_text(Some("Hide Installed"));
    header.pack_end(&hide_btn);

    let clean_btn = gtk4::Button::new();
    clean_btn.set_icon_name("user-trash-symbolic");
    clean_btn.set_tooltip_text(Some("Clean Cache"));
    header.pack_end(&clean_btn);

    let search_entry = gtk4::SearchEntry::new();
    search_entry.set_placeholder_text(Some("Search downloads"));
    search_entry.set_width_chars(24);
    header.pack_start(&search_entry);

    let refresh_btn = gtk4::Button::new();
    refresh_btn.set_icon_name("view-refresh-symbolic");
    refresh_btn.set_tooltip_text(Some("Refresh downloads list"));
    header.pack_start(&refresh_btn);

    toolbar_view.add_top_bar(&header);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.set_vexpand(true);

    // ── Install progress popup ────────────────────────────────────────────────
    let status_revealer = gtk4::Revealer::new();
    status_revealer.set_transition_type(gtk4::RevealerTransitionType::SlideDown);
    status_revealer.set_reveal_child(false);

    let status_card = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    status_card.add_css_class("card");
    status_card.set_margin_bottom(4);
    status_card.set_margin_top(4);
    status_card.set_margin_start(4);
    status_card.set_margin_end(4);

    let status_label = gtk4::Label::new(None);
    status_label.set_xalign(0.0);
    status_label.add_css_class("dim-label");
    status_label.set_margin_top(8);
    status_label.set_margin_start(8);
    status_label.set_margin_end(8);

    let status_progress = gtk4::ProgressBar::new();
    status_progress.set_show_text(true);
    status_progress.set_margin_start(8);
    status_progress.set_margin_end(8);
    status_progress.set_margin_bottom(8);

    let cancel_btn = gtk4::Button::builder()
        .label("Cancel")
        .halign(gtk4::Align::End)
        .build();
    cancel_btn.add_css_class("destructive-action");
    cancel_btn.set_margin_end(8);
    cancel_btn.set_margin_bottom(8);
    cancel_btn.set_visible(false); // only shown during extraction

    status_card.append(&status_label);
    status_card.append(&status_progress);
    status_card.append(&cancel_btn);
    status_revealer.set_child(Some(&status_card));
    content.append(&status_revealer);
    // ─────────────────────────────────────────────────────────────────────────

    let list_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    list_container.set_vexpand(true);

    content.append(&list_container);

    let cancel_flag = Arc::new(AtomicBool::new(false));

    // Wire the cancel button to set the shared flag.
    {
        let flag = Arc::clone(&cancel_flag);
        cancel_btn.connect_clicked(move |_| {
            flag.store(true, Ordering::Relaxed);
        });
    }

    let status_ctx = InstallStatusCtx {
        revealer: status_revealer,
        label: status_label,
        progress: status_progress,
        search_entry: search_entry.clone(),
        hide_btn: hide_btn.clone(),
        clean_btn: clean_btn.clone(),
        refresh_btn: refresh_btn.clone(),
        list_container: list_container.clone(),
        is_installing: Rc::new(RefCell::new(false)),
        active_progress_bars: Rc::new(RefCell::new(HashMap::new())),
        cancel_flag: Arc::clone(&cancel_flag),
        cancel_btn: cancel_btn.clone(),
    };

    let hide_installed = Rc::new(RefCell::new(false));
    let search_query = Rc::new(RefCell::new(String::new()));
    // Full progress fingerprint (includes bytes) — drives in-place bar updates.
    let active_download_fingerprint = Rc::new(RefCell::new(String::new()));
    // Identity-only fingerprint (name, no bytes) — drives full list rebuilds.
    let download_identity_fingerprint = Rc::new(RefCell::new(String::new()));
    let game_rc: Rc<Option<Game>> = Rc::new(game.cloned());

    refresh_content_with_search(
        &list_container,
        &config,
        *hide_installed.borrow(),
        &game_rc,
        &search_query.borrow(),
        &status_ctx,
    );
    toolbar_view.set_content(Some(&content));

    {
        let container_c = list_container.clone();
        let config_c = Rc::clone(&config);
        let hide_c = Rc::clone(&hide_installed);
        let search_c = Rc::clone(&search_query);
        let game_c = Rc::clone(&game_rc);
        let ctx_c = status_ctx.clone();
        hide_btn.connect_toggled(move |btn| {
            *hide_c.borrow_mut() = btn.is_active();
            refresh_content_with_search(
                &container_c,
                &config_c,
                *hide_c.borrow(),
                &game_c,
                &search_c.borrow(),
                &ctx_c,
            );
        });
    }

    {
        let container_c = list_container.clone();
        let config_c = Rc::clone(&config);
        let hide_c = Rc::clone(&hide_installed);
        let search_c = Rc::clone(&search_query);
        let game_c = Rc::clone(&game_rc);
        let ctx_c = status_ctx.clone();
        clean_btn.connect_clicked(move |btn| {
            show_clean_cache_dialog(
                btn,
                &config_c,
                &container_c,
                &hide_c,
                &search_c,
                &game_c,
                &ctx_c,
            );
        });
    }

    {
        let container_c = list_container.clone();
        let config_c = Rc::clone(&config);
        let hide_c = Rc::clone(&hide_installed);
        let search_c = Rc::clone(&search_query);
        let game_c = Rc::clone(&game_rc);
        let ctx_c = status_ctx.clone();
        refresh_btn.connect_clicked(move |_| {
            refresh_content_with_search(
                &container_c,
                &config_c,
                *hide_c.borrow(),
                &game_c,
                &search_c.borrow(),
                &ctx_c,
            );
        });
    }

    {
        let container_c = list_container.clone();
        let config_c = Rc::clone(&config);
        let hide_c = Rc::clone(&hide_installed);
        let search_c = Rc::clone(&search_query);
        let game_c = Rc::clone(&game_rc);
        let ctx_c = status_ctx.clone();
        search_entry.connect_search_changed(move |entry| {
            *search_c.borrow_mut() = entry.text().to_string();
            refresh_content_with_search(
                &container_c,
                &config_c,
                *hide_c.borrow(),
                &game_c,
                &search_c.borrow(),
                &ctx_c,
            );
        });
    }

    {
        let container_c = list_container.clone();
        let config_c = Rc::clone(&config);
        let hide_c = Rc::clone(&hide_installed);
        let search_c = Rc::clone(&search_query);
        let game_c = Rc::clone(&game_rc);
        let progress_fingerprint_c = Rc::clone(&active_download_fingerprint);
        let identity_fingerprint_c = Rc::clone(&download_identity_fingerprint);
        let ctx_c = status_ctx.clone();
        gtk4::glib::timeout_add_local(
            std::time::Duration::from_millis(DOWNLOAD_PROGRESS_POLL_INTERVAL_MS),
            move || {
                // Skip list rebuilds while a mod is being installed to avoid
                // disrupting the extraction in progress.
                if *ctx_c.is_installing.borrow() {
                    return gtk4::glib::ControlFlow::Continue;
                }
                let active = download_state::all_active();
                let mut identity_key = String::new();
                let mut progress_key = String::new();
                for (id, entry) in &active {
                    identity_key.push_str(&format!("{id}:{}|", entry.file_name));
                    progress_key.push_str(&format!(
                        "{id}:{}:{}:{}|",
                        entry.file_name, entry.downloaded, entry.total
                    ));
                }
                if let Some(g) = game_c.as_ref().as_ref() {
                    let cfg = config_c.borrow();
                    if let Some(gs) = cfg.game_settings.get(&g.id) {
                        for arc in &gs.installed_archives {
                            identity_key.push_str(&format!("inst:{}|", arc));
                        }
                    }
                }
                if *identity_fingerprint_c.borrow() != identity_key {
                    // A download started or finished → full rebuild of both sections.
                    *identity_fingerprint_c.borrow_mut() = identity_key;
                    *progress_fingerprint_c.borrow_mut() = progress_key;
                    refresh_content_with_search(
                        &container_c,
                        &config_c,
                        *hide_c.borrow(),
                        &game_c,
                        &search_c.borrow(),
                        &ctx_c,
                    );
                } else if *progress_fingerprint_c.borrow() != progress_key {
                    // Only bytes changed → update existing progress bars in-place
                    // without touching the static file rows or their buttons.
                    *progress_fingerprint_c.borrow_mut() = progress_key;
                    let bars = ctx_c.active_progress_bars.borrow();
                    for (id, entry) in &active {
                        if let Some(pb) = bars.get(id) {
                            if entry.total > 0 {
                                let fraction =
                                    (entry.downloaded as f64 / entry.total as f64).clamp(0.0, 1.0);
                                pb.set_fraction(fraction);
                                pb.set_text(Some(&format!("{:.0}%", fraction * 100.0)));
                            } else {
                                pb.pulse();
                                pb.set_text(Some(&format_size(entry.downloaded)));
                            }
                        }
                    }
                }
                gtk4::glib::ControlFlow::Continue
            },
        );
    }

    toolbar_view.upcast()
}

// ── Content rendering ─────────────────────────────────────────────────────────

fn find_existing_downloads_view(
    container: &gtk4::Box,
) -> Option<(gtk4::Stack, gtk4::ListBox, gtk4::ScrolledWindow)> {
    let stack = container
        .first_child()
        .and_then(|child| child.downcast::<gtk4::Stack>().ok())?;
    let list_page = stack.child_by_name("list")?;
    let scrolled = list_page.downcast::<gtk4::ScrolledWindow>().ok()?;
    let clamp = scrolled.child()?.downcast::<adw::Clamp>().ok()?;
    let list_box = clamp.child()?.downcast::<gtk4::ListBox>().ok()?;
    Some((stack, list_box, scrolled))
}

fn ensure_downloads_view(
    container: &gtk4::Box,
) -> (gtk4::Stack, gtk4::ListBox, gtk4::ScrolledWindow) {
    if let Some(existing) = find_existing_downloads_view(container) {
        return existing;
    }

    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::None);

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

    let stack = gtk4::Stack::new();
    stack.set_vexpand(true);
    stack.add_named(&scrolled, Some("list"));
    container.append(&stack);
    (stack, list_box, scrolled)
}

fn set_downloads_status(stack: &gtk4::Stack, status: &adw::StatusPage) {
    if let Some(existing) = stack.child_by_name("status") {
        stack.remove(&existing);
    }
    stack.add_named(status, Some("status"));
    stack.set_visible_child_name("status");
}

fn clamped_scroll_target(previous_value: f64, upper: f64, page_size: f64) -> f64 {
    let max_value = (upper - page_size).max(0.0);
    previous_value.clamp(0.0, max_value)
}

fn refresh_content_with_search(
    container: &gtk4::Box,
    config: &Rc<RefCell<AppConfig>>,
    hide_installed: bool,
    game: &Rc<Option<Game>>,
    search_query: &str,
    status_ctx: &InstallStatusCtx,
) {
    let (stack, list_box, scrolled) = ensure_downloads_view(container);
    let previous_scroll = {
        let adj = scrolled.vadjustment();
        (adj.value(), adj.upper(), adj.page_size())
    };
    // Discard stale bar references; they will be repopulated below.
    status_ctx.active_progress_bars.borrow_mut().clear();

    let downloads_dir = config
        .borrow()
        .downloads_dir(game.as_ref().as_ref().map(|g| g.id.as_str()));

    if !downloads_dir.exists() {
        let _ = std::fs::create_dir_all(&downloads_dir);
    }

    if !downloads_dir.exists() {
        let status = adw::StatusPage::builder()
            .title("No Downloads Directory")
            .description("Set your app data directory in Preferences so that downloads have a place to live.")
            .icon_name("folder-download-symbolic")
            .build();
        status.set_vexpand(true);
        set_downloads_status(&stack, &status);
        return;
    }

    let installed_archives: Vec<String> = match game.as_ref().as_ref() {
        Some(g) => config
            .borrow()
            .game_settings_ref(&g.id)
            .map(|gs| gs.installed_archives.clone())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let entries = scan_downloads(&downloads_dir);
    let active_downloads = download_state::all_active();

    let visible: Vec<&DownloadEntry> = entries
        .iter()
        .filter(|e| {
            (!hide_installed || !installed_archives.contains(&e.name))
                && matches_query(&e.name, search_query)
        })
        .collect();

    if visible.is_empty() && active_downloads.is_empty() {
        let description = if !search_query.trim().is_empty() {
            "No downloads match your search."
        } else if hide_installed && !entries.is_empty() {
            "All downloaded mods have been installed.\nToggle \u{201c}Hide Installed\u{201d} to show them."
        } else {
            "Downloaded mod archives will appear here.\nClick \u{201c}Download with manager\u{201d} on nexusmods.com to start a download,\nor place archive files in the downloads folder manually."
        };
        let status = adw::StatusPage::builder()
            .title("No Downloads")
            .description(description)
            .icon_name("folder-download-symbolic")
            .build();
        let open_btn = gtk4::Button::with_label("Open Downloads Folder");
        open_btn.add_css_class("pill");
        open_btn.set_halign(gtk4::Align::Center);
        let dir_clone = downloads_dir.clone();
        open_btn.connect_clicked(move |_| {
            open_in_file_manager(&dir_clone);
        });
        status.set_child(Some(&open_btn));
        status.set_vexpand(true);
        set_downloads_status(&stack, &status);
        return;
    }

    if let Some(status) = stack.child_by_name("status") {
        stack.remove(&status);
    }
    stack.set_visible_child_name("list");
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    {
        let mut bars = status_ctx.active_progress_bars.borrow_mut();
        for (download_id, active) in active_downloads.iter().rev() {
            let (row, progress_bar) = build_active_download_row(*download_id, active);
            bars.insert(*download_id, progress_bar);
            list_box.append(&row);
        }
    }
    for entry in &visible {
        let row = build_entry_row(
            entry,
            &installed_archives,
            config,
            container,
            hide_installed,
            search_query,
            game,
            status_ctx,
        );
        list_box.append(&row);
    }
    let adj = scrolled.vadjustment();
    adj.set_value(clamped_scroll_target(
        previous_scroll.0,
        adj.upper(),
        adj.page_size(),
    ));
}

fn build_active_download_row(
    download_id: u64,
    active: &download_state::ActiveDownload,
) -> (adw::ActionRow, gtk4::ProgressBar) {
    let row = adw::ActionRow::builder()
        .title(&active.file_name)
        .subtitle("Downloading…")
        .build();
    row.add_css_class("accent");

    let progress = gtk4::ProgressBar::new();
    progress.set_show_text(true);
    progress.set_hexpand(true);
    if active.total > 0 {
        let fraction = (active.downloaded as f64 / active.total as f64).clamp(0.0, 1.0);
        progress.set_fraction(fraction);
        progress.set_text(Some(&format!("{:.0}%", fraction * 100.0)));
    } else {
        progress.pulse();
        progress.set_text(Some(&format_size(active.downloaded)));
    }
    row.add_suffix(&progress);

    let cancel_btn = gtk4::Button::new();
    cancel_btn.set_icon_name("process-stop-symbolic");
    cancel_btn.set_tooltip_text(Some("Discard download"));
    cancel_btn.add_css_class("flat");
    cancel_btn.add_css_class("destructive-action");
    cancel_btn.set_valign(gtk4::Align::Center);
    cancel_btn.connect_clicked(move |_| {
        download_state::request_cancel(download_id);
    });
    row.add_suffix(&cancel_btn);

    (row, progress)
}

// ── Row builder ───────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn build_entry_row(
    entry: &DownloadEntry,
    installed_archives: &[String],
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: bool,
    search_query: &str,
    game: &Rc<Option<Game>>,
    status_ctx: &InstallStatusCtx,
) -> adw::ActionRow {
    let is_installed = installed_archives.contains(&entry.name);
    let row = adw::ActionRow::builder()
        .title(&entry.name)
        .subtitle(format_size(entry.size_bytes))
        .build();

    if is_installed {
        let badge = gtk4::Label::new(Some("Installed"));
        badge.add_css_class("success");
        badge.add_css_class("caption");
        badge.set_valign(gtk4::Align::Center);
        row.add_suffix(&badge);
    }

    // Install button (when a game is selected)
    if !is_installed && let Some(ref g) = **game {
        let ext = entry
            .path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        if !INSTALLABLE_ARCHIVE_EXTENSIONS.contains(&ext.as_str()) {
            return row;
        }
        let install_btn = gtk4::Button::with_label("Install");
        install_btn.set_tooltip_text(Some("Install mod"));
        install_btn.set_valign(gtk4::Align::Center);
        install_btn.add_css_class("suggested-action");

        let path_c = entry.path.clone();
        let name_c = entry.name.clone();
        let game_c = g.clone();
        let config_c = Rc::clone(config);
        let container_c = container.clone();
        let game_rc_c = Rc::clone(game);
        let hide_installed_c = hide_installed;
        let search_query_c = search_query.to_string();
        let ctx_c = status_ctx.clone();
        install_btn.connect_clicked(move |btn| {
            show_install_dialog(
                btn,
                &path_c,
                &name_c,
                &game_c,
                &config_c,
                &container_c,
                hide_installed_c,
                &search_query_c,
                &game_rc_c,
                &ctx_c,
            );
        });
        row.add_suffix(&install_btn);
    }

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
    let game_c = Rc::clone(game);
    let search_query_c = search_query.to_string();
    let ctx_c = status_ctx.clone();
    delete_btn.connect_clicked(move |_| {
        if let Err(e) = std::fs::remove_file(&path_c) {
            log::error!("Failed to remove archive \"{}\": {e}", name_c);
        } else {
            let mut cfg = config_c.borrow_mut();
            if let Some(g) = game_c.as_ref().as_ref() {
                cfg.game_settings_mut(&g.id)
                    .installed_archives
                    .retain(|a| a != &name_c);
            }
            cfg.save();
            drop(cfg);
            refresh_content_with_search(
                &container_c,
                &config_c,
                hide_installed,
                &game_c,
                &search_query_c,
                &ctx_c,
            );
        }
    });
    row.add_suffix(&delete_btn);
    row
}

// ── Install dialog ────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn show_install_dialog(
    anchor: &gtk4::Button,
    archive_path: &Path,
    archive_name: &str,
    game: &Game,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: bool,
    search_query: &str,
    game_rc: &Rc<Option<Game>>,
    status_ctx: &InstallStatusCtx,
) {
    // Show the status bar immediately.  The archive is fully extracted ONCE
    // into a hidden temporary folder inside the game's mods directory (same
    // filesystem as the final mod destination).  All subsequent analysis
    // (strategy detection, FOMOD XML parsing, image loading) runs instantly
    // from the filesystem.  At install time the extracted files are *moved*
    // (renamed) rather than copied, so large archives install instantly.
    set_downloads_busy(status_ctx, true);
    status_ctx.revealer.set_reveal_child(true);
    status_ctx
        .label
        .set_text("Extracting archive to mods directory… Navigation is locked until install finishes or is canceled.");
    status_ctx.progress.set_fraction(0.0);
    status_ctx.progress.set_text(Some("Extracting…"));

    // Reset the cancel flag and show the cancel button for the extraction phase.
    status_ctx.cancel_flag.store(false, Ordering::Relaxed);
    status_ctx.cancel_btn.set_visible(true);

    // Clone Send-able data for the blocking task (no Rc / GTK widget).
    let ap = archive_path.to_path_buf();
    let archive_name_bg = archive_name.to_string();
    // Resolve the mods directory now (on the main thread) so the blocking
    // task can pass it to `from_archive_in` without touching `game` across
    // the thread boundary.
    let mods_dir_bg = game.mods_dir();

    // Shared progress counters written by the extraction thread and read by
    // the GTK timer on the main thread.  Both use `Relaxed` ordering — we
    // only need eventual visibility, not strict synchronisation.
    let extract_bytes_done = Arc::new(AtomicU64::new(0));
    let extract_bytes_total = Arc::new(AtomicU64::new(0));
    let extract_bytes_done_bg = Arc::clone(&extract_bytes_done);
    let extract_bytes_total_bg = Arc::clone(&extract_bytes_total);
    let cancel_for_progress = Arc::clone(&status_ctx.cancel_flag);

    // Drive the progress bar from the main thread at ~10 Hz.
    let progress_bar = status_ctx.progress.clone();
    let bytes_done_ui = Arc::clone(&extract_bytes_done);
    let bytes_total_ui = Arc::clone(&extract_bytes_total);
    let pulse_id =
        gtk4::glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            let done = bytes_done_ui.load(Ordering::Relaxed);
            let total = bytes_total_ui.load(Ordering::Relaxed);
            if total > 0 {
                let fraction = (done as f64 / total as f64).min(1.0);
                progress_bar.set_fraction(fraction);
                let done_mb = done as f64 / 1_048_576.0;
                let total_mb = total as f64 / 1_048_576.0;
                progress_bar.set_text(Some(&format!("{done_mb:.0} / {total_mb:.0} MB")));
            } else {
                progress_bar.pulse();
            }
            gtk4::glib::ControlFlow::Continue
        });

    // Clone GTK-side handles for the completion closure (main-thread-only).
    let anchor_c = anchor.clone();
    let archive_path_c = archive_path.to_path_buf();
    let archive_name_s = archive_name.to_string();
    let game_c = game.clone();
    let config_c = Rc::clone(config);
    let container_c = container.clone();
    let game_rc_c = Rc::clone(game_rc);
    let search_query_s = search_query.to_string();
    let status_ctx_c = status_ctx.clone();

    glib::spawn_future_local(async move {
        #[allow(clippy::type_complexity)]
        let result = gio::spawn_blocking(
            move || -> Result<
                (
                    Arc<ExtractedArchive>,
                    InstallStrategy,
                    Option<FomodConfig>,
                    HashMap<String, Vec<u8>>,
                ),
                String,
            > {
                // ── Single-pass extraction (on-disk, same filesystem) ─────
                // Extract the entire archive once into a hidden temp folder
                // inside the game's mods directory.  Because the temp dir and
                // the final mod directory share the same filesystem, the
                // install step can *move* (rename) files instead of copying
                // them — making even multi-gigabyte archives install instantly.
                let extracted = Arc::new(
                    ExtractedArchive::from_archive_in(&ap, &mods_dir_bg, &|done, total| {
                        extract_bytes_done_bg.store(done, Ordering::Relaxed);
                        extract_bytes_total_bg.store(total, Ordering::Relaxed);
                        // Return false to abort extraction when user cancels.
                        !cancel_for_progress.load(Ordering::Relaxed)
                    })?
                );

                // ── Strategy detection (instant — filesystem only) ────────
                let strategy = detect_strategy_from_extracted(&extracted);

                // ── FOMOD parsing + image loading (instant) ───────────────
                let mut images_data = HashMap::new();
                let fomod_config = if let InstallStrategy::Fomod(_) = &strategy {
                    match parse_fomod_from_extracted(&extracted, &archive_name_bg) {
                        Ok(cfg) => {
                            let image_paths: Vec<&str> = cfg
                                .steps
                                .iter()
                                .flat_map(|s| &s.groups)
                                .flat_map(|g| &g.plugins)
                                .filter_map(|p| p.image_path.as_deref())
                                .collect();
                            if !image_paths.is_empty() {
                                images_data =
                                    read_images_from_extracted(&extracted, &image_paths);
                            }
                            Some(cfg)
                        }
                        Err(e) => {
                            log::debug!("No FOMOD config in archive, using standard install: {e}");
                            None
                        }
                    }
                } else {
                    None
                };

                Ok((extracted, strategy, fomod_config, images_data))
            },
        )
        .await;

        // Stop pulsing the progress bar.
        pulse_id.remove();

        match result {
            Ok(Ok((extracted, _strategy, fomod_config_opt, images_data))) => {
                status_ctx_c.cancel_btn.set_visible(false);
                set_downloads_busy(&status_ctx_c, false);
                status_ctx_c.revealer.set_reveal_child(false);

                if let Some(fomod_config) = fomod_config_opt {
                    let parent = anchor_c
                        .root()
                        .and_then(|r| r.downcast::<gtk4::Window>().ok());
                    let extracted_c = Arc::clone(&extracted);
                    let ap = archive_path_c.clone();
                    let an = archive_name_s.clone();
                    let gc = game_c.clone();
                    let cc = Rc::clone(&config_c);
                    let cont = container_c.clone();
                    let hide = hide_installed;
                    let search = search_query_s.clone();
                    let grc = Rc::clone(&game_rc_c);
                    let ctx = Some(status_ctx_c.clone());
                    crate::ui::setup_wizard::show_fomod_wizard(
                        parent.as_ref(),
                        &archive_name_s,
                        &fomod_config,
                        images_data,
                        move |strategy| {
                            do_install_from_extracted(
                                Arc::clone(&extracted_c),
                                &ap,
                                &an,
                                &gc,
                                &cc,
                                &cont,
                                hide,
                                &search,
                                &strategy,
                                &grc,
                                ctx.as_ref(),
                            );
                        },
                    );
                } else {
                    let parent = anchor_c
                        .root()
                        .and_then(|r| r.downcast::<gtk4::Window>().ok());
                    show_strategy_picker(
                        parent.as_ref(),
                        extracted,
                        &archive_path_c,
                        &archive_name_s,
                        &game_c,
                        &config_c,
                        &container_c,
                        hide_installed,
                        &search_query_s,
                        &game_rc_c,
                        &status_ctx_c,
                    );
                }
            }
            Ok(Err(ref e)) if e == "Cancelled by user" => {
                log::info!("Archive extraction cancelled by user");
                status_ctx_c.cancel_btn.set_visible(false);
                set_downloads_busy(&status_ctx_c, false);
                status_ctx_c.revealer.set_reveal_child(false);
            }
            Ok(Err(e)) => {
                log::error!("Failed to extract archive: {e}");
                status_ctx_c.cancel_btn.set_visible(false);
                set_downloads_busy(&status_ctx_c, false);
                hide_status_popup_later(status_ctx_c.revealer.clone());
                show_app_toast(&format!("Error: {e}"));
            }
            Err(_) => {
                // Blocking task panicked.
                log::error!("Archive extraction task terminated unexpectedly");
                status_ctx_c.cancel_btn.set_visible(false);
                set_downloads_busy(&status_ctx_c, false);
                hide_status_popup_later(status_ctx_c.revealer.clone());
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn show_strategy_picker(
    parent: Option<&gtk4::Window>,
    extracted: Arc<ExtractedArchive>,
    archive_path: &Path,
    archive_name: &str,
    game: &Game,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: bool,
    search_query: &str,
    game_rc: &Rc<Option<Game>>,
    status_ctx: &InstallStatusCtx,
) {
    let dialog = adw::AlertDialog::builder()
        .heading("Install Mod")
        .body(format!(
            "Install \"{archive_name}\" into the game's Data folder?"
        ))
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("data", "Install");
    dialog.set_response_appearance("data", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("data"));
    dialog.set_close_response("cancel");

    let ap = archive_path.to_path_buf();
    let an = archive_name.to_string();
    let gc = game.clone();
    let cc = Rc::clone(config);
    let cont = container.clone();
    let hide = hide_installed;
    let search = search_query.to_string();
    let grc = Rc::clone(game_rc);
    let ctx = status_ctx.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "data" {
            do_install_from_extracted(
                Arc::clone(&extracted),
                &ap,
                &an,
                &gc,
                &cc,
                &cont,
                hide,
                &search,
                &InstallStrategy::Data,
                &grc,
                Some(&ctx),
            );
        }
    });
    dialog.present(parent);
}

/// Install a mod from a pre-extracted archive.
///
/// The `ExtractedArchive` temp directory already contains all files; the
/// background thread only needs to copy them into the game's mod folder,
/// avoiding any re-decompression of the original archive.
#[allow(clippy::too_many_arguments)]
fn do_install_from_extracted(
    extracted: Arc<ExtractedArchive>,
    archive_path: &Path,
    archive_name: &str,
    game: &Game,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: bool,
    search_query: &str,
    strategy: &InstallStrategy,
    game_rc: &Rc<Option<Game>>,
    status_ctx: Option<&InstallStatusCtx>,
) {
    let mod_name = Path::new(archive_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| archive_name.to_string());

    if let Some(ctx) = status_ctx {
        set_downloads_busy(ctx, true);
        // Install phase is essentially instant (file moves/renames);
        // hide the cancel button immediately so it is not shown.
        ctx.cancel_btn.set_visible(false);
        ctx.revealer.set_reveal_child(true);
        ctx.label.set_text(&format!("Installing \"{}\"…", mod_name));
        ctx.progress.set_fraction(0.0);
        ctx.progress.set_text(Some("Installing…"));
    }

    let nexus_id = read_nxm_mod_id_for_archive(archive_path, game);

    // Clone all Send data needed by the background thread.
    let game_clone = game.clone();
    let mod_name_bg = mod_name.clone();
    let strategy_clone = strategy.clone();
    let archive_name_bg = archive_name.to_string();

    let (tx, rx) = std::sync::mpsc::channel::<Result<crate::core::mods::Mod, String>>();

    // The Arc<ExtractedArchive> is moved into the thread; the temp directory
    // persists until this thread (and any other Arc clones) are dropped.
    std::thread::spawn(move || {
        let result = install_mod_from_extracted(
            &extracted,
            &game_clone,
            &mod_name_bg,
            &strategy_clone,
            nexus_id,
            Some(&archive_name_bg),
            &|| {},
        );
        let _ = tx.send(result);
        // `extracted` (Arc) is dropped here — if this is the last clone the
        // temp directory is cleaned up automatically.
    });

    let config_c = Rc::clone(config);
    let container_c = container.clone();
    let game_rc_c = Rc::clone(game_rc);
    let archive_name_s = archive_name.to_string();
    let mod_name_s = mod_name.clone();
    let search_query_s = search_query.to_string();
    let status_ctx_clone = status_ctx.cloned();

    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        match rx.try_recv() {
            Ok(result) => {
                on_install_complete(
                    result,
                    &archive_name_s,
                    &mod_name_s,
                    &config_c,
                    hide_installed,
                    &search_query_s,
                    &game_rc_c,
                    status_ctx_clone.as_ref(),
                    &container_c,
                );
                gtk4::glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if let Some(ctx) = &status_ctx_clone {
                    ctx.progress.pulse();
                }
                gtk4::glib::ControlFlow::Continue
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::error!("Install thread terminated unexpectedly for \"{mod_name_s}\"");
                on_install_complete(
                    Err("Install thread terminated unexpectedly".to_string()),
                    &archive_name_s,
                    &mod_name_s,
                    &config_c,
                    hide_installed,
                    &search_query_s,
                    &game_rc_c,
                    status_ctx_clone.as_ref(),
                    &container_c,
                );
                gtk4::glib::ControlFlow::Break
            }
        }
    });
}

/// Legacy install path: re-reads the archive from disk.
///
/// Still used by NXM link handler and the library reinstall flow which do not
/// go through the extract-first pipeline (yet).
#[allow(clippy::too_many_arguments)]
pub(crate) fn do_install(
    archive_path: &Path,
    archive_name: &str,
    game: &Game,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: bool,
    search_query: &str,
    strategy: &InstallStrategy,
    game_rc: &Rc<Option<Game>>,
    status_ctx: Option<&InstallStatusCtx>,
) {
    let mod_name = Path::new(archive_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| archive_name.to_string());

    if let Some(ctx) = status_ctx {
        set_downloads_busy(ctx, true);
        ctx.revealer.set_reveal_child(true);
        ctx.label.set_text(&format!("Installing \"{}\"…", mod_name));
        ctx.progress.set_fraction(0.0);
        ctx.progress.set_text(Some("Extracting archive…"));
    }

    let nexus_id = read_nxm_mod_id_for_archive(archive_path, game);

    let ap = archive_path.to_path_buf();
    let game_clone = game.clone();
    let mod_name_bg = mod_name.clone();
    let strategy_clone = strategy.clone();

    let (tx, rx) = std::sync::mpsc::channel::<Result<crate::core::mods::Mod, String>>();

    std::thread::spawn(move || {
        let result = install_mod_from_archive_with_nexus(
            &ap,
            &game_clone,
            &mod_name_bg,
            &strategy_clone,
            nexus_id,
        );
        let _ = tx.send(result);
    });

    let config_c = Rc::clone(config);
    let container_c = container.clone();
    let game_rc_c = Rc::clone(game_rc);
    let archive_name_s = archive_name.to_string();
    let mod_name_s = mod_name.clone();
    let search_query_s = search_query.to_string();
    let status_ctx_clone = status_ctx.cloned();

    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        match rx.try_recv() {
            Ok(result) => {
                on_install_complete(
                    result,
                    &archive_name_s,
                    &mod_name_s,
                    &config_c,
                    hide_installed,
                    &search_query_s,
                    &game_rc_c,
                    status_ctx_clone.as_ref(),
                    &container_c,
                );
                gtk4::glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if let Some(ctx) = &status_ctx_clone {
                    ctx.progress.pulse();
                }
                gtk4::glib::ControlFlow::Continue
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::error!("Install thread terminated unexpectedly for \"{mod_name_s}\"");
                on_install_complete(
                    Err("Install thread terminated unexpectedly".to_string()),
                    &archive_name_s,
                    &mod_name_s,
                    &config_c,
                    hide_installed,
                    &search_query_s,
                    &game_rc_c,
                    status_ctx_clone.as_ref(),
                    &container_c,
                );
                gtk4::glib::ControlFlow::Break
            }
        }
    });
}

/// Handle the install result on the GTK main thread once the background thread
/// has finished.  Updates the status popup, config, and refreshes the list.
#[allow(clippy::too_many_arguments)]
fn on_install_complete(
    result: Result<crate::core::mods::Mod, String>,
    archive_name: &str,
    mod_name: &str,
    config: &Rc<RefCell<AppConfig>>,
    hide_installed: bool,
    search_query: &str,
    game_rc: &Rc<Option<Game>>,
    status_ctx: Option<&InstallStatusCtx>,
    _container: &gtk4::Box,
) {
    match result {
        Ok(_) => {
            let mut cfg = config.borrow_mut();
            if let Some(g) = game_rc.as_ref().as_ref() {
                let gs = cfg.game_settings_mut(&g.id);
                if !gs.installed_archives.contains(&archive_name.to_string()) {
                    gs.installed_archives.push(archive_name.to_string());
                }
            }
            cfg.save();
            drop(cfg);
            log::info!("Installed mod \"{mod_name}\" from \"{archive_name}\"");
            let msg = format!("Installed: {mod_name}");
            if let Some(ctx) = status_ctx {
                ctx.label.set_text(&msg);
                ctx.progress.set_fraction(1.0);
                ctx.progress.set_text(Some("100%"));
                set_downloads_busy(ctx, false);
                hide_status_popup_later(ctx.revealer.clone());
                show_app_toast(&msg);
                refresh_content_with_search(
                    &ctx.list_container,
                    config,
                    hide_installed,
                    game_rc,
                    search_query,
                    ctx,
                );
            } else {
                show_app_toast(&msg);
            }
        }
        Err(e) => {
            log::error!("Failed to install mod \"{mod_name}\": {e}");
            let msg = format!("Install failed: {e}");
            if let Some(ctx) = status_ctx {
                ctx.label.set_text(&msg);
                ctx.progress.set_fraction(0.0);
                ctx.progress.set_text(Some("Error"));
                set_downloads_busy(ctx, false);
                hide_status_popup_later(ctx.revealer.clone());
                show_app_toast(&msg);
            } else {
                show_app_toast(&msg);
            }
        }
    }
}

// ── Clean cache dialog ────────────────────────────────────────────────────────

fn show_clean_cache_dialog(
    anchor: &gtk4::Button,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: &Rc<RefCell<bool>>,
    search_query: &Rc<RefCell<String>>,
    game: &Rc<Option<Game>>,
    status_ctx: &InstallStatusCtx,
) {
    let dialog = adw::AlertDialog::new(
        Some("Clean Download Cache?"),
        Some(
            "All downloaded archive files will be permanently deleted.\nInstalled mods in your library will not be affected.",
        ),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("clean", "Clean Cache");
    dialog.set_response_appearance("clean", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let cc = Rc::clone(config);
    let cont = container.clone();
    let hc = Rc::clone(hide_installed);
    let search_c = Rc::clone(search_query);
    let gc = Rc::clone(game);
    let ctx_c = status_ctx.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "clean" {
            delete_all_archives(&cc, &gc);
            refresh_content_with_search(&cont, &cc, *hc.borrow(), &gc, &search_c.borrow(), &ctx_c);
        }
    });
    let parent = anchor
        .root()
        .and_then(|r| r.downcast::<gtk4::Window>().ok());
    dialog.present(parent.as_ref());
}

fn delete_all_archives(config: &Rc<RefCell<AppConfig>>, game: &Rc<Option<Game>>) {
    let downloads_dir = config
        .borrow()
        .downloads_dir(game.as_ref().as_ref().map(|g| g.id.as_str()));
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
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    let mut cfg = config.borrow_mut();
    if let Some(g) = game.as_ref().as_ref() {
        cfg.game_settings_mut(&g.id).installed_archives.clear();
    }
    cfg.save();
}

// ── Filesystem helpers ────────────────────────────────────────────────────────

#[derive(Clone)]
struct DownloadEntry {
    name: String,
    path: PathBuf,
    size_bytes: u64,
    modified: std::time::SystemTime,
}

fn scan_downloads(dir: &Path) -> Vec<DownloadEntry> {
    let mut entries = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for item in rd.flatten() {
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
            let meta = std::fs::metadata(&path).ok();
            let size_bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = meta
                .and_then(|m| m.modified().ok())
                .unwrap_or(std::time::UNIX_EPOCH);
            entries.push(DownloadEntry {
                name,
                path,
                size_bytes,
                modified,
            });
        }
    }
    entries.sort_by(|a, b| b.modified.cmp(&a.modified));
    entries
}

fn matches_query(value: &str, query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return true;
    }
    value.to_lowercase().contains(&trimmed.to_lowercase())
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

fn nxm_metadata_path_for_archive(archive_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.nxm.json", archive_path.to_string_lossy()))
}

fn read_nxm_mod_id_for_archive(archive_path: &Path, game: &Game) -> Option<u32> {
    // Try new consolidated format first (~/.config/linkmm/<game_id>/nxm_metadata.json)
    if let Some(file_name) = archive_path.file_name().and_then(|n| n.to_str())
        && let Some(id) = game.read_nxm_mod_id(file_name)
    {
        return Some(id);
    }
    // Fallback: try old per-archive sidecar file (.nxm.json alongside archive)
    let metadata_path = nxm_metadata_path_for_archive(archive_path);
    let contents = std::fs::read_to_string(metadata_path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&contents).ok()?;
    let game_domain = value.get("game_domain")?.as_str()?;
    if game_domain != game.kind.nexus_game_id() {
        return None;
    }
    value
        .get("mod_id")?
        .as_u64()
        .and_then(|id| u32::try_from(id).ok())
}

fn open_in_file_manager(path: &Path) {
    let file = gio::File::for_path(path);
    let uri = file.uri();
    let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
}

/// Disable all interactive Downloads controls while an install is in progress,
/// visually indicating to the user that the UI is locked.
fn set_downloads_busy(ctx: &InstallStatusCtx, busy: bool) {
    *ctx.is_installing.borrow_mut() = busy;
    let sensitive = !busy;
    ctx.search_entry.set_sensitive(sensitive);
    ctx.hide_btn.set_sensitive(sensitive);
    ctx.clean_btn.set_sensitive(sensitive);
    ctx.refresh_btn.set_sensitive(sensitive);
    ctx.list_container.set_sensitive(sensitive);
    update_global_state(|state| {
        state.install_active = busy;
        if busy {
            state.message =
                Some("Install active: destructive actions are temporarily disabled.".to_string());
        } else if !state.deploy_active && !state.runtime_session_active {
            state.message = None;
        }
    });
    // Cancel button is managed separately (shown only during extraction).
}

/// Schedule the status popup to slide back up after a short delay.
fn hide_status_popup_later(revealer: gtk4::Revealer) {
    gtk4::glib::timeout_add_local_once(
        std::time::Duration::from_millis(STATUS_POPUP_HIDE_DELAY_MS),
        move || {
            revealer.set_reveal_child(false);
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::games::{Game, GameKind};
    use std::path::PathBuf;

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static CTR: AtomicU32 = AtomicU32::new(0);
        let n = CTR.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("linkmm_downloads_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn matches_query_is_case_insensitive_and_trim_aware() {
        assert!(matches_query("MyCoolMod.zip", ""));
        assert!(matches_query("MyCoolMod.zip", "  cool "));
        assert!(!matches_query("MyCoolMod.zip", "armor"));
    }

    #[test]
    fn clamped_scroll_target_clamps_within_adjustment_bounds() {
        assert_eq!(clamped_scroll_target(25.0, 200.0, 100.0), 25.0);
        assert_eq!(clamped_scroll_target(250.0, 200.0, 100.0), 100.0);
        assert_eq!(clamped_scroll_target(-5.0, 200.0, 100.0), 0.0);
        assert_eq!(clamped_scroll_target(10.0, 50.0, 100.0), 0.0);
    }

    #[test]
    fn installable_archive_extensions_match_supported_archive_types() {
        assert!(INSTALLABLE_ARCHIVE_EXTENSIONS.contains(&"zip"));
        assert!(INSTALLABLE_ARCHIVE_EXTENSIONS.contains(&"rar"));
        assert!(INSTALLABLE_ARCHIVE_EXTENSIONS.contains(&"7z"));
        assert!(INSTALLABLE_ARCHIVE_EXTENSIONS.contains(&"tar"));
        assert!(INSTALLABLE_ARCHIVE_EXTENSIONS.contains(&"gz"));
        assert!(INSTALLABLE_ARCHIVE_EXTENSIONS.contains(&"bz2"));
        assert!(INSTALLABLE_ARCHIVE_EXTENSIONS.contains(&"xz"));
    }

    #[test]
    fn scan_downloads_includes_non_zip_archives() {
        let tmp = tempdir();
        std::fs::write(tmp.join("mod-a.zip"), b"zip").unwrap();
        std::fs::write(tmp.join("mod-b.rar"), b"rar").unwrap();
        std::fs::write(tmp.join("mod-c.7z"), b"7z").unwrap();
        std::fs::write(tmp.join("notes.txt"), b"txt").unwrap();

        let entries = scan_downloads(&tmp);
        let names: Vec<String> = entries.into_iter().map(|e| e.name).collect();

        assert!(names.contains(&"mod-a.zip".to_string()));
        assert!(names.contains(&"mod-b.rar".to_string()));
        assert!(names.contains(&"mod-c.7z".to_string()));
        assert!(!names.contains(&"notes.txt".to_string()));
    }

    #[test]
    fn read_nxm_mod_id_for_archive_returns_id_from_consolidated_file() {
        let tmp = tempdir();
        let archive_path = tmp.join("SomeMod.zip");
        std::fs::write(&archive_path, b"zip").unwrap();

        // Write new consolidated format into a fake config dir for the game.
        // We point the game's config_dir to a subdirectory under tmp.
        // Since game.config_dir() uses dirs::config_dir(), we test via the
        // sidecar fallback path which still works for the old format.
        let metadata_path = nxm_metadata_path_for_archive(&archive_path);
        std::fs::write(
            &metadata_path,
            r#"{"game_domain":"skyrimspecialedition","mod_id":173949}"#,
        )
        .unwrap();

        let game = Game::new_steam(GameKind::SkyrimSE, tmp.join("game_root"));
        assert_eq!(
            read_nxm_mod_id_for_archive(&archive_path, &game),
            Some(173949)
        );
    }

    #[test]
    fn read_nxm_mod_id_for_archive_returns_none_for_wrong_game_domain() {
        let tmp = tempdir();
        let archive_path = tmp.join("SomeMod.zip");
        std::fs::write(&archive_path, b"zip").unwrap();
        let metadata_path = nxm_metadata_path_for_archive(&archive_path);
        std::fs::write(&metadata_path, r#"{"game_domain":"fallout4","mod_id":999}"#).unwrap();

        // Sidecar says fallout4 but game is SkyrimSE → mismatch → None
        let game = Game::new_steam(GameKind::SkyrimSE, tmp.join("game_root"));
        assert_eq!(read_nxm_mod_id_for_archive(&archive_path, &game), None);
    }
}
