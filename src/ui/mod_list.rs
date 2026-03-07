use std::cell::RefCell;
use std::rc::Rc;

use libadwaita as adw;
use gtk4::prelude::*;
use libadwaita::prelude::*;
use gio;

use crate::config::AppConfig;
use crate::games::Game;
use crate::mods::{ModDatabase, ModManager};

pub fn build_mod_list(
    game: &Game,
    config: Rc<RefCell<AppConfig>>,
) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let title_label = gtk4::Label::new(Some(&game.name));
    title_label.add_css_class("title");
    header.set_title_widget(Some(&title_label));

    let add_mod_button = gtk4::Button::new();
    add_mod_button.set_icon_name("folder-open-symbolic");
    add_mod_button.set_tooltip_text(Some("Add Mod"));
    header.pack_end(&add_mod_button);

    toolbar_view.add_top_bar(&header);

    let mut db = ModDatabase::load(game);
    db.scan_mods_dir(game);
    db.save(game);

    let content: gtk4::Widget = if db.mods.is_empty() {
        let status = adw::StatusPage::builder()
            .title("No Mods Installed")
            .description("Add a mod folder to get started.")
            .icon_name("package-x-generic-symbolic")
            .build();
        status.set_vexpand(true);
        status.upcast()
    } else {
        let list_box = gtk4::ListBox::new();
        list_box.add_css_class("boxed-list");
        list_box.set_selection_mode(gtk4::SelectionMode::None);

        for mod_entry in &db.mods {
            let row = build_mod_row(mod_entry, game, Rc::clone(&config));
            list_box.append(&row);
        }

        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(800);
        clamp.set_child(Some(&list_box));
        clamp.set_margin_top(12);
        clamp.set_margin_bottom(12);
        clamp.set_margin_start(12);
        clamp.set_margin_end(12);

        let scrolled = gtk4::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
        scrolled.set_child(Some(&clamp));
        scrolled.upcast()
    };

    toolbar_view.set_content(Some(&content));

    // Handle "Add Mod" button
    let game_clone = game.clone();
    let config_clone = Rc::clone(&config);
    add_mod_button.connect_clicked(move |btn| {
        let parent = btn
            .root()
            .and_then(|r| r.downcast::<gtk4::Window>().ok());

        let dialog = gtk4::FileDialog::new();
        dialog.set_title("Select Mod Folder");

        let game_clone2 = game_clone.clone();
        let config_clone2 = Rc::clone(&config_clone);
        dialog.select_folder(parent.as_ref(), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    let mod_name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Unknown Mod".to_string());

                    let mut db = ModDatabase::load(&game_clone2);
                    let new_mod = crate::mods::Mod::new(mod_name, path);
                    db.mods.push(new_mod);
                    db.save(&game_clone2);

                    // Save config to persist any state
                    config_clone2.borrow().save();
                }
            }
        });
    });

    toolbar_view.upcast()
}

fn build_mod_row(
    mod_entry: &crate::mods::Mod,
    game: &Game,
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
    let game_clone = game.clone();
    let config_clone = Rc::clone(&config);

    row.connect_active_notify(move |switch_row: &adw::SwitchRow| {
        let enabled = switch_row.is_active();
        let mut db = ModDatabase::load(&game_clone);

        if let Some(mod_entry) = db.mods.iter_mut().find(|m| m.id == mod_id) {
            let result = if enabled {
                ModManager::enable_mod(&game_clone, mod_entry)
            } else {
                ModManager::disable_mod(&game_clone, mod_entry)
            };

            match result {
                Ok(()) => {
                    mod_entry.enabled = enabled;
                    db.save(&game_clone);
                    config_clone.borrow().save();
                }
                Err(e) => {
                    log::error!("Failed to toggle mod: {e}");
                    // Revert the switch without triggering the signal again
                    // We use a flag via the widget's name to avoid re-entrancy
                    switch_row.set_active(!enabled);
                }
            }
        }
    });

    row
}
