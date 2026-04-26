use std::cell::RefCell;
use std::rc::Rc;

use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::Game;
use crate::core::umu;

use super::GameSelection;

pub(super) fn build_nexus_page(
    wizard_window: &adw::Window,
    config: Rc<RefCell<AppConfig>>,
    selected_game: Rc<RefCell<Option<GameSelection>>>,
    selected_app_dir: Rc<RefCell<Option<std::path::PathBuf>>>,
    on_finish: Rc<dyn Fn()>,
) -> gtk4::Box {
    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    page.set_vexpand(true);
    page.set_margin_start(24);
    page.set_margin_end(24);
    page.set_margin_top(24);
    page.set_margin_bottom(24);

    let header_label = gtk4::Label::new(Some("NexusMods API Key"));
    header_label.add_css_class("title-1");
    header_label.set_halign(gtk4::Align::Start);
    page.append(&header_label);

    let existing_key = config.borrow().nexus_api_key.clone();
    let (desc_text, desc_class) = if existing_key.is_some() {
        (
            "Your NexusMods API key is already configured. You can keep it or enter a new one.",
            Some("success"),
        )
    } else {
        (
            "Enter your NexusMods API key to browse and download mods.",
            None,
        )
    };

    let desc_label = gtk4::Label::new(Some(desc_text));
    desc_label.set_wrap(true);
    desc_label.set_halign(gtk4::Align::Start);
    if let Some(css) = desc_class {
        desc_label.add_css_class(css);
    }
    page.append(&desc_label);

    let link_btn = gtk4::LinkButton::builder()
        .label("Get your API key on NexusMods")
        .uri("https://www.nexusmods.com/users/myaccount?tab=api+access")
        .halign(gtk4::Align::Start)
        .build();
    page.append(&link_btn);

    let prefs_group = adw::PreferencesGroup::new();
    let api_key_row = adw::PasswordEntryRow::builder().title("API Key").build();

    // Pre-fill the key if one is already configured
    if let Some(ref key) = existing_key {
        api_key_row.set_text(key);
    }

    prefs_group.add(&api_key_row);
    page.append(&prefs_group);

    // Validate button
    let validate_btn = gtk4::Button::with_label("Validate Key");
    validate_btn.set_halign(gtk4::Align::Center);
    validate_btn.set_margin_top(8);
    page.append(&validate_btn);

    let status_label = gtk4::Label::new(None);
    status_label.set_halign(gtk4::Align::Center);
    page.append(&status_label);

    {
        let api_key_row_clone = api_key_row.clone();
        let status_label_clone = status_label.clone();
        validate_btn.connect_clicked(move |_| {
            let key = api_key_row_clone.text().to_string();
            if key.is_empty() {
                status_label_clone.set_text("Please enter an API key.");
                return;
            }
            status_label_clone.set_text("Validating\u{2026}");

            let (tx, rx) =
                std::sync::mpsc::channel::<Result<crate::core::nexus::NexusUser, String>>();

            std::thread::spawn(move || {
                let client = crate::core::nexus::NexusClient::new(&key);
                let _ = tx.send(client.validate());
            });

            let status_label2 = status_label_clone.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                match rx.try_recv() {
                    Ok(Ok(user)) => {
                        status_label2.set_text(&format!(
                            "\u{2714} Logged in as {} ({})",
                            user.username,
                            if user.is_premium { "Premium" } else { "Member" }
                        ));
                        glib::ControlFlow::Break
                    }
                    Ok(Err(e)) => {
                        status_label2.set_text(&format!("\u{2718} Validation failed: {e}"));
                        glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        status_label2.set_text("\u{2718} Validation error");
                        glib::ControlFlow::Break
                    }
                }
            });
        });
    }

    // Spacer
    let spacer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    page.append(&spacer);

    // Bottom buttons
    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btn_box.set_halign(gtk4::Align::End);

    let skip_btn = gtk4::Button::with_label("Skip");
    let finish_btn = gtk4::Button::with_label("Finish");
    finish_btn.add_css_class("suggested-action");

    btn_box.append(&skip_btn);
    btn_box.append(&finish_btn);
    page.append(&btn_box);

    // Skip handler
    {
        let wizard_window_clone = wizard_window.clone();
        let config_clone = Rc::clone(&config);
        let selected_game_clone = Rc::clone(&selected_game);
        let selected_app_dir_clone = Rc::clone(&selected_app_dir);
        let on_finish_clone = Rc::clone(&on_finish);
        skip_btn.connect_clicked(move |_| {
            finish_wizard(
                &wizard_window_clone,
                Rc::clone(&config_clone),
                Rc::clone(&selected_game_clone),
                Rc::clone(&selected_app_dir_clone),
                None,
                Rc::clone(&on_finish_clone),
            );
        });
    }

    // Finish handler
    {
        let wizard_window_clone = wizard_window.clone();
        let config_clone = Rc::clone(&config);
        let selected_game_clone = Rc::clone(&selected_game);
        let selected_app_dir_clone = Rc::clone(&selected_app_dir);
        let api_key_row_clone = api_key_row.clone();
        let on_finish_clone = Rc::clone(&on_finish);
        finish_btn.connect_clicked(move |_| {
            let key = api_key_row_clone.text().to_string();
            let api_key = if key.is_empty() { None } else { Some(key) };
            finish_wizard(
                &wizard_window_clone,
                Rc::clone(&config_clone),
                Rc::clone(&selected_game_clone),
                Rc::clone(&selected_app_dir_clone),
                api_key,
                Rc::clone(&on_finish_clone),
            );
        });
    }

    page
}

