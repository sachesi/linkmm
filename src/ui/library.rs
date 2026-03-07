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
///
/// The returned widget is an `adw::ToolbarView` with its own `adw::HeaderBar`
/// (so it shows the window title-buttons on the content side of the split-view).
pub fn build_library_page(game: &Game, config: Rc<RefCell<AppConfig>>) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    // "Library / <Game Name>" centred title
    let title_widget = adw::WindowTitle::new("Library", &game.name);
    header.set_title_widget(Some(&title_widget));

    // Search button (placeholder)
    let search_btn = gtk4::Button::new();
    search_btn.set_icon_name("system-search-symbolic");
    search_btn.set_tooltip_text(Some("Search mods"));
    header.pack_start(&search_btn);

    // Deploy button
    let deploy_btn = gtk4::Button::with_label("Deploy");
    deploy_btn.add_css_class("suggested-action");
    deploy_btn.set_tooltip_text(Some("Apply all enabled mods"));
    header.pack_end(&deploy_btn);

    toolbar_view.add_top_bar(&header);

    // Scrollable content container that can be refreshed
    let content_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content_container.set_vexpand(true);

    let game_rc = Rc::new(game.clone());

    refresh_library_content(&content_container, &game_rc, Rc::clone(&config));

    toolbar_view.set_content(Some(&content_container));

    // "Add Mod" via header button (for when the list is not empty)
    let add_mod_btn = gtk4::Button::new();
    add_mod_btn.set_icon_name("list-add-symbolic");
    add_mod_btn.set_tooltip_text(Some("Add Mod"));
    header.pack_end(&add_mod_btn);

    wire_add_mod_button(&add_mod_btn, &game_rc, &content_container, Rc::clone(&config));

    toolbar_view.upcast()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Re-populate `container` from the current mod database for `game`.
fn refresh_library_content(container: &gtk4::Box, game: &Rc<Game>, config: Rc<RefCell<AppConfig>>) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let mut db = ModDatabase::load(game);
    db.scan_mods_dir(game);
    db.save(game);

    if db.mods.is_empty() {
        // Empty-state with "Add Mod…" button
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

/// Connect a button so it opens a folder-picker and adds the chosen folder as a new mod.
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

    if let Some(version) = &mod_entry.version {
        row.set_subtitle(version.as_str());
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
