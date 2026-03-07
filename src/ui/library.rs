use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gio;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::Game;
use crate::core::mods::{ModDatabase, ModManager};

/// Build the full Library page for `game`.
pub fn build_library_page(game: &Game, config: Rc<RefCell<AppConfig>>) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let title_widget = adw::WindowTitle::new("Library", &game.name);
    header.set_title_widget(Some(&title_widget));

    let search_entry = gtk4::SearchEntry::new();
    search_entry.set_placeholder_text(Some("Search mods"));
    search_entry.set_width_chars(24);
    header.pack_start(&search_entry);

    // Deploy button – applies all enabled mods by (re)linking their files
    let deploy_btn = gtk4::Button::with_label("Deploy");
    deploy_btn.add_css_class("suggested-action");
    deploy_btn.set_tooltip_text(Some(
        "Apply all enabled mods by linking their files into the game directory",
    ));
    header.pack_end(&deploy_btn);

    // Undeploy button – removes all mod symlinks from the game directory
    let undeploy_btn = gtk4::Button::with_label("Undeploy");
    undeploy_btn.add_css_class("destructive-action");
    undeploy_btn.set_tooltip_text(Some("Remove all mod symlinks from the game directory"));
    header.pack_end(&undeploy_btn);

    // Add-mod button
    let add_mod_btn = gtk4::Button::new();
    add_mod_btn.set_icon_name("list-add-symbolic");
    add_mod_btn.set_tooltip_text(Some("Add Mod"));
    header.pack_end(&add_mod_btn);

    toolbar_view.add_top_bar(&header);

    let content_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content_container.set_vexpand(true);

    let game_rc = Rc::new(game.clone());
    let search_query = Rc::new(RefCell::new(String::new()));

    refresh_library_content_with_search(
        &content_container,
        &game_rc,
        Rc::clone(&config),
        &search_query.borrow(),
    );

    toolbar_view.set_content(Some(&content_container));

    wire_add_mod_button(
        &add_mod_btn,
        &game_rc,
        &content_container,
        Rc::clone(&config),
    );

    {
        let container_c = content_container.clone();
        let game_c = Rc::clone(&game_rc);
        let config_c = Rc::clone(&config);
        let search_c = Rc::clone(&search_query);
        search_entry.connect_search_changed(move |entry| {
            *search_c.borrow_mut() = entry.text().to_string();
            refresh_library_content_with_search(
                &container_c,
                &game_c,
                Rc::clone(&config_c),
                &search_c.borrow(),
            );
        });
    }

    // Wire Deploy button: undeploy everything, then deploy all enabled mods
    {
        let game_c = Rc::clone(&game_rc);
        let container_c = content_container.clone();
        let config_c = Rc::clone(&config);
        let search_c = Rc::clone(&search_query);
        deploy_btn.connect_clicked(move |btn| {
            let db = ModDatabase::load(&game_c);
            let mut errors: Vec<String> = Vec::new();

            // First, unlink all tracked mods so we start from a clean state
            for m in &db.mods {
                if let Err(e) = ModManager::disable_mod(&game_c, m) {
                    log::warn!("Undeploy warning for {}: {e}", m.name);
                }
            }

            // Then deploy all enabled mods
            let mut deployed_count = 0usize;
            for m in db.mods.iter().filter(|m| m.enabled) {
                if let Err(e) = ModManager::enable_mod(&game_c, m) {
                    errors.push(format!("{}: {}", m.name, e));
                } else {
                    deployed_count += 1;
                }
            }
            let _ = db.write_plugins_txt(&game_c);

            let msg = if errors.is_empty() {
                format!("Deployed {deployed_count} mod(s)")
            } else {
                for e in &errors {
                    log::error!("Deploy error: {e}");
                }
                format!("Deploy finished with {} error(s)", errors.len())
            };
            show_toast(btn.upcast_ref(), &msg);
            refresh_library_content_with_search(
                &container_c,
                &game_c,
                Rc::clone(&config_c),
                &search_c.borrow(),
            );
        });
    }

    // Wire Undeploy button: remove all mod symlinks from the game directory
    {
        let game_c = Rc::clone(&game_rc);
        let container_c = content_container.clone();
        let config_c = Rc::clone(&config);
        let search_c = Rc::clone(&search_query);
        undeploy_btn.connect_clicked(move |btn| {
            let db = ModDatabase::load(&game_c);
            let mut errors: Vec<String> = Vec::new();
            let mut count = 0;
            // Unlink ALL mods regardless of enabled state so the game directory
            // is fully clean.  The enabled state is intentionally preserved so
            // the user can re-deploy with the same selection later.
            for m in &db.mods {
                if let Err(e) = ModManager::disable_mod(&game_c, m) {
                    errors.push(format!("{}: {}", m.name, e));
                } else {
                    count += 1;
                }
            }
            let _ = db.write_plugins_txt(&game_c);

            let msg = if errors.is_empty() {
                format!("Undeployed {count} mod(s)")
            } else {
                for e in &errors {
                    log::error!("Undeploy error: {e}");
                }
                format!("Undeploy finished with {} error(s)", errors.len())
            };
            show_toast(btn.upcast_ref(), &msg);
            refresh_library_content_with_search(
                &container_c,
                &game_c,
                Rc::clone(&config_c),
                &search_c.borrow(),
            );
        });
    }

    toolbar_view.upcast()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn refresh_library_content(container: &gtk4::Box, game: &Rc<Game>, config: Rc<RefCell<AppConfig>>) {
    refresh_library_content_with_search(container, game, config, "");
}

