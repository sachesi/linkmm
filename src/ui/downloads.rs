use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;

use gio;
use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::download;
use crate::core::games::Game;
use crate::core::installer::{
    self, detect_strategy, install_mod_from_archive, parse_fomod_from_zip, FomodConfig, FomodFile,
    GroupType, InstallStrategy, PluginType,
};
use crate::core::nexus::NexusClient;

// ── Archive extensions we recognise as mod archives ──────────────────────────

const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "rar", "7z", "tar", "gz", "bz2", "xz"];

// ── View mode ─────────────────────────────────────────────────────────────────

/// Which sub-view the Downloads page is currently showing.
#[derive(Clone, Debug, PartialEq)]
enum ViewMode {
    /// Local archive list (default).
    Local,
    /// Nexus mod browser (trending / latest / search results).
    NexusBrowse,
    /// File list for a specific Nexus mod.
    NexusModFiles {
        mod_id: u64,
        mod_name: String,
    },
}

// ── Public entry-point ────────────────────────────────────────────────────────

/// Build the Downloads page.
///
/// Shows a list of archive files that the user has placed (or that a future
/// downloader will place) in the configured downloads directory.  Provides
/// actions to hide already-installed archives and to clean the cache.
///
/// Also provides a Nexus Mods browser when an API key is configured.
pub fn build_downloads_page(
    game: Option<&Game>,
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

    // "Browse Nexus" toggle
    let nexus_btn = gtk4::ToggleButton::new();
    nexus_btn.set_icon_name("web-browser-symbolic");
    nexus_btn.set_tooltip_text(Some("Browse Nexus Mods"));
    header.pack_start(&nexus_btn);

    toolbar_view.add_top_bar(&header);

    // Scrollable content container
    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.set_vexpand(true);

    let hide_installed = Rc::new(RefCell::new(false));
    let view_mode = Rc::new(RefCell::new(ViewMode::Local));
    let game_rc: Rc<Option<Game>> = Rc::new(game.cloned());

    refresh_content(
        &content,
        &config,
        *hide_installed.borrow(),
        &view_mode.borrow(),
        &game_rc,
    );

    toolbar_view.set_content(Some(&content));

    // Wire "Hide Installed" toggle
    {
        let content_c = content.clone();
        let config_c = Rc::clone(&config);
        let hide_c = Rc::clone(&hide_installed);
        let view_mode_c = Rc::clone(&view_mode);
        let game_c = Rc::clone(&game_rc);
        hide_btn.connect_toggled(move |btn| {
            *hide_c.borrow_mut() = btn.is_active();
            refresh_content(
                &content_c,
                &config_c,
                *hide_c.borrow(),
                &view_mode_c.borrow(),
                &game_c,
            );
        });
    }

    // Wire "Clean Cache" button
    {
        let content_c = content.clone();
        let config_c = Rc::clone(&config);
        let hide_c = Rc::clone(&hide_installed);
        let view_mode_c = Rc::clone(&view_mode);
        let game_c = Rc::clone(&game_rc);
        clean_btn.connect_clicked(move |btn| {
            show_clean_cache_dialog(btn, &config_c, &content_c, &hide_c, &view_mode_c, &game_c);
        });
    }

    // Wire "Browse Nexus" toggle
    {
        let content_c = content.clone();
        let config_c = Rc::clone(&config);
        let hide_c = Rc::clone(&hide_installed);
        let view_mode_c = Rc::clone(&view_mode);
        let game_c = Rc::clone(&game_rc);
        nexus_btn.connect_toggled(move |btn| {
            if btn.is_active() {
                *view_mode_c.borrow_mut() = ViewMode::NexusBrowse;
            } else {
                *view_mode_c.borrow_mut() = ViewMode::Local;
            }
            refresh_content(
                &content_c,
                &config_c,
                *hide_c.borrow(),
                &view_mode_c.borrow(),
                &game_c,
            );
        });
    }

    toolbar_view.upcast()
}

