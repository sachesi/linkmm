use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gio;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::Game;
use crate::core::installer::{
    DependencyOperator, FlagDependency, FomodConfig, FomodFile, FomodPlugin, GroupType,
    InstallStrategy, PluginDependencies, PluginType, detect_strategy, install_mod_from_archive,
    parse_fomod_from_zip, read_archive_file_bytes,
};
use crate::core::mods::ModDatabase;

// ── Archive extensions ────────────────────────────────────────────────────────

/// All archive file types the Downloads page can clean from cache.
const ARCHIVE_EXTENSIONS: &[&str] = &["zip", "rar", "7z", "tar", "gz", "bz2", "xz"];
/// Archive types that are currently installable by the app.
const INSTALLABLE_ARCHIVE_EXTENSIONS: &[&str] = &["zip"];

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

    let refresh_btn = gtk4::Button::new();
    refresh_btn.set_icon_name("view-refresh-symbolic");
    refresh_btn.set_tooltip_text(Some("Refresh downloads list"));
    header.pack_start(&refresh_btn);

    toolbar_view.add_top_bar(&header);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.set_vexpand(true);

    let hide_installed = Rc::new(RefCell::new(false));
    let game_rc: Rc<Option<Game>> = Rc::new(game.cloned());

    refresh_content(&content, &config, *hide_installed.borrow(), &game_rc);
    toolbar_view.set_content(Some(&content));

    {
        let content_c = content.clone();
        let config_c = Rc::clone(&config);
        let hide_c = Rc::clone(&hide_installed);
        let game_c = Rc::clone(&game_rc);
        hide_btn.connect_toggled(move |btn| {
            *hide_c.borrow_mut() = btn.is_active();
            refresh_content(&content_c, &config_c, *hide_c.borrow(), &game_c);
        });
    }

    {
        let content_c = content.clone();
        let config_c = Rc::clone(&config);
        let hide_c = Rc::clone(&hide_installed);
        let game_c = Rc::clone(&game_rc);
        clean_btn.connect_clicked(move |btn| {
            show_clean_cache_dialog(btn, &config_c, &content_c, &hide_c, &game_c);
        });
    }

    {
        let content_c = content.clone();
        let config_c = Rc::clone(&config);
        let hide_c = Rc::clone(&hide_installed);
        let game_c = Rc::clone(&game_rc);
        refresh_btn.connect_clicked(move |_| {
            refresh_content(&content_c, &config_c, *hide_c.borrow(), &game_c);
        });
    }

    toolbar_view.upcast()
}

// ── Content rendering ─────────────────────────────────────────────────────────