fn refresh_library_content_with_search(
    container: &gtk4::Box,
    game: &Rc<Game>,
    config: Rc<RefCell<AppConfig>>,
    search_query: &str,
) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let mut db = ModDatabase::load(game);
    db.scan_mods_dir(game);
    db.save(game);

    let visible_mods: Vec<_> = db
        .mods
        .iter()
        .filter(|m| matches_query(&m.name, search_query))
        .cloned()
        .collect();

    if visible_mods.is_empty() {
        if !search_query.trim().is_empty() && !db.mods.is_empty() {
            let status = adw::StatusPage::builder()
                .title("No Matching Mods")
                .description("No installed mods match your search.")
                .icon_name("system-search-symbolic")
                .build();
            status.set_vexpand(true);
            container.append(&status);
            return;
        }
        let status = adw::StatusPage::builder()
            .title("No Mods Installed")
            .description(&format!(
                "Install mods for {} from the Downloads page or by clicking the \u{201c}+\u{201d} button to select a mod archive.",
                game.name
            ))
            .icon_name("package-x-generic-symbolic")
            .build();
        status.set_vexpand(true);

        let add_btn = gtk4::Button::with_label("Add Mod\u{2026}");
        add_btn.add_css_class("suggested-action");
        add_btn.add_css_class("pill");
        add_btn.set_halign(gtk4::Align::Center);

        let game_clone = Rc::clone(game);
        let container_clone = container.clone();
        let config_clone = Rc::clone(&config);
        wire_add_mod_button(&add_btn, &game_clone, &container_clone, config_clone);

        status.set_child(Some(&add_btn));
        container.append(&status);
    } else {
        let list_box = gtk4::ListBox::new();
        list_box.add_css_class("boxed-list");
        list_box.set_selection_mode(gtk4::SelectionMode::None);

        for mod_entry in &visible_mods {
            let row = build_mod_row(mod_entry, game, container, Rc::clone(&config));
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
}

fn wire_add_mod_button(
    btn: &gtk4::Button,
    game: &Rc<Game>,
    container: &gtk4::Box,
    config: Rc<RefCell<AppConfig>>,
) {
    let game_clone = Rc::clone(game);
    let container_clone = container.clone();
    let config_clone = Rc::clone(&config);

    btn.connect_clicked(move |b| {
        let parent = b.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
        let dialog = gtk4::FileDialog::new();
        dialog.set_title("Select Mod Archive");

        // Only allow zip archives
        let filter = gtk4::FileFilter::new();
        filter.set_name(Some("Mod archives (*.zip)"));
        filter.add_pattern("*.zip");
        let filters = gio::ListStore::new::<gtk4::FileFilter>();
        filters.append(&filter);
        dialog.set_filters(Some(&filters));

        let game2 = Rc::clone(&game_clone);
        let container2 = container_clone.clone();
        let config2 = Rc::clone(&config_clone);
        dialog.open(parent.as_ref(), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    if !path.is_file() {
                        return;
                    }

                    let archive_name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Unknown".to_string());

                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|s| s.to_lowercase())
                        .unwrap_or_default();

                    if ext != "zip" {
                        log::error!("Only .zip archives are supported for installation");
                        return;
                    }

                    let strategy = match crate::core::installer::detect_strategy(&path) {
                        Ok(s) => s,
                        Err(e) => {
                            log::error!("Failed to detect install strategy: {e}");
                            return;
                        }
                    };

                    let game_rc: Rc<Option<Game>> = Rc::new(Some((*game2).clone()));

                    // For FOMOD mods, launch the wizard
                    if let crate::core::installer::InstallStrategy::Fomod(_) = &strategy {
                        if let Ok(fomod_config) =
                            crate::core::installer::parse_fomod_from_zip(&path)
                        {
                            crate::ui::downloads::show_fomod_wizard_from_library(
                                None,
                                &path,
                                &archive_name,
                                &game2,
                                &config2,
                                &container2,
                                &fomod_config,
                                &game_rc,
                            );
                            return;
                        }
                    }

                    // Non-FOMOD: use detected strategy directly
                    let mod_name = std::path::Path::new(&archive_name)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| archive_name.clone());

                    match crate::core::installer::install_mod_from_archive(
                        &path, &game2, &mod_name, &strategy,
                    ) {
                        Ok(_) => {
                            let mut cfg = config2.borrow_mut();
                            if !cfg.installed_archives.contains(&archive_name) {
                                cfg.installed_archives.push(archive_name.clone());
                            }
                            cfg.save();
                            drop(cfg);
                            refresh_library_content(&container2, &game2, Rc::clone(&config2));
                        }
                        Err(e) => {
                            log::error!("Failed to install mod: {e}");
                        }
                    }
                }
            }
        });
    });
}