// ── Content rendering ─────────────────────────────────────────────────────────

fn refresh_content(
    container: &gtk4::Box,
    config: &Rc<RefCell<AppConfig>>,
    hide_installed: bool,
    view_mode: &ViewMode,
    game: &Rc<Option<Game>>,
) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    match view_mode {
        ViewMode::Local => {
            render_local_downloads(container, config, hide_installed, game);
        }
        ViewMode::NexusBrowse => {
            render_nexus_browse(container, config, game);
        }
        ViewMode::NexusModFiles { mod_id, mod_name } => {
            render_nexus_mod_files(container, config, game, *mod_id, mod_name);
        }
    }
}

// ── Local downloads view ──────────────────────────────────────────────────────

fn render_local_downloads(
    container: &gtk4::Box,
    config: &Rc<RefCell<AppConfig>>,
    hide_installed: bool,
    game: &Rc<Option<Game>>,
) {
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
        let row = build_entry_row(entry, &installed_archives, config, container, hide_installed, game);
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
    game: &Rc<Option<Game>>,
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

    // Install button (only for zip archives when a game is selected)
    if !is_installed {
        if let Some(ref g) = **game {
            let ext = entry
                .path
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            if ext == "zip" {
                let install_btn = gtk4::Button::new();
                install_btn.set_icon_name("emblem-system-symbolic");
                install_btn.set_tooltip_text(Some("Install mod"));
                install_btn.set_valign(gtk4::Align::Center);
                install_btn.add_css_class("flat");
                install_btn.add_css_class("suggested-action");

                let path_c = entry.path.clone();
                let name_c = entry.name.clone();
                let game_c = g.clone();
                let config_c = Rc::clone(config);
                let container_c = container.clone();
                install_btn.connect_clicked(move |btn| {
                    show_install_dialog(
                        btn,
                        &path_c,
                        &name_c,
                        &game_c,
                        &config_c,
                        &container_c,
                    );
                });

                row.add_suffix(&install_btn);
            }
        }
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
    let game_c = Rc::clone(game);
    delete_btn.connect_clicked(move |_| {
        if let Err(e) = std::fs::remove_file(&path_c) {
            log::error!("Failed to remove archive \"{}\": {e}", name_c);
        } else {
            let mut cfg = config_c.borrow_mut();
            cfg.installed_archives.retain(|a| a != &name_c);
            cfg.save();
            drop(cfg);
            render_local_downloads(&container_c, &config_c, hide_installed, &game_c);
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
) {
    let strategy = match detect_strategy(archive_path) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Failed to detect install strategy: {e}");
            show_toast(anchor.upcast_ref(), &format!("Error: {e}"));
            return;
        }
    };

    // For FOMOD mods, show the FOMOD wizard
    if let InstallStrategy::Fomod(_) = &strategy {
        match parse_fomod_from_zip(archive_path) {
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
                    &fomod_config,
                );
                return;
            }
            Err(e) => {
                log::warn!("Failed to parse FOMOD config, falling back: {e}");
                // Fall through to strategy picker
            }
        }
    }

    // For non-FOMOD mods, show a strategy picker dialog
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
        &strategy,
    );
}

fn show_strategy_picker(
    parent: Option<&gtk4::Window>,
    archive_path: &Path,
    archive_name: &str,
    game: &Game,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    detected: &InstallStrategy,
) {
    let dialog = adw::AlertDialog::builder()
        .heading("Install Mod")
        .body(&format!(
            "Choose how to install \"{archive_name}\".\n\n\
             • Root: Extracts to the game root folder\n\
             • Data: Extracts into the Data subfolder\n\n\
             Detected strategy: {}",
            match detected {
                InstallStrategy::Root => "Root",
                InstallStrategy::Data => "Data",
                InstallStrategy::Fomod(_) => "FOMOD",
            }
        ))
        .build();

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("root", "Install to Root");
    dialog.add_response("data", "Install to Data");
    dialog.set_response_appearance("data", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some(match detected {
        InstallStrategy::Root => "root",
        _ => "data",
    }));
    dialog.set_close_response("cancel");

    let archive_path_c = archive_path.to_path_buf();
    let archive_name_c = archive_name.to_string();
    let game_c = game.clone();
    let config_c = Rc::clone(config);
    let container_c = container.clone();

    dialog.connect_response(None, move |_, response| {
        let strategy = match response {
            "root" => InstallStrategy::Root,
            "data" => InstallStrategy::Data,
            _ => return,
        };

        do_install(
            &archive_path_c,
            &archive_name_c,
            &game_c,
            &config_c,
            &container_c,
            &strategy,
        );
    });

    dialog.present(parent);
}