pub(super) fn finish_wizard(
    wizard_window: &adw::Window,
    config: Rc<RefCell<AppConfig>>,
    selected_game: Rc<RefCell<Option<GameSelection>>>,
    selected_app_dir: Rc<RefCell<Option<std::path::PathBuf>>>,
    api_key: Option<String>,
    on_finish: Rc<dyn Fn()>,
) {
    let selection = selected_game.borrow().clone();
    let is_umu = matches!(selection, Some(GameSelection::Umu { .. }));

    if is_umu {
        let progress_dialog = adw::Window::builder()
            .title("Downloading UMU Launcher")
            .modal(true)
            .transient_for(wizard_window)
            .default_width(400)
            .default_height(150)
            .deletable(false)
            .build();

        let pbox = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        pbox.set_margin_start(24);
        pbox.set_margin_end(24);
        pbox.set_margin_top(24);
        pbox.set_margin_bottom(24);
        pbox.set_valign(gtk4::Align::Center);

        let plabel = gtk4::Label::new(Some(if umu::is_umu_available() {
            "Checking for umu-launcher updates\u{2026}"
        } else {
            "Downloading umu-launcher\u{2026}"
        }));
        let pbar = gtk4::ProgressBar::new();
        pbar.set_show_text(true);

        pbox.append(&plabel);
        pbox.append(&pbar);
        progress_dialog.set_content(Some(&pbox));
        progress_dialog.present();

        let installed_version = config.borrow().umu_installed_version.clone();
        let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();

        std::thread::spawn(move || {
            let result =
                umu::ensure_umu_available(installed_version.as_deref(), |_downloaded, _total| true);
            let _ = tx.send(result.map(|(tag, _path)| tag));
        });

        let wizard_window_c = wizard_window.clone();
        let config_c = Rc::clone(&config);
        let selected_game_c = Rc::clone(&selected_game);
        let selected_app_dir_c = Rc::clone(&selected_app_dir);
        let on_finish_c = Rc::clone(&on_finish);
        let progress_dialog_c = progress_dialog.clone();
        let pbar_c = pbar.clone();

        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            pbar_c.pulse();
            match rx.try_recv() {
                Ok(Ok(new_tag)) => {
                    progress_dialog_c.destroy();
                    config_c.borrow_mut().umu_installed_version = Some(new_tag);
                    finish_wizard_apply(
                        &wizard_window_c,
                        Rc::clone(&config_c),
                        Rc::clone(&selected_game_c),
                        Rc::clone(&selected_app_dir_c),
                        api_key.clone(),
                        Rc::clone(&on_finish_c),
                    );
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    progress_dialog_c.destroy();
                    log::error!("Failed to download umu-launcher: {e}");
                    finish_wizard_apply(
                        &wizard_window_c,
                        Rc::clone(&config_c),
                        Rc::clone(&selected_game_c),
                        Rc::clone(&selected_app_dir_c),
                        api_key.clone(),
                        Rc::clone(&on_finish_c),
                    );
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    progress_dialog_c.destroy();
                    finish_wizard_apply(
                        &wizard_window_c,
                        Rc::clone(&config_c),
                        Rc::clone(&selected_game_c),
                        Rc::clone(&selected_app_dir_c),
                        api_key.clone(),
                        Rc::clone(&on_finish_c),
                    );
                    glib::ControlFlow::Break
                }
            }
        });

        return;
    }

    finish_wizard_apply(
        wizard_window,
        config,
        selected_game,
        selected_app_dir,
        api_key,
        on_finish,
    );
}

fn finish_wizard_apply(
    wizard_window: &adw::Window,
    config: Rc<RefCell<AppConfig>>,
    selected_game: Rc<RefCell<Option<GameSelection>>>,
    selected_app_dir: Rc<RefCell<Option<std::path::PathBuf>>>,
    api_key: Option<String>,
    on_finish: Rc<dyn Fn()>,
) {
    {
        let mut cfg = config.borrow_mut();
        cfg.first_run = false;

        if let Some(key) = api_key {
            cfg.nexus_api_key = Some(key);
        }

        let game_opt: Option<Game> = match selected_game.borrow().clone() {
            Some(GameSelection::Steam { kind, app_id, path }) => {
                Some(Game::new_steam_with_app_id(kind, path, app_id))
            }
            Some(GameSelection::Umu {
                kind,
                root_path,
                umu_cfg,
            }) => Some(Game::new_non_steam_umu(kind, root_path, umu_cfg)),
            None => None,
        };

        if let Some(game) = game_opt {
            let game_id = game.id.clone();
            if !cfg.games.iter().any(|g| g.id == game_id) {
                cfg.games.push(game);
            }
            cfg.current_game_id = Some(game_id.clone());

            let gs = cfg.game_settings_mut(&game_id);
            if let Some(app_dir) = selected_app_dir.borrow().clone() {
                gs.app_data_dir = Some(app_dir);
            }
        }

        cfg.apply_mods_base_dirs();
        cfg.save();
    }

    wizard_window.destroy();
    on_finish();
}
