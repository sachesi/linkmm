use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gio;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::download_state;
use crate::core::games::Game;
use crate::core::installer::{
    DependencyOperator, FlagDependency, FomodConfig, FomodFile, FomodPlugin, GroupType,
    InstallStep, InstallStrategy, PluginDependencies, PluginType, detect_strategy,
    install_mod_from_archive_with_nexus_ticking, parse_fomod_from_archive,
    read_archive_file_bytes,
};
use crate::core::mods::ModDatabase;

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
struct InstallStatusCtx {
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

    status_card.append(&status_label);
    status_card.append(&status_progress);
    status_revealer.set_child(Some(&status_card));
    content.append(&status_revealer);
    // ─────────────────────────────────────────────────────────────────────────

    let list_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    list_container.set_vexpand(true);

    content.append(&list_container);

    let status_ctx = InstallStatusCtx {
        revealer: status_revealer,
        label: status_label,
        progress: status_progress,
        search_entry: search_entry.clone(),
        hide_btn: hide_btn.clone(),
        clean_btn: clean_btn.clone(),
        refresh_btn: refresh_btn.clone(),
        list_container: list_container.clone(),
    };

    let hide_installed = Rc::new(RefCell::new(false));
    let search_query = Rc::new(RefCell::new(String::new()));
    let active_download_fingerprint = Rc::new(RefCell::new(String::new()));
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
            show_clean_cache_dialog(btn, &config_c, &container_c, &hide_c, &search_c, &game_c, &ctx_c);
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
        let active_download_fingerprint_c = Rc::clone(&active_download_fingerprint);
        let ctx_c = status_ctx.clone();
        gtk4::glib::timeout_add_local(
            std::time::Duration::from_millis(DOWNLOAD_PROGRESS_POLL_INTERVAL_MS),
            move || {
                let active = download_state::all_active();
                let mut download_state_key = String::new();
                for (id, entry) in &active {
                    download_state_key.push_str(&format!(
                        "{id}:{}:{}:{}|",
                        entry.file_name, entry.downloaded, entry.total
                    ));
                }
                if *active_download_fingerprint_c.borrow() != download_state_key {
                    *active_download_fingerprint_c.borrow_mut() = download_state_key;
                    refresh_content_with_search(
                        &container_c,
                        &config_c,
                        *hide_c.borrow(),
                        &game_c,
                        &search_c.borrow(),
                        &ctx_c,
                    );
                }
                gtk4::glib::ControlFlow::Continue
            },
        );
    }

    toolbar_view.upcast()
}

// ── Content rendering ─────────────────────────────────────────────────────────