fn do_install(
    archive_path: &Path,
    archive_name: &str,
    game: &Game,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    strategy: &InstallStrategy,
) {
    // Derive mod name from archive filename (strip extension)
    let mod_name = Path::new(archive_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| archive_name.to_string());

    match install_mod_from_archive(archive_path, game, &mod_name, strategy) {
        Ok(_mod_entry) => {
            // Mark as installed
            let mut cfg = config.borrow_mut();
            if !cfg.installed_archives.contains(&archive_name.to_string()) {
                cfg.installed_archives.push(archive_name.to_string());
            }
            cfg.save();
            drop(cfg);
            log::info!("Installed mod \"{}\" from \"{}\"", mod_name, archive_name);

            let game_rc: Rc<Option<Game>> = Rc::new(Some(game.clone()));
            render_local_downloads(container, config, false, &game_rc);
        }
        Err(e) => {
            log::error!("Failed to install mod \"{mod_name}\": {e}");
        }
    }
}

// ── FOMOD wizard ──────────────────────────────────────────────────────────────

fn show_fomod_wizard(
    parent: Option<&gtk4::Window>,
    archive_path: &Path,
    archive_name: &str,
    game: &Game,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    fomod: &FomodConfig,
) {
    let mod_display_name = fomod
        .mod_name
        .clone()
        .unwrap_or_else(|| archive_name.to_string());

    if fomod.steps.is_empty() {
        // No interactive steps – just install required files
        let strategy = InstallStrategy::Fomod(fomod.required_files.clone());
        do_install(archive_path, archive_name, game, config, container, &strategy);
        return;
    }

    let dialog = adw::Window::builder()
        .title(&format!("Install: {mod_display_name}"))
        .modal(true)
        .default_width(600)
        .default_height(500)
        .build();

    if let Some(p) = parent {
        dialog.set_transient_for(Some(p));
    }

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    main_box.set_margin_start(24);
    main_box.set_margin_end(24);
    main_box.set_margin_top(12);
    main_box.set_margin_bottom(12);

    // Track selections: step_index -> group_index -> Vec<selected plugin indices>
    let selections: Rc<RefCell<Vec<Vec<Vec<usize>>>>> = Rc::new(RefCell::new(Vec::new()));

    // Initialize selections with defaults
    {
        let mut sel = selections.borrow_mut();
        for step in &fomod.steps {
            let mut step_sel = Vec::new();
            for group in &step.groups {
                let mut group_sel: Vec<usize> = Vec::new();
                for (idx, plugin) in group.plugins.iter().enumerate() {
                    match plugin.type_descriptor {
                        PluginType::Required | PluginType::Recommended => {
                            group_sel.push(idx);
                        }
                        _ => {}
                    }
                    if group.group_type == GroupType::SelectAll {
                        group_sel.push(idx);
                    }
                }
                // Ensure SelectExactlyOne / SelectAtLeastOne has at least one
                if group_sel.is_empty()
                    && (group.group_type == GroupType::SelectExactlyOne
                        || group.group_type == GroupType::SelectAtLeastOne)
                    && !group.plugins.is_empty()
                {
                    group_sel.push(0);
                }
                group_sel.sort();
                group_sel.dedup();
                step_sel.push(group_sel);
            }
            sel.push(step_sel);
        }
    }

    // Notebook-style: show one step at a time
    let step_index = Rc::new(RefCell::new(0usize));
    let step_content = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    step_content.set_vexpand(true);

    let nav_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    nav_box.set_halign(gtk4::Align::End);
    nav_box.set_margin_top(8);

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

    // Render the current step
    let render_step = {
        let step_content_c = step_content.clone();
        let fomod_c = Rc::clone(&fomod_rc);
        let selections_c = Rc::clone(&selections);
        let back_btn_c = back_btn.clone();
        let next_btn_c = next_btn.clone();
        let install_btn_c = install_btn.clone();

        Rc::new(move |idx: usize| {
            while let Some(child) = step_content_c.first_child() {
                step_content_c.remove(&child);
            }

            back_btn_c.set_sensitive(idx > 0);
            next_btn_c.set_visible(idx + 1 < step_count);
            install_btn_c.set_visible(idx + 1 >= step_count);

            if idx >= fomod_c.steps.len() {
                return;
            }

            let step = &fomod_c.steps[idx];
            let title = gtk4::Label::new(Some(&step.name));
            title.add_css_class("title-2");
            title.set_halign(gtk4::Align::Start);
            step_content_c.append(&title);

            let scrolled = gtk4::ScrolledWindow::new();
            scrolled.set_vexpand(true);
            scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);

            let groups_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);

            for (gi, group) in step.groups.iter().enumerate() {
                let frame = gtk4::Frame::new(Some(&format!(
                    "{} ({})",
                    group.name,
                    match group.group_type {
                        GroupType::SelectExactlyOne => "select one",
                        GroupType::SelectAtMostOne => "select at most one",
                        GroupType::SelectAtLeastOne => "select at least one",
                        GroupType::SelectAll => "all required",
                        GroupType::SelectAny => "select any",
                    }
                )));

                let list_box = gtk4::ListBox::new();
                list_box.add_css_class("boxed-list");
                list_box.set_selection_mode(gtk4::SelectionMode::None);

                let use_radio = group.group_type == GroupType::SelectExactlyOne
                    || group.group_type == GroupType::SelectAtMostOne;

                // For radio-style groups, create a shared group
                let radio_group: Option<gtk4::CheckButton> = if use_radio {
                    Some(gtk4::CheckButton::new())
                } else {
                    None
                };

                for (pi, plugin) in group.plugins.iter().enumerate() {
                    let row = adw::ActionRow::builder()
                        .title(&plugin.name)
                        .build();

                    if let Some(ref desc) = plugin.description {
                        if !desc.is_empty() {
                            row.set_subtitle(desc);
                        }
                    }

                    let is_all_required = group.group_type == GroupType::SelectAll;

                    if use_radio {
                        let check = gtk4::CheckButton::new();
                        if let Some(ref rg) = radio_group {
                            if pi > 0 {
                                check.set_group(Some(rg));
                            }
                        }
                        // Set initial state
                        let sel = selections_c.borrow();
                        if let Some(group_sel) = sel.get(idx).and_then(|s| s.get(gi)) {
                            check.set_active(group_sel.contains(&pi));
                        }

                        let selections_cc = Rc::clone(&selections_c);
                        let step_idx = idx;
                        check.connect_toggled(move |btn| {
                            let mut sel = selections_cc.borrow_mut();
                            if let Some(group_sel) = sel.get_mut(step_idx).and_then(|s| s.get_mut(gi)) {
                                if btn.is_active() {
                                    // For radio, clear first
                                    group_sel.clear();
                                    group_sel.push(pi);
                                } else {
                                    group_sel.retain(|&x| x != pi);
                                }
                            }
                        });

                        check.set_valign(gtk4::Align::Center);
                        row.add_prefix(&check);
                        row.set_activatable_widget(Some(&check));
                    } else {
                        let check = gtk4::CheckButton::new();
                        let sel = selections_c.borrow();
                        if let Some(group_sel) = sel.get(idx).and_then(|s| s.get(gi)) {
                            check.set_active(group_sel.contains(&pi));
                        }

                        if is_all_required {
                            check.set_active(true);
                            check.set_sensitive(false);
                        }

                        let selections_cc = Rc::clone(&selections_c);
                        let step_idx = idx;
                        check.connect_toggled(move |btn| {
                            let mut sel = selections_cc.borrow_mut();
                            if let Some(group_sel) = sel.get_mut(step_idx).and_then(|s| s.get_mut(gi)) {
                                if btn.is_active() {
                                    if !group_sel.contains(&pi) {
                                        group_sel.push(pi);
                                    }
                                } else {
                                    group_sel.retain(|&x| x != pi);
                                }
                            }
                        });

                        check.set_valign(gtk4::Align::Center);
                        row.add_prefix(&check);
                        row.set_activatable_widget(Some(&check));
                    }

                    // Type badge
                    let type_label = match plugin.type_descriptor {
                        PluginType::Required => Some("Required"),
                        PluginType::Recommended => Some("Recommended"),
                        PluginType::NotUsable => Some("Not Usable"),
                        PluginType::Optional => None,
                    };
                    if let Some(label_text) = type_label {
                        let badge = gtk4::Label::new(Some(label_text));
                        badge.add_css_class("dim-label");
                        badge.add_css_class("caption");
                        badge.set_valign(gtk4::Align::Center);
                        row.add_suffix(&badge);
                    }

                    list_box.append(&row);
                }

                frame.set_child(Some(&list_box));
                groups_box.append(&frame);
            }

            scrolled.set_child(Some(&groups_box));
            step_content_c.append(&scrolled);
        })
    };

    // Render initial step
    render_step(0);

    // Wire navigation buttons
    {
        let step_index_c = Rc::clone(&step_index);
        let render_step_c = Rc::clone(&render_step);
        back_btn.connect_clicked(move |_| {
            let mut idx = step_index_c.borrow_mut();
            if *idx > 0 {
                *idx -= 1;
                render_step_c(*idx);
            }
        });
    }

    {
        let step_index_c = Rc::clone(&step_index);
        let render_step_c = Rc::clone(&render_step);
        next_btn.connect_clicked(move |_| {
            let mut idx = step_index_c.borrow_mut();
            if *idx + 1 < step_count {
                *idx += 1;
                render_step_c(*idx);
            }
        });
    }

    // Wire install button
    {
        let selections_c = Rc::clone(&selections);
        let fomod_c = Rc::clone(&fomod_rc);
        let archive_path_c = archive_path.to_path_buf();
        let archive_name_c = archive_name.to_string();
        let game_c = game.clone();
        let config_c = Rc::clone(config);
        let container_c = container.clone();
        let dialog_c = dialog.clone();

        install_btn.connect_clicked(move |_| {
            // Collect all selected files
            let mut files: Vec<FomodFile> = fomod_c.required_files.clone();

            let sel = selections_c.borrow();
            for (si, step) in fomod_c.steps.iter().enumerate() {
                for (gi, group) in step.groups.iter().enumerate() {
                    if let Some(group_sel) = sel.get(si).and_then(|s| s.get(gi)) {
                        for &pi in group_sel {
                            if let Some(plugin) = group.plugins.get(pi) {
                                files.extend(plugin.files.iter().cloned());
                            }
                        }
                    }
                }
            }

            let strategy = InstallStrategy::Fomod(files);
            do_install(
                &archive_path_c,
                &archive_name_c,
                &game_c,
                &config_c,
                &container_c,
                &strategy,
            );
            dialog_c.destroy();
        });
    }

    main_box.append(&step_content);
    main_box.append(&nav_box);
    toolbar_view.set_content(Some(&main_box));
    dialog.set_content(Some(&toolbar_view));
    dialog.present();
}

