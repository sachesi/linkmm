use std::cell::RefCell;
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

    // Search button (placeholder)
    let search_btn = gtk4::Button::new();
    search_btn.set_icon_name("system-search-symbolic");
    search_btn.set_tooltip_text(Some("Search mods"));
    header.pack_start(&search_btn);

    // Deploy button – applies all enabled mods by (re)linking their files
    let deploy_btn = gtk4::Button::with_label("Deploy");
    deploy_btn.add_css_class("suggested-action");
    deploy_btn.set_tooltip_text(Some("Apply all enabled mods by linking their files into the game directory"));
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

    refresh_library_content(&content_container, &game_rc, Rc::clone(&config));

    toolbar_view.set_content(Some(&content_container));

    wire_add_mod_button(&add_mod_btn, &game_rc, &content_container, Rc::clone(&config));

    // Wire Deploy button: re-enable all currently-enabled mods
    {
        let game_c = Rc::clone(&game_rc);
        let container_c = content_container.clone();
        let config_c = Rc::clone(&config);
        deploy_btn.connect_clicked(move |btn| {
            let db = ModDatabase::load(&game_c);
            let mut errors: Vec<String> = Vec::new();
            for m in db.mods.iter().filter(|m| m.enabled) {
                if let Err(e) = ModManager::enable_mod(&game_c, m) {
                    errors.push(format!("{}: {}", m.name, e));
                }
            }
            // Write plugins.txt after deploy
            let _ = db.write_plugins_txt(&game_c);

            let msg = if errors.is_empty() {
                format!("Deployed {} mod(s)", db.mods.iter().filter(|m| m.enabled).count())
            } else {
                for e in &errors {
                    log::error!("Deploy error: {e}");
                }
                format!("Deploy finished with {} error(s)", errors.len())
            };
            show_toast(btn.upcast_ref(), &msg);
            refresh_library_content(&container_c, &game_c, Rc::clone(&config_c));
        });
    }

    // Wire Undeploy button: remove all mod symlinks from the game directory
    {
        let game_c = Rc::clone(&game_rc);
        let container_c = content_container.clone();
        let config_c = Rc::clone(&config);
        undeploy_btn.connect_clicked(move |btn| {
            let db = ModDatabase::load(&game_c);
            let mut errors: Vec<String> = Vec::new();
            let mut count = 0;
            for m in db.mods.iter().filter(|m| m.enabled) {
                if let Err(e) = ModManager::disable_mod(&game_c, m) {
                    errors.push(format!("{}: {}", m.name, e));
                } else {
                    count += 1;
                }
            }

            // Mark all mods as disabled in the database
            let mut db = ModDatabase::load(&game_c);
            for m in db.mods.iter_mut() {
                m.enabled = false;
            }
            db.save(&game_c);
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
            refresh_library_content(&container_c, &game_c, Rc::clone(&config_c));
        });
    }

    toolbar_view.upcast()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn refresh_library_content(container: &gtk4::Box, game: &Rc<Game>, config: Rc<RefCell<AppConfig>>) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let mut db = ModDatabase::load(game);
    db.scan_mods_dir(game);
    db.save(game);

    if db.mods.is_empty() {
        let status = adw::StatusPage::builder()
            .title("No Mods Installed")
            .description(&format!(
                "Add mods for {} by clicking the button below or dragging an archive onto this window.",
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

        for mod_entry in &db.mods {
            let row = build_mod_row(mod_entry, game, Rc::clone(&config));
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
        dialog.set_title("Select Mod Folder");

        let game2 = Rc::clone(&game_clone);
        let container2 = container_clone.clone();
        let config2 = Rc::clone(&config_clone);
        dialog.select_folder(parent.as_ref(), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    let mod_name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Unknown Mod".to_string());

                    let mut db = ModDatabase::load(&game2);
                    let new_mod = crate::core::mods::Mod::new(mod_name, path);
                    db.mods.push(new_mod);
                    db.save(&game2);
                    config2.borrow().save();

                    refresh_library_content(&container2, &game2, Rc::clone(&config2));
                }
            }
        });
    });
}

fn build_mod_row(
    mod_entry: &crate::core::mods::Mod,
    game: &Rc<Game>,
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
    let config_clone = Rc::clone(&config);
    let reverting = Rc::new(RefCell::new(false));

    row.connect_active_notify(move |switch_row| {
        if *reverting.borrow() {
            return;
        }
        let enabled = switch_row.is_active();
        let mut db = ModDatabase::load(&game_clone);

        if let Some(m) = db.mods.iter_mut().find(|m| m.id == mod_id) {
            let result = if enabled {
                ModManager::enable_mod(&game_clone, m)
            } else {
                ModManager::disable_mod(&game_clone, m)
            };

            match result {
                Ok(()) => {
                    m.enabled = enabled;
                    db.save(&game_clone);
                    config_clone.borrow().save();
                }
                Err(e) => {
                    log::error!("Failed to toggle mod: {e}");
                    *reverting.borrow_mut() = true;
                    switch_row.set_active(!enabled);
                    *reverting.borrow_mut() = false;
                }
            }
        }
    });

    row
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