fn refresh_content(
    container: &gtk4::Box,
    config: &Rc<RefCell<AppConfig>>,
    hide_installed: bool,
    game: &Rc<Option<Game>>,
) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let downloads_dir = config.borrow().downloads_dir();

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

    let visible: Vec<&DownloadEntry> = if hide_installed {
        entries
            .iter()
            .filter(|e| !entry_is_installed(e, &installed_archives, &installed_mod_names))
            .collect()
    } else {
        entries.iter().collect()
    };

    if visible.is_empty() {
        let description = if hide_installed && !entries.is_empty() {
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
    for entry in &visible {
        let row = build_entry_row(
            entry,
            &installed_archives,
            &installed_mod_names,
            config,
            container,
            hide_installed,
            game,
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

// ── Row builder ───────────────────────────────────────────────────────────────

fn build_entry_row(
    entry: &DownloadEntry,
    installed_archives: &[String],
    installed_mod_names: &[String],
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: bool,
    game: &Rc<Option<Game>>,
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
            let game_rc_c = Rc::clone(game);
            let hide_installed_c = hide_installed;
            install_btn.connect_clicked(move |btn| {
                show_install_dialog(
                    btn,
                    &path_c,
                    &name_c,
                    &game_c,
                    &config_c,
                    &container_c,
                    hide_installed_c,
                    &game_rc_c,
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
    delete_btn.connect_clicked(move |_| {
        if let Err(e) = std::fs::remove_file(&path_c) {
            log::error!("Failed to remove archive \"{}\": {e}", name_c);
        } else {
            let mut cfg = config_c.borrow_mut();
            cfg.installed_archives.retain(|a| a != &name_c);
            cfg.save();
            drop(cfg);
            refresh_content(&container_c, &config_c, hide_installed, &game_c);
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
    game_rc: &Rc<Option<Game>>,
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
                    hide_installed,
                    &fomod_config,
                    game_rc,
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
        &strategy,
        game_rc,
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
    _detected: &InstallStrategy,
    game_rc: &Rc<Option<Game>>,
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
    let grc = Rc::clone(game_rc);
    dialog.connect_response(None, move |_, response| {
        if response == "data" {
            do_install(
                &ap,
                &an,
                &gc,
                &cc,
                &cont,
                hide,
                &InstallStrategy::Data,
                &grc,
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
    strategy: &InstallStrategy,
    game_rc: &Rc<Option<Game>>,
) {
    let mod_name = Path::new(archive_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| archive_name.to_string());

    match install_mod_from_archive(archive_path, game, &mod_name, strategy) {
        Ok(_) => {
            let mut cfg = config.borrow_mut();
            if !cfg.installed_archives.contains(&archive_name.to_string()) {
                cfg.installed_archives.push(archive_name.to_string());
            }
            cfg.save();
            drop(cfg);
            log::info!("Installed mod \"{mod_name}\" from \"{archive_name}\"");
            show_toast(container.upcast_ref(), &format!("Installed: {mod_name}"));
            refresh_content(container, config, hide_installed, game_rc);
        }
        Err(e) => {
            log::error!("Failed to install mod \"{mod_name}\": {e}");
            show_toast(container.upcast_ref(), &format!("Install failed: {e}"));
        }
    }
}

// ── FOMOD wizard ──────────────────────────────────────────────────────────────

/// Selected plugin indices by `[step_index][group_index][plugin_indices]`.
type FomodSelections = Vec<Vec<Vec<usize>>>;
const OPTION_IMAGE_WIDTH: i32 = 128;
const OPTION_IMAGE_HEIGHT: i32 = 72;

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

fn plugin_is_visible(
    plugin: &FomodPlugin,
    active_flags: &HashMap<String, HashSet<String>>,
) -> bool {
    let Some(PluginDependencies { operator, flags }) = &plugin.dependencies else {
        return true;
    };
    if flags.is_empty() {
        return true;
    }
    match operator {
        DependencyOperator::And => flags
            .iter()
            .all(|dep| flag_dependency_matches(dep, active_flags)),
        DependencyOperator::Or => flags
            .iter()
            .any(|dep| flag_dependency_matches(dep, active_flags)),
    }
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
    fomod: &FomodConfig,
    game_rc: &Rc<Option<Game>>,
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
            &strategy,
            game_rc,
        );
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
    toolbar_view.add_top_bar(&adw::HeaderBar::new());

    let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
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
            sc.append(&title);
            let scrolled = gtk4::ScrolledWindow::new();
            scrolled.set_vexpand(true);
            scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
            let groups_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
            for (gi, group) in step.groups.iter().enumerate() {
                let type_desc = match group.group_type {
                    GroupType::SelectExactlyOne => "select one",
                    GroupType::SelectAtMostOne => "select at most one",
                    GroupType::SelectAtLeastOne => "select at least one",
                    GroupType::SelectAll => "all required",
                    GroupType::SelectAny => "select any",
                };
                let frame = gtk4::Frame::new(Some(&format!("{} ({type_desc})", group.name)));
                let lb = gtk4::ListBox::new();
                lb.add_css_class("boxed-list");
                lb.set_selection_mode(gtk4::SelectionMode::None);
                let use_radio = matches!(
                    group.group_type,
                    GroupType::SelectExactlyOne | GroupType::SelectAtMostOne
                );
                let radio_group: Option<gtk4::CheckButton> = if use_radio {
                    Some(gtk4::CheckButton::new())
                } else {
                    None
                };
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
                        if let Some(ref rg) = radio_group {
                            if pi > 0 {
                                check.set_group(Some(rg));
                            }
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
                            let pic = gtk4::Picture::new();
                            pic.set_paintable(Some(&texture));
                            pic.set_content_fit(gtk4::ContentFit::Contain);
                            pic.set_size_request(OPTION_IMAGE_WIDTH, OPTION_IMAGE_HEIGHT);
                            row.add_suffix(&pic);
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
            sc.append(&scrolled);
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
        let grc = Rc::clone(game_rc);
        install_btn.connect_clicked(move |_| {
            let files = {
                let sel = sel_c.borrow();
                resolve_fomod_files(&fc, &sel)
            };
            do_install(
                &ap,
                &an,
                &gc,
                &cc,
                &cont,
                hide,
                &InstallStrategy::Fomod(files),
                &grc,
            );
            dlg.destroy();
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
        fomod,
        game_rc,
    );
}

// ── Clean cache dialog ────────────────────────────────────────────────────────

fn show_clean_cache_dialog(
    anchor: &gtk4::Button,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    hide_installed: &Rc<RefCell<bool>>,
    game: &Rc<Option<Game>>,
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
    let gc = Rc::clone(game);
    dialog.connect_response(None, move |_, response| {
        if response == "clean" {
            delete_all_archives(&cc);
            refresh_content(&cont, &cc, *hc.borrow(), &gc);
        }
    });
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
            if !INSTALLABLE_ARCHIVE_EXTENSIONS.contains(&ext.as_str()) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::installer::{
        ConditionFlag, DependencyOperator, FlagDependency, FomodPlugin, InstallStep,
        PluginDependencies, PluginGroup,
    };

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

    #[test]
    fn resolve_fomod_files_filters_plus_minus_variants_by_dependency_flags() {
        let mut config = FomodConfig {
            mod_name: Some("Test".to_string()),
            required_files: Vec::new(),
            steps: Vec::new(),
        };
        config.steps.push(InstallStep {
            name: "Flags".to_string(),
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
}