fn build_mod_row(
    mod_entry: &crate::core::mods::Mod,
    game: &Rc<Game>,
    container: &gtk4::Box,
    config: Rc<RefCell<AppConfig>>,
) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title(&mod_entry.name)
        .active(mod_entry.enabled)
        .build();

    // Subtitle: version and Nexus source indicator
    let subtitle = match (&mod_entry.version, mod_entry.installed_from_nexus) {
        (Some(v), true) => format!("{v} · From Nexus Mods"),
        (Some(v), false) => v.clone(),
        (None, true) => "From Nexus Mods".to_string(),
        (None, false) => String::new(),
    };
    if !subtitle.is_empty() {
        row.set_subtitle(&subtitle);
    }

    let mod_id = mod_entry.id.clone();
    let game_clone = Rc::clone(game);

    row.connect_active_notify(move |switch_row| {
        let enabled = switch_row.is_active();
        let mut db = ModDatabase::load(&game_clone);
        if let Some(m) = db.mods.iter_mut().find(|m| m.id == mod_id) {
            m.enabled = enabled;
            db.save(&game_clone);
        }
    });

    // ── Uninstall button ─────────────────────────────────────────────────────
    let delete_btn = gtk4::Button::new();
    delete_btn.set_icon_name("user-trash-symbolic");
    delete_btn.set_tooltip_text(Some("Uninstall mod"));
    delete_btn.add_css_class("flat");
    delete_btn.set_valign(gtk4::Align::Center);
    row.add_suffix(&delete_btn);

    let mod_id_del = mod_entry.id.clone();
    let mod_name_del = mod_entry.name.clone();
    let game_del = Rc::clone(game);
    let container_del = container.clone();
    let config_del = Rc::clone(&config);

    delete_btn.connect_clicked(move |btn| {
        let parent = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());

        let dialog = adw::AlertDialog::builder()
            .heading("Remove Mod?")
            .body(format!(
                "\u{201c}{}\u{201d} will be permanently removed from disk.",
                mod_name_del
            ))
            .build();

        dialog.add_response("cancel", "Cancel");
        dialog.add_response("remove", "Remove");
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let mod_id_c = mod_id_del.clone();
        let game_c = Rc::clone(&game_del);
        let container_c = container_del.clone();
        let config_c = Rc::clone(&config_del);
        dialog.connect_response(None, move |_, response| {
            if response != "remove" {
                return;
            }
            let db = ModDatabase::load(&game_c);
            if let Some(m) = db.mods.iter().find(|m| m.id == mod_id_c) {
                if let Err(e) = ModManager::uninstall_mod(&game_c, m) {
                    log::error!("Failed to uninstall mod: {e}");
                } else {
                    // Keep downloaded archives on disk, but clear install marker
                    // so Downloads reflects that this mod is no longer installed.
                    let mod_name_lower = m.name.to_lowercase();
                    let mut cfg = config_c.borrow_mut();
                    cfg.installed_archives.retain(|archive| {
                        let archive_stem = Path::new(archive)
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_lowercase())
                            .unwrap_or_default();
                        archive_stem != mod_name_lower
                    });
                    cfg.save();
                }
            }
            refresh_library_content(&container_c, &game_c, Rc::clone(&config_c));
        });

        dialog.present(parent.as_ref());
    });

    row
}

fn matches_query(value: &str, query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return true;
    }
    value.to_lowercase().contains(&trimmed.to_lowercase())
}

/// Show a brief in-app toast notification anchored to `widget`.
fn show_toast(widget: &gtk4::Widget, message: &str) {
    // Walk up to the nearest AdwToastOverlay
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
    // Fallback: log to stderr
    log::info!("{message}");
}

#[cfg(test)]
mod tests {
    use super::matches_query;

    #[test]
    fn matches_query_is_case_insensitive() {
        assert!(matches_query("Immersive Armors", "arm"));
        assert!(matches_query("Immersive Armors", "  ARMORS  "));
        assert!(!matches_query("Immersive Armors", "weapons"));
    }
}