fn refresh_content_with_search(
    container: &gtk4::Box,
    config: &Rc<RefCell<AppConfig>>,
    hide_installed: bool,
    game: &Rc<Option<Game>>,
    search_query: &str,
    status_ctx: &InstallStatusCtx,
) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

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
        container.append(&status);
        return;
    }

    let installed_archives: Vec<String> = config.borrow().installed_archives.clone();
    let installed_mod_names: Vec<String> = match game.as_ref() {
        Some(g) => ModDatabase::load(g)
            .mods
            .into_iter()
            .map(|m| m.name.to_lowercase())
            .collect(),
        None => Vec::new(),
    };
    let entries = scan_downloads(&downloads_dir);
    let active_downloads = download_state::all_active();

    let visible: Vec<&DownloadEntry> = entries
        .iter()
        .filter(|e| {
            (!hide_installed || !entry_is_installed(e, &installed_archives, &installed_mod_names))
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
        container.append(&status);
        return;
    }

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::None);
    for (download_id, active) in active_downloads.iter().rev() {
        let row = build_active_download_row(*download_id, active);
        list_box.append(&row);
    }
    for entry in &visible {
        let row = build_entry_row(
            entry,
            &installed_archives,
            &installed_mod_names,
            config,
            container,
            hide_installed,
            search_query,
            game,
            status_ctx,
        );
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

fn build_active_download_row(download_id: u64, active: &download_state::ActiveDownload) -> adw::ActionRow {
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

    row
}

// ── Row builder ───────────────────────────────────────────────────────────────

fn build_entry_row(
    entry: &DownloadEntry,
    installed_archives: &[String],
    installed_mod_names: &[String],
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: bool,
    search_query: &str,
    game: &Rc<Option<Game>>,
    status_ctx: &InstallStatusCtx,
) -> adw::ActionRow {
    let is_installed = entry_is_installed(entry, installed_archives, installed_mod_names);
    let row = adw::ActionRow::builder()
        .title(&entry.name)
        .subtitle(&format_size(entry.size_bytes))
        .build();

    if is_installed {
        let badge = gtk4::Label::new(Some("Installed"));
        badge.add_css_class("success");
        badge.add_css_class("caption");
        badge.set_valign(gtk4::Align::Center);
        row.add_suffix(&badge);
    }

    // Install button (when a game is selected)
    if !is_installed {
        if let Some(ref g) = **game {
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
            cfg.installed_archives.retain(|a| a != &name_c);
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
    let strategy = match detect_strategy(archive_path) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Failed to detect install strategy: {e}");
            show_toast(anchor.upcast_ref(), &format!("Error: {e}"));
            return;
        }
    };

    if let InstallStrategy::Fomod(_) = &strategy {
        match parse_fomod_from_archive(archive_path) {
            Ok(fomod_config) => {
                let parent = anchor
                    .root()
                    .and_then(|r| r.downcast::<gtk4::Window>().ok());
                show_fomod_wizard(
                    parent.as_ref(),
                    archive_path,
                    archive_name,
                    game,
                    config,
                    container,
                    hide_installed,
                    search_query,
                    &fomod_config,
                    game_rc,
                    Some(status_ctx),
                );
                return;
            }
            Err(e) => {
                log::warn!("Failed to parse FOMOD config, falling back: {e}");
            }
        }
    }

    let parent = anchor
        .root()
        .and_then(|r| r.downcast::<gtk4::Window>().ok());
    show_strategy_picker(
        parent.as_ref(),
        archive_path,
        archive_name,
        game,
        config,
        container,
        hide_installed,
        search_query,
        &strategy,
        game_rc,
        status_ctx,
    );
}

fn show_strategy_picker(
    parent: Option<&gtk4::Window>,
    archive_path: &Path,
    archive_name: &str,
    game: &Game,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: bool,
    search_query: &str,
    _detected: &InstallStrategy,
    game_rc: &Rc<Option<Game>>,
    status_ctx: &InstallStatusCtx,
) {
    let dialog = adw::AlertDialog::builder()
        .heading("Install Mod")
        .body(&format!(
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
            do_install(
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

fn do_install(
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
        // Dispatch one event to let the Revealer animation frame start before
        // the blocking extraction work begins.
        gtk4::glib::MainContext::default().iteration(false);
    }

    let nexus_id = read_nxm_mod_id_for_archive(archive_path, game);

    // For non-zip archives (7z, rar, …) the underlying 7z process can take
    // several seconds to extract a large archive.  Provide a tick callback so
    // the progress bar pulses every ~50 ms and the UI stays responsive.
    // The nexus_id check is factored out once here so the tick closure doesn't
    // need to be duplicated across branches.
    let install_result = {
        let tick: Box<dyn Fn()> = if let Some(ctx) = status_ctx {
            let progress = ctx.progress.clone();
            Box::new(move || {
                progress.pulse();
                // Process one pending event so the animation frame clock can
                // advance without draining the entire event queue at once,
                // which would cause the progress animation to stutter.
                gtk4::glib::MainContext::default().iteration(false);
            })
        } else {
            Box::new(|| {})
        };
        install_mod_from_archive_with_nexus_ticking(
            archive_path,
            game,
            &mod_name,
            strategy,
            nexus_id,
            tick.as_ref(),
        )
    };
    match install_result {
        Ok(_) => {
            let mut cfg = config.borrow_mut();
            if !cfg.installed_archives.contains(&archive_name.to_string()) {
                cfg.installed_archives.push(archive_name.to_string());
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
                show_toast(ctx.list_container.upcast_ref(), &msg);
                refresh_content_with_search(
                    &ctx.list_container,
                    config,
                    hide_installed,
                    game_rc,
                    search_query,
                    ctx,
                );
            } else {
                show_toast(container.upcast_ref(), &msg);
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
                show_toast(ctx.list_container.upcast_ref(), &msg);
            } else {
                show_toast(container.upcast_ref(), &msg);
            }
        }
    }
}

// ── FOMOD wizard ──────────────────────────────────────────────────────────────

/// Selected plugin indices by `[step_index][group_index][plugin_indices]`.
type FomodSelections = Vec<Vec<Vec<usize>>>;
const GROUP_OPTIONS_MIN_WIDTH: i32 = 360;
const GROUP_PREVIEW_PANE_WIDTH: i32 = 300;
const GROUP_PREVIEW_IMAGE_WIDTH: i32 = 268;
const GROUP_PREVIEW_IMAGE_HEIGHT: i32 = 268;
const GROUP_PREVIEW_NAME_WIDTH_CHARS: i32 = 28;
const GROUP_PREVIEW_NAME_MAX_WIDTH_CHARS: i32 = 32;
const FOMOD_CARD_EDGE_MARGIN: i32 = 8;

fn collect_active_flags(
    fomod: &FomodConfig,
    selections: &FomodSelections,
    up_to_step_inclusive: usize,
) -> HashMap<String, HashSet<String>> {
    let mut flags: HashMap<String, HashSet<String>> = HashMap::new();
    for (si, step) in fomod.steps.iter().enumerate() {
        if si > up_to_step_inclusive {
            break;
        }
        if !step_is_visible_with_flags(step, &flags) {
            continue;
        }
        for (gi, group) in step.groups.iter().enumerate() {
            let Some(selected) = selections.get(si).and_then(|s| s.get(gi)) else {
                continue;
            };
            for &pi in selected {
                let Some(plugin) = group.plugins.get(pi) else {
                    continue;
                };
                for flag in &plugin.condition_flags {
                    flags
                        .entry(flag.name.clone())
                        .or_default()
                        .insert(flag.value.clone());
                }
            }
        }
    }
    flags
}

fn flag_dependency_matches(
    dep: &FlagDependency,
    active_flags: &HashMap<String, HashSet<String>>,
) -> bool {
    if let Some(values) = active_flags.get(&dep.flag) {
        values.contains(&dep.value)
    } else {
        false
    }
}

fn dependencies_match(
    dependencies: &PluginDependencies,
    active_flags: &HashMap<String, HashSet<String>>,
) -> bool {
    if dependencies.flags.is_empty() {
        return true;
    }
    match dependencies.operator {
        DependencyOperator::And => dependencies
            .flags
            .iter()
            .all(|dep| flag_dependency_matches(dep, active_flags)),
        DependencyOperator::Or => dependencies
            .flags
            .iter()
            .any(|dep| flag_dependency_matches(dep, active_flags)),
    }
}

fn step_is_visible_with_flags(
    step: &InstallStep,
    active_flags: &HashMap<String, HashSet<String>>,
) -> bool {
    let Some(visible) = &step.visible else {
        return true;
    };
    dependencies_match(visible, active_flags)
}

fn plugin_is_visible(
    plugin: &FomodPlugin,
    active_flags: &HashMap<String, HashSet<String>>,
) -> bool {
    let Some(deps) = &plugin.dependencies else {
        return true;
    };
    dependencies_match(deps, active_flags)
}

fn sanitize_step_selection(
    fomod: &FomodConfig,
    selections: &mut FomodSelections,
    step_index: usize,
) {
    let Some(step) = fomod.steps.get(step_index) else {
        return;
    };
    let active_flags = collect_active_flags(fomod, selections, step_index);
    for (gi, group) in step.groups.iter().enumerate() {
        let Some(selected) = selections.get_mut(step_index).and_then(|s| s.get_mut(gi)) else {
            continue;
        };
        let visible: Vec<usize> = group
            .plugins
            .iter()
            .enumerate()
            .filter_map(|(pi, plugin)| {
                if plugin_is_visible(plugin, &active_flags) {
                    Some(pi)
                } else {
                    None
                }
            })
            .collect();
        selected.retain(|pi| visible.contains(pi));
        match group.group_type {
            GroupType::SelectAll => {
                *selected = visible;
            }
            GroupType::SelectExactlyOne => {
                if selected.len() > 1 {
                    selected.truncate(1);
                }
                if selected.is_empty() {
                    if let Some(first) = visible.first() {
                        selected.push(*first);
                    }
                }
            }
            GroupType::SelectAtLeastOne => {
                if selected.is_empty() {
                    if let Some(first) = visible.first() {
                        selected.push(*first);
                    }
                }
            }
            GroupType::SelectAtMostOne => {
                if selected.len() > 1 {
                    selected.truncate(1);
                }
            }
            GroupType::SelectAny => {}
        }
        selected.sort();
        selected.dedup();
    }
}

fn resolve_fomod_files(fomod: &FomodConfig, selections: &FomodSelections) -> Vec<FomodFile> {
    let mut files: Vec<FomodFile> = fomod.required_files.clone();
    let mut normalized = selections.clone();
    for si in 0..fomod.steps.len() {
        sanitize_step_selection(fomod, &mut normalized, si);
    }
    for (si, step) in fomod.steps.iter().enumerate() {
        let step_flags = if si == 0 {
            HashMap::new()
        } else {
            collect_active_flags(fomod, &normalized, si - 1)
        };
        if !step_is_visible_with_flags(step, &step_flags) {
            continue;
        }
        for (gi, group) in step.groups.iter().enumerate() {
            if let Some(selected) = normalized.get(si).and_then(|s| s.get(gi)) {
                for &pi in selected {
                    if let Some(plugin) = group.plugins.get(pi) {
                        files.extend(plugin.files.iter().cloned());
                    }
                }
            }
        }
    }
    let active_flags = if fomod.steps.is_empty() {
        HashMap::new()
    } else {
        collect_active_flags(fomod, &normalized, fomod.steps.len() - 1)
    };
    for conditional in &fomod.conditional_file_installs {
        if dependencies_match(&conditional.dependencies, &active_flags) {
            files.extend(conditional.files.iter().cloned());
        }
    }
    files
}

fn load_fomod_option_image(
    archive_path: &Path,
    image_path: &str,
    cache: &Rc<RefCell<HashMap<String, gtk4::gdk::Texture>>>,
) -> Option<gtk4::gdk::Texture> {
    if let Some(texture) = cache.borrow().get(image_path).cloned() {
        return Some(texture);
    }
    let bytes = read_archive_file_bytes(archive_path, image_path).ok()?;
    let texture = gtk4::gdk::Texture::from_bytes(&gtk4::glib::Bytes::from_owned(bytes)).ok()?;
    cache
        .borrow_mut()
        .insert(image_path.to_string(), texture.clone());
    Some(texture)
}

fn show_fomod_wizard(
    parent: Option<&gtk4::Window>,
    archive_path: &Path,
    archive_name: &str,
    game: &Game,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: bool,
    search_query: &str,
    fomod: &FomodConfig,
    game_rc: &Rc<Option<Game>>,
    status_ctx: Option<&InstallStatusCtx>,
) {
    let mod_display_name = fomod
        .mod_name
        .clone()
        .unwrap_or_else(|| archive_name.to_string());

    if fomod.steps.is_empty() {
        let strategy = InstallStrategy::Fomod(fomod.required_files.clone());
        do_install(
            archive_path,
            archive_name,
            game,
            config,
            container,
            hide_installed,
            search_query,
            &strategy,
            game_rc,
            status_ctx,
        );
        return;
    }

    let dialog = adw::Window::builder()
        .title(&format!("Install: {mod_display_name}"))
        .modal(true)
        .default_width(900)
        .default_height(620)
        .build();
    if let Some(p) = parent {
        dialog.set_transient_for(Some(p));
    }

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&adw::HeaderBar::new());

    let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
    main_box.set_margin_start(24);
    main_box.set_margin_end(24);
    main_box.set_margin_top(12);
    main_box.set_margin_bottom(12);

    let selections: Rc<RefCell<FomodSelections>> = Rc::new(RefCell::new(Vec::new()));
    {
        let mut sel = selections.borrow_mut();
        for step in &fomod.steps {
            let mut step_sel = Vec::new();
            for group in &step.groups {
                let mut gs: Vec<usize> = Vec::new();
                for (idx, plugin) in group.plugins.iter().enumerate() {
                    if matches!(
                        plugin.type_descriptor,
                        PluginType::Required | PluginType::Recommended
                    ) || group.group_type == GroupType::SelectAll
                    {
                        gs.push(idx);
                    }
                }
                if gs.is_empty()
                    && matches!(
                        group.group_type,
                        GroupType::SelectExactlyOne | GroupType::SelectAtLeastOne
                    )
                    && !group.plugins.is_empty()
                {
                    gs.push(0);
                }
                gs.sort();
                gs.dedup();
                step_sel.push(gs);
            }
            sel.push(step_sel);
        }
    }

    let step_index = Rc::new(RefCell::new(0usize));
    let step_content = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    step_content.set_vexpand(true);

    let nav_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    nav_box.set_halign(gtk4::Align::End);
    nav_box.set_margin_top(12);
    let back_btn = gtk4::Button::with_label("Back");
    let next_btn = gtk4::Button::with_label("Next");
    next_btn.add_css_class("suggested-action");
    let install_btn = gtk4::Button::with_label("Install");
    install_btn.add_css_class("suggested-action");
    nav_box.append(&back_btn);
    nav_box.append(&next_btn);
    nav_box.append(&install_btn);

    let fomod_rc = Rc::new(fomod.clone());
    let step_count = fomod.steps.len();
    let image_cache: Rc<RefCell<HashMap<String, gtk4::gdk::Texture>>> =
        Rc::new(RefCell::new(HashMap::new()));

    let render_step = {
        let sc = step_content.clone();
        let fc = Rc::clone(&fomod_rc);
        let sel_c = Rc::clone(&selections);
        let img_cache = Rc::clone(&image_cache);
        let ap = archive_path.to_path_buf();
        let bb = back_btn.clone();
        let nb = next_btn.clone();
        let ib = install_btn.clone();
        Rc::new(move |idx: usize| {
            while let Some(child) = sc.first_child() {
                sc.remove(&child);
            }
            bb.set_sensitive(idx > 0);
            nb.set_visible(idx + 1 < step_count);
            ib.set_visible(idx + 1 >= step_count);
            if idx >= fc.steps.len() {
                return;
            }
            {
                let mut sel = sel_c.borrow_mut();
                sanitize_step_selection(&fc, &mut sel, idx);
            }
            let active_flags = {
                let sel = sel_c.borrow();
                collect_active_flags(&fc, &sel, idx)
            };
            let step = &fc.steps[idx];
            let title = gtk4::Label::new(Some(&step.name));
            title.add_css_class("title-2");
            title.set_halign(gtk4::Align::Start);
            title.set_margin_bottom(4);
            sc.append(&title);
            let step_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 16);
            step_row.set_hexpand(true);
            step_row.set_vexpand(true);
            let scrolled = gtk4::ScrolledWindow::new();
            scrolled.set_vexpand(true);
            scrolled.set_hexpand(true);
            scrolled.set_size_request(GROUP_OPTIONS_MIN_WIDTH, -1);
            scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
            scrolled.add_css_class("card");
            let groups_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
            groups_box.set_margin_start(12);
            groups_box.set_margin_end(12);
            groups_box.set_margin_top(12);
            groups_box.set_margin_bottom(12);
            let preview_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
            preview_box.add_css_class("card");
            preview_box.set_halign(gtk4::Align::Start);
            preview_box.set_valign(gtk4::Align::Start);
            preview_box.set_vexpand(false);
            preview_box.set_size_request(GROUP_PREVIEW_PANE_WIDTH, -1);
            preview_box.set_margin_top(FOMOD_CARD_EDGE_MARGIN);
            preview_box.set_margin_bottom(FOMOD_CARD_EDGE_MARGIN);
            preview_box.set_margin_start(FOMOD_CARD_EDGE_MARGIN);
            preview_box.set_margin_end(FOMOD_CARD_EDGE_MARGIN);
            let preview_label = gtk4::Label::new(Some("Preview"));
            preview_label.add_css_class("dim-label");
            preview_label.add_css_class("caption");
            preview_label.set_halign(gtk4::Align::Start);
            preview_label.set_margin_start(12);
            preview_label.set_margin_top(12);
            let preview_name = gtk4::Label::new(Some(""));
            preview_name.set_wrap(false);
            preview_name.set_single_line_mode(true);
            preview_name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            preview_name.set_halign(gtk4::Align::Start);
            preview_name.set_width_chars(GROUP_PREVIEW_NAME_WIDTH_CHARS);
            preview_name.set_max_width_chars(GROUP_PREVIEW_NAME_MAX_WIDTH_CHARS);
            preview_name.set_margin_start(12);
            preview_name.set_margin_end(12);
            preview_name.set_margin_bottom(12);
            let preview_picture = gtk4::Picture::new();
            preview_picture.set_content_fit(gtk4::ContentFit::Contain);
            preview_picture.set_can_shrink(true);
            preview_picture.set_hexpand(false);
            preview_picture.set_vexpand(false);
            preview_picture.set_halign(gtk4::Align::Center);
            let preview_picture_frame = gtk4::Frame::new(None);
            preview_picture_frame
                .set_size_request(GROUP_PREVIEW_IMAGE_WIDTH, GROUP_PREVIEW_IMAGE_HEIGHT);
            preview_picture_frame.set_halign(gtk4::Align::Center);
            preview_picture_frame.set_margin_start(12);
            preview_picture_frame.set_margin_end(12);
            preview_picture_frame.set_margin_bottom(8);
            preview_picture_frame.set_child(Some(&preview_picture));
            preview_box.append(&preview_label);
            preview_box.append(&preview_picture_frame);
            preview_box.append(&preview_name);
            let mut has_step_preview = false;
            let mut default_preview: Option<(gtk4::gdk::Texture, String)> = None;
            for (gi, group) in step.groups.iter().enumerate() {
                let type_desc = match group.group_type {
                    GroupType::SelectExactlyOne => "select one",
                    GroupType::SelectAtMostOne => "select at most one",
                    GroupType::SelectAtLeastOne => "select at least one",
                    GroupType::SelectAll => "all required",
                    GroupType::SelectAny => "select any",
                };
                let frame = gtk4::Frame::new(Some(&format!("{} ({type_desc})", group.name)));
                frame.add_css_class("card");
                frame.set_margin_top(FOMOD_CARD_EDGE_MARGIN);
                frame.set_margin_bottom(FOMOD_CARD_EDGE_MARGIN);
                let lb = gtk4::ListBox::new();
                lb.add_css_class("boxed-list");
                lb.set_selection_mode(gtk4::SelectionMode::None);
                lb.set_hexpand(true);
                let use_radio = matches!(
                    group.group_type,
                    GroupType::SelectExactlyOne | GroupType::SelectAtMostOne
                );
                let mut first_radio_button: Option<gtk4::CheckButton> = None;
                for (pi, plugin) in group.plugins.iter().enumerate() {
                    if !plugin_is_visible(plugin, &active_flags) {
                        continue;
                    }
                    let row = adw::ActionRow::builder().title(&plugin.name).build();
                    if let Some(ref d) = plugin.description {
                        if !d.is_empty() {
                            row.set_subtitle(d);
                        }
                    }
                    let check = gtk4::CheckButton::new();
                    if use_radio {
                        if let Some(ref first) = first_radio_button {
                            check.set_group(Some(first));
                        } else {
                            first_radio_button = Some(check.clone());
                        }
                    }
                    {
                        let sel = sel_c.borrow();
                        if let Some(gs) = sel.get(idx).and_then(|s| s.get(gi)) {
                            check.set_active(gs.contains(&pi));
                        }
                    }
                    if group.group_type == GroupType::SelectAll {
                        check.set_active(true);
                        check.set_sensitive(false);
                    }
                    let sel_cc = Rc::clone(&sel_c);
                    let is_radio = use_radio;
                    let si = idx;
                    check.connect_toggled(move |btn| {
                        let mut sel = sel_cc.borrow_mut();
                        if let Some(gs) = sel.get_mut(si).and_then(|s| s.get_mut(gi)) {
                            if btn.is_active() {
                                if is_radio {
                                    gs.clear();
                                }
                                if !gs.contains(&pi) {
                                    gs.push(pi);
                                }
                            } else {
                                gs.retain(|&x| x != pi);
                            }
                        }
                    });
                    check.set_valign(gtk4::Align::Center);
                    row.add_prefix(&check);
                    row.set_activatable_widget(Some(&check));
                    if let Some(ref image_path) = plugin.image_path {
                        if let Some(texture) = load_fomod_option_image(&ap, image_path, &img_cache)
                        {
                            has_step_preview = true;
                            if check.is_active() || default_preview.is_none() {
                                default_preview = Some((texture.clone(), plugin.name.clone()));
                            }
                            let hover_pic = preview_picture.clone();
                            let hover_name = preview_name.clone();
                            let hover_texture = texture.clone();
                            let hover_plugin_name = plugin.name.clone();
                            let row_motion = gtk4::EventControllerMotion::new();
                            row_motion.connect_enter(move |_, _, _| {
                                hover_pic.set_paintable(Some(&hover_texture));
                                hover_name.set_label(&hover_plugin_name);
                            });
                            row.add_controller(row_motion);

                            let radio_pic = preview_picture.clone();
                            let radio_name = preview_name.clone();
                            let radio_texture = texture.clone();
                            let radio_plugin_name = plugin.name.clone();
                            let check_motion = gtk4::EventControllerMotion::new();
                            check_motion.connect_enter(move |_, _, _| {
                                radio_pic.set_paintable(Some(&radio_texture));
                                radio_name.set_label(&radio_plugin_name);
                            });
                            check.add_controller(check_motion);
                        }
                    }
                    let tl = match plugin.type_descriptor {
                        PluginType::Required => Some("Required"),
                        PluginType::Recommended => Some("Recommended"),
                        PluginType::NotUsable => Some("Not Usable"),
                        PluginType::Optional => None,
                    };
                    if let Some(lt) = tl {
                        let badge = gtk4::Label::new(Some(lt));
                        badge.add_css_class("dim-label");
                        badge.add_css_class("caption");
                        badge.set_valign(gtk4::Align::Center);
                        row.add_suffix(&badge);
                    }
                    lb.append(&row);
                }
                frame.set_child(Some(&lb));
                groups_box.append(&frame);
            }
            scrolled.set_child(Some(&groups_box));
            step_row.append(&scrolled);
            if has_step_preview {
                if let Some((texture, plugin_name)) = default_preview {
                    preview_picture.set_paintable(Some(&texture));
                    preview_name.set_label(&plugin_name);
                }
            } else {
                preview_picture.set_paintable(None::<&gtk4::gdk::Texture>);
                preview_name.set_label("No preview available");
            }
            step_row.append(&preview_box);
            sc.append(&step_row);
        })
    };
    render_step(0);
    {
        let si = Rc::clone(&step_index);
        let rs = Rc::clone(&render_step);
        back_btn.connect_clicked(move |_| {
            let mut i = si.borrow_mut();
            if *i > 0 {
                *i -= 1;
                rs(*i);
            }
        });
    }
    {
        let si = Rc::clone(&step_index);
        let rs = Rc::clone(&render_step);
        next_btn.connect_clicked(move |_| {
            let mut i = si.borrow_mut();
            if *i + 1 < step_count {
                *i += 1;
                rs(*i);
            }
        });
    }
    {
        let sel_c = Rc::clone(&selections);
        let fc = Rc::clone(&fomod_rc);
        let ap = archive_path.to_path_buf();
        let an = archive_name.to_string();
        let gc = game.clone();
        let cc = Rc::clone(config);
        let cont = container.clone();
        let dlg = dialog.clone();
        let hide = hide_installed;
        let search = search_query.to_string();
        let grc = Rc::clone(game_rc);
        let ctx = status_ctx.cloned();
        install_btn.connect_clicked(move |_| {
            let files = {
                let sel = sel_c.borrow();
                resolve_fomod_files(&fc, &sel)
            };
            // Close the wizard first so the Downloads page is visible when
            // the install progress popup appears.
            dlg.destroy();
            do_install(
                &ap,
                &an,
                &gc,
                &cc,
                &cont,
                hide,
                &search,
                &InstallStrategy::Fomod(files),
                &grc,
                ctx.as_ref(),
            );
        });
    }
    main_box.append(&step_content);
    main_box.append(&nav_box);
    toolbar_view.set_content(Some(&main_box));
    dialog.set_content(Some(&toolbar_view));
    dialog.present();
}

/// Public entry point for the FOMOD wizard, callable from the Library page.
pub fn show_fomod_wizard_from_library(
    parent: Option<&gtk4::Window>,
    archive_path: &Path,
    archive_name: &str,
    game: &Game,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    fomod: &crate::core::installer::FomodConfig,
    game_rc: &Rc<Option<Game>>,
) {
    show_fomod_wizard(
        parent,
        archive_path,
        archive_name,
        game,
        config,
        container,
        false,
        "",
        fomod,
        game_rc,
        None,
    );
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

fn entry_is_installed(
    entry: &DownloadEntry,
    installed_archives: &[String],
    installed_mod_names: &[String],
) -> bool {
    if installed_archives.contains(&entry.name) {
        return true;
    }
    let mod_name = Path::new(&entry.name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    !mod_name.is_empty() && installed_mod_names.iter().any(|m| m == &mod_name)
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

fn show_toast(widget: &gtk4::Widget, message: &str) {
    let mut ancestor: Option<gtk4::Widget> = Some(widget.clone());
    while let Some(current) = ancestor {
        if let Ok(overlay) = current.clone().downcast::<adw::ToastOverlay>() {
            let toast = adw::Toast::new(message);
            toast.set_timeout(3);
            overlay.add_toast(toast);
            return;
        }
        ancestor = current.parent();
    }
    log::info!("{message}");
}

/// Disable all interactive Downloads controls while an install is in progress,
/// visually indicating to the user that the UI is locked.
fn set_downloads_busy(ctx: &InstallStatusCtx, busy: bool) {
    let sensitive = !busy;
    ctx.search_entry.set_sensitive(sensitive);
    ctx.hide_btn.set_sensitive(sensitive);
    ctx.clean_btn.set_sensitive(sensitive);
    ctx.refresh_btn.set_sensitive(sensitive);
    ctx.list_container.set_sensitive(sensitive);
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
    use crate::core::installer::{
        ConditionFlag, DependencyOperator, FlagDependency, FomodPlugin, InstallStep,
        PluginDependencies, PluginGroup,
    };
    use std::path::PathBuf;

    fn test_plugin(
        name: &str,
        file_source: &str,
        condition_flags: Vec<ConditionFlag>,
        dependencies: Option<PluginDependencies>,
    ) -> FomodPlugin {
        FomodPlugin {
            name: name.to_string(),
            description: None,
            image_path: None,
            files: vec![FomodFile {
                source: file_source.to_string(),
                destination: "Data".to_string(),
                priority: 0,
            }],
            type_descriptor: PluginType::Optional,
            condition_flags,
            dependencies,
        }
    }

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static CTR: AtomicU32 = AtomicU32::new(0);
        let n = CTR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("linkmm_downloads_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_fomod_files_filters_plus_minus_variants_by_dependency_flags() {
        let mut config = FomodConfig {
            mod_name: Some("Test".to_string()),
            required_files: Vec::new(),
            steps: Vec::new(),
            conditional_file_installs: Vec::new(),
        };
        config.steps.push(InstallStep {
            name: "Flags".to_string(),
            visible: None,
            groups: vec![PluginGroup {
                name: "Feature".to_string(),
                group_type: GroupType::SelectExactlyOne,
                plugins: vec![
                    test_plugin(
                        "Enable +",
                        "flags-plus.txt",
                        vec![ConditionFlag {
                            name: "FeaturePack".to_string(),
                            value: "+".to_string(),
                        }],
                        None,
                    ),
                    test_plugin(
                        "Enable -",
                        "flags-minus.txt",
                        vec![ConditionFlag {
                            name: "FeaturePack".to_string(),
                            value: "-".to_string(),
                        }],
                        None,
                    ),
                ],
            }],
        });
        config.steps.push(InstallStep {
            name: "Variant".to_string(),
            visible: None,
            groups: vec![PluginGroup {
                name: "Pick".to_string(),
                group_type: GroupType::SelectAny,
                plugins: vec![
                    test_plugin(
                        "Plus variant",
                        "plus-variant.txt",
                        Vec::new(),
                        Some(PluginDependencies {
                            operator: DependencyOperator::And,
                            flags: vec![FlagDependency {
                                flag: "FeaturePack".to_string(),
                                value: "+".to_string(),
                            }],
                        }),
                    ),
                    test_plugin(
                        "Minus variant",
                        "minus-variant.txt",
                        Vec::new(),
                        Some(PluginDependencies {
                            operator: DependencyOperator::And,
                            flags: vec![FlagDependency {
                                flag: "FeaturePack".to_string(),
                                value: "-".to_string(),
                            }],
                        }),
                    ),
                ],
            }],
        });

        // Simulate stale UI state selecting both variants in step 2 while step 1
        // has "+" selected.
        let selections = vec![vec![vec![0]], vec![vec![0, 1]]];
        let files = resolve_fomod_files(&config, &selections);
        let sources: Vec<String> = files.into_iter().map(|f| f.source).collect();
        assert!(sources.contains(&"plus-variant.txt".to_string()));
        assert!(!sources.contains(&"minus-variant.txt".to_string()));
    }

    #[test]
    fn sanitize_step_selection_respects_at_most_and_exactly_one_rules() {
        let config = FomodConfig {
            mod_name: Some("Test".to_string()),
            required_files: Vec::new(),
            conditional_file_installs: Vec::new(),
            steps: vec![InstallStep {
                name: "Main".to_string(),
                visible: None,
                groups: vec![
                    PluginGroup {
                        name: "Exactly one".to_string(),
                        group_type: GroupType::SelectExactlyOne,
                        plugins: vec![
                            test_plugin("A", "a.txt", Vec::new(), None),
                            test_plugin("B", "b.txt", Vec::new(), None),
                        ],
                    },
                    PluginGroup {
                        name: "At most one".to_string(),
                        group_type: GroupType::SelectAtMostOne,
                        plugins: vec![
                            test_plugin("C", "c.txt", Vec::new(), None),
                            test_plugin("D", "d.txt", Vec::new(), None),
                        ],
                    },
                ],
            }],
        };
        let mut selections = vec![vec![vec![0, 1], vec![0, 1]]];
        sanitize_step_selection(&config, &mut selections, 0);
        assert_eq!(selections[0][0], vec![0]);
        assert_eq!(selections[0][1], vec![0]);

        let mut empty_at_most_one = vec![vec![vec![1], vec![]]];
        sanitize_step_selection(&config, &mut empty_at_most_one, 0);
        assert_eq!(empty_at_most_one[0][1], Vec::<usize>::new());
    }

    #[test]
    fn resolve_fomod_files_applies_step_visibility_and_conditional_files() {
        let config = FomodConfig {
            mod_name: Some("Test".to_string()),
            required_files: Vec::new(),
            steps: vec![
                InstallStep {
                    name: "Flags".to_string(),
                    visible: None,
                    groups: vec![PluginGroup {
                        name: "Feature".to_string(),
                        group_type: GroupType::SelectExactlyOne,
                        plugins: vec![
                            test_plugin(
                                "Enable Underwear",
                                "underwear-on.txt",
                                vec![ConditionFlag {
                                    name: "bUnderwear".to_string(),
                                    value: "On".to_string(),
                                }],
                                None,
                            ),
                            test_plugin("Disable Underwear", "underwear-off.txt", Vec::new(), None),
                        ],
                    }],
                },
                InstallStep {
                    name: "Underwear Options".to_string(),
                    visible: Some(PluginDependencies {
                        operator: DependencyOperator::And,
                        flags: vec![FlagDependency {
                            flag: "bUnderwear".to_string(),
                            value: "On".to_string(),
                        }],
                    }),
                    groups: vec![PluginGroup {
                        name: "Color".to_string(),
                        group_type: GroupType::SelectExactlyOne,
                        plugins: vec![test_plugin(
                            "Black",
                            "underwear-black.txt",
                            Vec::new(),
                            None,
                        )],
                    }],
                },
            ],
            conditional_file_installs: vec![crate::core::installer::ConditionalFileInstall {
                dependencies: PluginDependencies {
                    operator: DependencyOperator::And,
                    flags: vec![FlagDependency {
                        flag: "bUnderwear".to_string(),
                        value: "On".to_string(),
                    }],
                },
                files: vec![FomodFile {
                    source: "conditional-underwear.txt".to_string(),
                    destination: "Data".to_string(),
                    priority: 0,
                }],
            }],
        };

        // Step 1 picks "Disable Underwear". Step 2 has stale selection but should
        // not contribute because the step is hidden.
        let hidden_step_selections = vec![vec![vec![1]], vec![vec![0]]];
        let hidden_files = resolve_fomod_files(&config, &hidden_step_selections);
        let hidden_sources: Vec<String> = hidden_files.into_iter().map(|f| f.source).collect();
        assert!(!hidden_sources.contains(&"underwear-black.txt".to_string()));
        assert!(!hidden_sources.contains(&"conditional-underwear.txt".to_string()));

        // Step 1 picks "Enable Underwear", so step 2 and conditional files apply.
        let visible_step_selections = vec![vec![vec![0]], vec![vec![0]]];
        let visible_files = resolve_fomod_files(&config, &visible_step_selections);
        let visible_sources: Vec<String> = visible_files.into_iter().map(|f| f.source).collect();
        assert!(visible_sources.contains(&"underwear-black.txt".to_string()));
        assert!(visible_sources.contains(&"conditional-underwear.txt".to_string()));
    }

    #[test]
    fn matches_query_is_case_insensitive_and_trim_aware() {
        assert!(matches_query("MyCoolMod.zip", ""));
        assert!(matches_query("MyCoolMod.zip", "  cool "));
        assert!(!matches_query("MyCoolMod.zip", "armor"));
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
    fn read_nxm_mod_id_for_archive_returns_id_for_matching_game_domain() {
        let tmp = tempdir();
        let archive_path = tmp.join("SomeMod.zip");
        std::fs::write(&archive_path, b"zip").unwrap();
        let metadata_path = nxm_metadata_path_for_archive(&archive_path);
        std::fs::write(
            &metadata_path,
            r#"{"game_domain":"skyrimspecialedition","mod_id":173949}"#,
        )
        .unwrap();

        let game = Game::new(GameKind::SkyrimSE, tmp.join("game_root"));
        assert_eq!(read_nxm_mod_id_for_archive(&archive_path, &game), Some(173949));
    }
}