// ── Nexus browse view ─────────────────────────────────────────────────────────

fn render_nexus_browse(
    container: &gtk4::Box,
    config: &Rc<RefCell<AppConfig>>,
    game: &Rc<Option<Game>>,
) {
    let api_key = config.borrow().nexus_api_key.clone();
    let Some(api_key) = api_key else {
        let status = adw::StatusPage::builder()
            .title("No API Key")
            .description("Set your NexusMods API key in Preferences to browse mods.")
            .icon_name("dialog-password-symbolic")
            .build();
        status.set_vexpand(true);
        container.append(&status);
        return;
    };

    let Some(ref game_inner) = **game else {
        let status = adw::StatusPage::builder()
            .title("No Game Selected")
            .description("Select a game first to browse Nexus Mods.")
            .icon_name("applications-games-symbolic")
            .build();
        status.set_vexpand(true);
        container.append(&status);
        return;
    };

    let game_domain = game_inner.kind.nexus_game_id().to_string();

    // Loading indicator
    let spinner = gtk4::Spinner::new();
    spinner.set_spinning(true);
    spinner.set_vexpand(true);
    spinner.set_halign(gtk4::Align::Center);
    spinner.set_valign(gtk4::Align::Center);
    container.append(&spinner);

    // Fetch trending mods in background
    let (tx, rx) = mpsc::channel();
    let game_domain_c = game_domain.clone();
    std::thread::spawn(move || {
        let client = NexusClient::new(&api_key);
        let result = client.list_trending_mods(&game_domain_c);
        let _ = tx.send(result);
    });

    let container_c = container.clone();
    let config_c = Rc::clone(config);
    let game_c = Rc::clone(game);
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        match rx.try_recv() {
            Ok(Ok(mods)) => {
                while let Some(child) = container_c.first_child() {
                    container_c.remove(&child);
                }
                render_mod_list(&container_c, &mods, &config_c, &game_c);
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                while let Some(child) = container_c.first_child() {
                    container_c.remove(&child);
                }
                let status = adw::StatusPage::builder()
                    .title("Error")
                    .description(&format!("Failed to load mods: {e}"))
                    .icon_name("dialog-error-symbolic")
                    .build();
                status.set_vexpand(true);
                container_c.append(&status);
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn render_mod_list(
    container: &gtk4::Box,
    mods: &[crate::core::nexus::NexusModInfo],
    config: &Rc<RefCell<AppConfig>>,
    game: &Rc<Option<Game>>,
) {
    let title = gtk4::Label::new(Some("Trending Mods"));
    title.add_css_class("title-2");
    title.set_halign(gtk4::Align::Start);
    title.set_margin_start(12);
    title.set_margin_top(12);
    container.append(&title);

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::None);

    for m in mods {
        let row = adw::ActionRow::builder()
            .title(&m.name)
            .activatable(true)
            .build();

        let subtitle = match (&m.version, &m.author) {
            (Some(v), Some(a)) => format!("v{v} by {a} · \u{2b50} {}", m.endorsement_count),
            (Some(v), None) => format!("v{v} · \u{2b50} {}", m.endorsement_count),
            (None, Some(a)) => format!("by {a} · \u{2b50} {}", m.endorsement_count),
            (None, None) => format!("\u{2b50} {}", m.endorsement_count),
        };
        row.set_subtitle(&subtitle);

        if let Some(ref summary) = m.summary {
            row.set_tooltip_text(Some(summary));
        }

        let arrow = gtk4::Image::from_icon_name("go-next-symbolic");
        arrow.add_css_class("dim-label");
        row.add_suffix(&arrow);

        // On click, navigate to mod files view
        let mod_id = m.mod_id;
        let mod_name = m.name.clone();
        let config_c = Rc::clone(config);
        let game_c = Rc::clone(game);
        let container_c = container.clone();
        row.connect_activated(move |_| {
            while let Some(child) = container_c.first_child() {
                container_c.remove(&child);
            }
            let view = ViewMode::NexusModFiles {
                mod_id,
                mod_name: mod_name.clone(),
            };
            refresh_content(&container_c, &config_c, false, &view, &game_c);
        });

        list_box.append(&row);
    }

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(900);
    clamp.set_child(Some(&list_box));
    clamp.set_margin_top(4);
    clamp.set_margin_bottom(12);
    clamp.set_margin_start(12);
    clamp.set_margin_end(12);

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
    scrolled.set_child(Some(&clamp));

    container.append(&scrolled);
}

// ── Nexus mod files view ──────────────────────────────────────────────────────

fn render_nexus_mod_files(
    container: &gtk4::Box,
    config: &Rc<RefCell<AppConfig>>,
    game: &Rc<Option<Game>>,
    mod_id: u64,
    mod_name: &str,
) {
    let api_key = config.borrow().nexus_api_key.clone();
    let Some(api_key) = api_key else {
        return;
    };
    let Some(ref game_inner) = **game else {
        return;
    };

    let game_domain = game_inner.kind.nexus_game_id().to_string();

    // Back button
    let back_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    back_box.set_margin_start(12);
    back_box.set_margin_top(12);
    let back_btn = gtk4::Button::new();
    back_btn.set_icon_name("go-previous-symbolic");
    back_btn.set_tooltip_text(Some("Back to browse"));
    back_btn.add_css_class("flat");

    let config_back = Rc::clone(config);
    let game_back = Rc::clone(game);
    let container_back = container.clone();
    back_btn.connect_clicked(move |_| {
        while let Some(child) = container_back.first_child() {
            container_back.remove(&child);
        }
        let view = ViewMode::NexusBrowse;
        refresh_content(&container_back, &config_back, false, &view, &game_back);
    });
    back_box.append(&back_btn);

    let title = gtk4::Label::new(Some(mod_name));
    title.add_css_class("title-2");
    back_box.append(&title);
    container.append(&back_box);

    // Loading
    let spinner = gtk4::Spinner::new();
    spinner.set_spinning(true);
    spinner.set_vexpand(true);
    spinner.set_halign(gtk4::Align::Center);
    spinner.set_valign(gtk4::Align::Center);
    container.append(&spinner);

    let (tx, rx) = mpsc::channel();
    let game_domain_c = game_domain.clone();
    std::thread::spawn(move || {
        let client = NexusClient::new(&api_key);
        let result = client.get_mod_files(&game_domain_c, mod_id as u32);
        let _ = tx.send(result);
    });

    let container_c = container.clone();
    let config_c = Rc::clone(config);
    let game_c = Rc::clone(game);
    let mod_name_c = mod_name.to_string();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        match rx.try_recv() {
            Ok(Ok(files)) => {
                // Remove spinner
                if let Some(child) = container_c.last_child() {
                    if child.is::<gtk4::Spinner>() {
                        container_c.remove(&child);
                    }
                }
                render_file_list(
                    &container_c,
                    &files,
                    &config_c,
                    &game_c,
                    mod_id,
                    &mod_name_c,
                );
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                if let Some(child) = container_c.last_child() {
                    if child.is::<gtk4::Spinner>() {
                        container_c.remove(&child);
                    }
                }
                let err_label = gtk4::Label::new(Some(&format!("Error: {e}")));
                err_label.add_css_class("error");
                err_label.set_margin_start(12);
                container_c.append(&err_label);
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn render_file_list(
    container: &gtk4::Box,
    files: &[crate::core::nexus::NexusModFile],
    config: &Rc<RefCell<AppConfig>>,
    game: &Rc<Option<Game>>,
    mod_id: u64,
    mod_name: &str,
) {
    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::None);

    for file in files {
        let row = adw::ActionRow::builder()
            .title(&file.name)
            .subtitle(&format!(
                "{} · {} · {}",
                file.category_name,
                file.version.as_deref().unwrap_or("—"),
                format_size(file.size_kb * 1024)
            ))
            .build();

        if file.is_primary {
            let badge = gtk4::Label::new(Some("Primary"));
            badge.add_css_class("success");
            badge.add_css_class("caption");
            badge.set_valign(gtk4::Align::Center);
            row.add_suffix(&badge);
        }

        // Download button
        let dl_btn = gtk4::Button::new();
        dl_btn.set_icon_name("folder-download-symbolic");
        dl_btn.set_tooltip_text(Some("Download"));
        dl_btn.set_valign(gtk4::Align::Center);
        dl_btn.add_css_class("flat");
        dl_btn.add_css_class("suggested-action");

        let file_id = file.file_id;
        let file_name = file.file_name.clone();
        let config_c = Rc::clone(config);
        let game_c = Rc::clone(game);

        dl_btn.connect_clicked(move |btn| {
            start_download(
                btn,
                &config_c,
                &game_c,
                mod_id,
                file_id,
                &file_name,
            );
        });

        row.add_suffix(&dl_btn);
        list_box.append(&row);
    }

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(900);
    clamp.set_child(Some(&list_box));
    clamp.set_margin_top(4);
    clamp.set_margin_bottom(12);
    clamp.set_margin_start(12);
    clamp.set_margin_end(12);

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
    scrolled.set_child(Some(&clamp));

    container.append(&scrolled);
}

// ── Download from Nexus ───────────────────────────────────────────────────────

fn start_download(
    btn: &gtk4::Button,
    config: &Rc<RefCell<AppConfig>>,
    game: &Rc<Option<Game>>,
    mod_id: u64,
    file_id: u64,
    file_name: &str,
) {
    let api_key = config.borrow().nexus_api_key.clone();
    let Some(api_key) = api_key else {
        return;
    };
    let Some(ref game_inner) = **game else {
        return;
    };
    let game_domain = game_inner.kind.nexus_game_id().to_string();
    let downloads_dir = config.borrow().downloads_dir();
    let dest_path = downloads_dir.join(file_name);

    if dest_path.exists() {
        show_toast(btn.upcast_ref(), "File already downloaded");
        return;
    }

    btn.set_sensitive(false);
    btn.set_icon_name("content-loading-symbolic");

    let (tx, rx) = mpsc::channel::<Result<PathBuf, String>>();
    let file_name_c = file_name.to_string();

    std::thread::spawn(move || {
        let result = (|| -> Result<PathBuf, String> {
            let client = NexusClient::new(&api_key);
            let links = client.get_download_links(&game_domain, mod_id as u32, file_id)?;
            let (_, url) = links.first().ok_or("No download links available")?;
            download::download_file(url, &dest_path, |_downloaded, _total| {
                // Progress reporting could be added here
            })
        })();
        let _ = tx.send(result);
    });

    let btn_c = btn.clone();
    let file_name_cc = file_name.to_string();
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        match rx.try_recv() {
            Ok(Ok(_path)) => {
                btn_c.set_sensitive(true);
                btn_c.set_icon_name("emblem-ok-symbolic");
                show_toast(btn_c.upcast_ref(), &format!("Downloaded {file_name_cc}"));
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                btn_c.set_sensitive(true);
                btn_c.set_icon_name("folder-download-symbolic");
                log::error!("Download failed: {e}");
                show_toast(btn_c.upcast_ref(), &format!("Download failed: {e}"));
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                btn_c.set_sensitive(true);
                btn_c.set_icon_name("folder-download-symbolic");
                glib::ControlFlow::Break
            }
        }
    });
}

// ── Clean cache dialog ────────────────────────────────────────────────────────

fn show_clean_cache_dialog(
    anchor: &gtk4::Button,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: &Rc<RefCell<bool>>,
    view_mode: &Rc<RefCell<ViewMode>>,
    game: &Rc<Option<Game>>,
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
    let view_mode_c = Rc::clone(view_mode);
    let game_c = Rc::clone(game);
    dialog.connect_response(None, move |_, response| {
        if response == "clean" {
            delete_all_archives(&config_c);
            refresh_content(
                &container_c,
                &config_c,
                *hide_c.borrow(),
                &view_mode_c.borrow(),
                &game_c,
            );
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

/// Show a brief in-app toast notification anchored to `widget`.
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
