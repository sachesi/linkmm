use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;

use gio;
use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::{GameLauncherSource, UmuGameConfig};
use crate::core::nexus::NexusClient;
use crate::core::runtime::build_phase1_steam_launch_option;
use crate::core::steam::{clear_launch_options, install_launch_options, read_launch_options};
use crate::core::umu;

/// Build the inline Preferences page shown as a tab in the main window.
pub fn build_settings_page(
    config: Rc<RefCell<AppConfig>>,
    parent_window: &gtk4::Window,
) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let title_widget = adw::WindowTitle::new("Preferences", "");
    header.set_title_widget(Some(&title_widget));
    toolbar_view.add_top_bar(&header);

    // Toast overlay so validation results can show inline notifications.
    let toast_overlay = adw::ToastOverlay::new();

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(900);
    clamp.set_margin_top(12);
    clamp.set_margin_bottom(12);
    clamp.set_margin_start(12);
    clamp.set_margin_end(12);

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 24);

    // ── NexusMods group ───────────────────────────────────────────────────
    let nexus_group = adw::PreferencesGroup::builder()
        .title("NexusMods")
        .description("Your NexusMods API key is used to browse and download mods.")
        .build();

    let validate_btn = gtk4::Button::with_label("Validate");
    validate_btn.add_css_class("suggested-action");
    validate_btn.set_valign(gtk4::Align::Center);
    validate_btn.set_tooltip_text(Some("Test your API key against the NexusMods API"));
    nexus_group.set_header_suffix(Some(&validate_btn));

    let api_key_row = adw::PasswordEntryRow::builder()
        .title("API Key")
        .show_apply_button(true)
        .build();

    let get_key_btn = gtk4::Button::builder()
        .label("Get your API key on NexusMods")
        .halign(gtk4::Align::Start)
        .css_classes(["flat", "accent"])
        .build();
    get_key_btn.connect_clicked(|_| {
        let _ = gtk4::gio::AppInfo::launch_default_for_uri(
            "https://www.nexusmods.com/settings/api-keys",
            None::<&gtk4::gio::AppLaunchContext>,
        );
    });

    let update_link_visibility = {
        let btn = get_key_btn.clone();
        let entry = api_key_row.clone();
        move || {
            btn.set_visible(entry.text().is_empty());
        }
    };

    api_key_row.connect_changed({
        let update = update_link_visibility.clone();
        move |_| update()
    });

    if let Some(key) = config.borrow().nexus_api_key.as_deref() {
        api_key_row.set_text(key);
    }
    update_link_visibility();

    {
        let config_clone = Rc::clone(&config);
        let api_key_row_clone = api_key_row.clone();
        api_key_row.connect_apply(move |_| {
            let key = api_key_row_clone.text().to_string();
            let mut cfg = config_clone.borrow_mut();
            cfg.nexus_api_key = if key.is_empty() { None } else { Some(key) };
            cfg.save();
        });
    }

    {
        let config_c = Rc::clone(&config);
        let api_key_row_c = api_key_row.clone();
        let toast_overlay_c = toast_overlay.clone();
        validate_btn.connect_clicked(move |btn| {
            let key = api_key_row_c.text().to_string();
            if key.is_empty() {
                toast_overlay_c.add_toast(adw::Toast::new("Please enter an API key first."));
                return;
            }

            {
                let mut cfg = config_c.borrow_mut();
                cfg.nexus_api_key = Some(key.clone());
                cfg.save();
            }

            btn.set_sensitive(false);

            let (tx, rx) = mpsc::channel::<Result<String, String>>();
            std::thread::spawn(move || {
                let result = NexusClient::new(&key).validate().map(|u| u.username);
                let _ = tx.send(result);
            });

            let btn_c = btn.clone();
            let toast_overlay_c2 = toast_overlay_c.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                match rx.try_recv() {
                    Ok(Ok(username)) => {
                        btn_c.set_sensitive(true);
                        toast_overlay_c2.add_toast(adw::Toast::new(&format!(
                            "API key valid — logged in as {username}."
                        )));
                        glib::ControlFlow::Break
                    }
                    Ok(Err(e)) => {
                        btn_c.set_sensitive(true);
                        toast_overlay_c2
                            .add_toast(adw::Toast::new(&format!("Validation failed: {e}")));
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        btn_c.set_sensitive(true);
                        glib::ControlFlow::Break
                    }
                }
            });
        });
    }

    nexus_group.add(&api_key_row);
    nexus_group.add(&get_key_btn);
    content_box.append(&nexus_group);

    // ── Debug Logging group ───────────────────────────────────────────────
    let logging_group = adw::PreferencesGroup::builder()
        .title("Debug Logging")
        .description("Choose which log categories are shown in the log viewer.")
        .build();

    // Helper that creates a toggle row wired to a config bool field.
    macro_rules! log_toggle_row {
        ($title:expr, $subtitle:expr, $field:ident) => {{
            let row = adw::SwitchRow::builder()
                .title($title)
                .subtitle($subtitle)
                .build();
            row.set_active(config.borrow().$field);
            let config_t = Rc::clone(&config);
            row.connect_active_notify(move |r| {
                let mut cfg = config_t.borrow_mut();
                cfg.$field = r.is_active();
                cfg.save();
            });
            row
        }};
    }

    let activity_row = log_toggle_row!(
        "Mod Activity",
        "Show mod download and installation progress in the log viewer",
        log_activity
    );
    let warnings_row = log_toggle_row!(
        "Warnings",
        "Show warning messages in the log viewer",
        log_warnings
    );
    let errors_row = log_toggle_row!(
        "Errors",
        "Show error messages in the log viewer",
        log_errors
    );
    let info_row = log_toggle_row!(
        "Info",
        "Show info-level messages in the log viewer",
        log_info
    );
    let debug_row = log_toggle_row!(
        "Debug",
        "Show debug-level messages in the log viewer",
        log_debug
    );

    logging_group.add(&activity_row);
    logging_group.add(&warnings_row);
    logging_group.add(&errors_row);
    logging_group.add(&info_row);
    logging_group.add(&debug_row);
    content_box.append(&logging_group);

    let current_steam_game = {
        let cfg = config.borrow();
        cfg.current_game().cloned().filter(|g| {
            g.launcher_source == GameLauncherSource::Steam
                && g.kind.is_phase1_steam_redirector_target()
        })
    };

    if let Some(game) = current_steam_game {
        let steam_group = adw::PreferencesGroup::builder()
            .title("Steam Redirector")
            .description(
                "Phase 1 Steam redirector support is currently limited to Skyrim Special Edition / Anniversary Edition on native Steam.",
            )
            .build();

        let app_id = game.steam_instance_app_id().unwrap_or(489830);
        let launch_option_value = build_phase1_steam_launch_option(app_id, &game.id)
            .unwrap_or_else(|e| format!("Unable to build launch option: {e}"));
        let launch_row = adw::ActionRow::builder()
            .title("Steam Launch Option")
            .subtitle(&launch_option_value)
            .build();
        launch_row.set_tooltip_text(Some(
            "This launch option is tied to the exact configured game instance shown in linkmm.",
        ));
        let copy_btn = gtk4::Button::with_label("Copy");
        copy_btn.add_css_class("flat");
        launch_row.add_suffix(&copy_btn);
        let install_btn = gtk4::Button::with_label("Install in Steam");
        install_btn.add_css_class("flat");
        launch_row.add_suffix(&install_btn);
        let clear_btn = gtk4::Button::with_label("Clear from Steam");
        clear_btn.add_css_class("flat");
        launch_row.add_suffix(&clear_btn);
        let installed_status = match read_launch_options(app_id) {
            Ok(Some(installed)) if installed == launch_option_value => {
                "Installed launch option matches this game instance.".to_string()
            }
            Ok(Some(_)) => {
                "Steam currently has a different launch option for this game.".to_string()
            }
            Ok(None) => "No Steam launch option is currently installed for this game.".to_string(),
            Err(e) => format!("Could not read Steam launch option: {e}"),
        };
        let installed_row = adw::ActionRow::builder()
            .title("Installed State")
            .subtitle(&installed_status)
            .build();
        {
            let subtitle = launch_row
                .subtitle()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let toast_c = toast_overlay.clone();
            copy_btn.connect_clicked(move |_| {
                if let Some(display) = gtk4::gdk::Display::default() {
                    display.clipboard().set_text(&subtitle);
                    toast_c.add_toast(adw::Toast::new("Steam launch option copied."));
                } else {
                    toast_c.add_toast(adw::Toast::new("Could not access clipboard."));
                }
            });
        }
        {
            let launch_option = launch_option_value.clone();
            let installed_row_c = installed_row.clone();
            let toast_c = toast_overlay.clone();
            install_btn.connect_clicked(move |_| {
                match install_launch_options(app_id, &launch_option) {
                    Ok(updated) => {
                        installed_row_c
                            .set_subtitle("Installed launch option matches this game instance.");
                        toast_c.add_toast(adw::Toast::new(&format!(
                            "Installed Steam launch option in {updated} Steam user config(s)."
                        )));
                    }
                    Err(e) => {
                        toast_c.add_toast(adw::Toast::new(&format!(
                            "Failed to install Steam launch option: {e}"
                        )));
                    }
                }
            });
        }
        {
            let installed_row_c = installed_row.clone();
            let toast_c = toast_overlay.clone();
            clear_btn.connect_clicked(move |_| match clear_launch_options(app_id) {
                Ok(updated) => {
                    installed_row_c.set_subtitle(
                        "No Steam launch option is currently installed for this game.",
                    );
                    toast_c.add_toast(adw::Toast::new(&format!(
                        "Cleared Steam launch option in {updated} Steam user config(s)."
                    )));
                }
                Err(e) => {
                    toast_c.add_toast(adw::Toast::new(&format!(
                        "Failed to clear Steam launch option: {e}"
                    )));
                }
            });
        }
        steam_group.add(&launch_row);
        steam_group.add(&installed_row);

        let target_names = game.kind.phase1_steam_launch_candidates();
        let mut target_labels = vec!["Automatic".to_string()];
        for exe in target_names {
            target_labels.push(game.kind.phase1_steam_target_label(exe).to_string());
        }
        let target_label_refs: Vec<&str> = target_labels.iter().map(|s| s.as_str()).collect();
        let target_model = gtk4::StringList::new(&target_label_refs);
        let target_row = adw::ComboRow::builder()
            .title("Steam Launch Target")
            .subtitle("Choose which executable linkmm starts after mounting the VFS.")
            .model(&target_model)
            .build();

        let saved_target = config
            .borrow()
            .game_settings_ref(&game.id)
            .and_then(|settings| settings.steam_redirect_exe.clone());
        let selected_index = saved_target
            .as_deref()
            .and_then(|saved| target_names.iter().position(|name| *name == saved))
            .map(|idx| idx as u32 + 1)
            .unwrap_or(0);
        target_row.set_selected(selected_index);
        {
            let config_c = Rc::clone(&config);
            let game_id = game.id.clone();
            let toast_c = toast_overlay.clone();
            target_row.connect_selected_notify(move |row| {
                let selected = row.selected();
                let mut cfg = config_c.borrow_mut();
                let settings = cfg.game_settings_mut(&game_id);
                settings.steam_redirect_exe = if selected == 0 {
                    None
                } else {
                    target_names
                        .get((selected - 1) as usize)
                        .map(|name| (*name).to_string())
                };
                cfg.save();
                toast_c.add_toast(adw::Toast::new("Steam launch target saved."));
            });
        }
        steam_group.add(&target_row);

        let native_only_row = adw::ActionRow::builder()
            .title("Runtime Support")
            .subtitle("Native linkmm works only with native Steam. Flatpak Steam needs a Flatpak build of linkmm.")
            .build();
        steam_group.add(&native_only_row);

        content_box.append(&steam_group);
    }

    // ── UMU Launcher group ────────────────────────────────────────────────
    // Only shown when the currently active game was set up via UMU (non-Steam).
    let current_umu_config: Option<(String, UmuGameConfig)> = {
        let cfg = config.borrow();
        cfg.current_game().and_then(|g| {
            (g.launcher_source == GameLauncherSource::NonSteamUmu)
                .then(|| g.umu_config.as_ref().map(|u| (g.id.clone(), u.clone())))
                .flatten()
        })
    };

    if let Some((game_id_for_umu, umu_cfg)) = current_umu_config {
        let installed_version = config.borrow().umu_installed_version.clone();
        let version_label_text = match &installed_version {
            Some(v) => format!("Installed version: {v}"),
            None => "Not installed".to_string(),
        };

        let umu_group = adw::PreferencesGroup::builder()
            .title("UMU Launcher")
            .description(
                "umu-run launches this non-Steam game through Proton without requiring Steam. \
                 The latest release is downloaded automatically on startup.",
            )
            .build();

        // ── Version row with update button ────────────────────────────────
        let version_row = adw::ActionRow::builder()
            .title("umu-run Version")
            .subtitle(&version_label_text)
            .build();

        let update_btn = gtk4::Button::with_label("Check for Update");
        update_btn.set_valign(gtk4::Align::Center);
        update_btn.add_css_class("flat");
        version_row.add_suffix(&update_btn);

        {
            let config_c = Rc::clone(&config);
            let toast_c = toast_overlay.clone();
            let version_row_c = version_row.clone();
            let update_btn_c = update_btn.clone();
            update_btn.connect_clicked(move |_| {
                update_btn_c.set_sensitive(false);
                version_row_c.set_subtitle("Checking…");

                let (tx, rx) = mpsc::channel::<Result<String, String>>();

                std::thread::spawn(move || {
                    // Force a fresh check by passing None so the tag is
                    // always re-compared against the GitHub latest.
                    let result = umu::ensure_umu_available(
                        None, // ignore stored version → always re-download if tag changed
                        |_, _| true,
                    );
                    let _ = tx.send(result.map(|(tag, _)| tag));
                });

                let config_cc = Rc::clone(&config_c);
                let toast_cc = toast_c.clone();
                let version_row_cc = version_row_c.clone();
                let update_btn_cc = update_btn_c.clone();
                glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
                    match rx.try_recv() {
                        Ok(Ok(tag)) => {
                            let mut cfg = config_cc.borrow_mut();
                            cfg.umu_installed_version = Some(tag.clone());
                            cfg.save();
                            drop(cfg);
                            version_row_cc.set_subtitle(&format!("Installed version: {tag}"));
                            update_btn_cc.set_sensitive(true);
                            toast_cc
                                .add_toast(adw::Toast::new(&format!("umu-run updated to {tag}.")));
                            glib::ControlFlow::Break
                        }
                        Ok(Err(e)) => {
                            version_row_cc.set_subtitle("Update failed — see logs.");
                            update_btn_cc.set_sensitive(true);
                            toast_cc.add_toast(adw::Toast::new(&format!("Update failed: {e}")));
                            glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            update_btn_cc.set_sensitive(true);
                            glib::ControlFlow::Break
                        }
                    }
                });
            });
        }

        umu_group.add(&version_row);

        // ── Wine/Proton Prefix ────────────────────────────────────────────
        let prefix_row = adw::EntryRow::builder()
            .title("Wine/Proton Prefix — default: ~/.local/share/umu/default")
            .show_apply_button(true)
            .build();

        if let Some(ref p) = umu_cfg.prefix_path {
            prefix_row.set_text(&p.to_string_lossy());
        }

        let browse_prefix_btn = gtk4::Button::new();
        browse_prefix_btn.set_icon_name("folder-open-symbolic");
        browse_prefix_btn.set_valign(gtk4::Align::Center);
        browse_prefix_btn.set_tooltip_text(Some("Browse for prefix folder"));
        prefix_row.add_suffix(&browse_prefix_btn);

        {
            let prefix_row_c = prefix_row.clone();
            let parent_c = parent_window.clone();
            browse_prefix_btn.connect_clicked(move |_| {
                let fd = gtk4::FileDialog::new();
                fd.set_title("Select Wine/Proton Prefix Folder");
                let row_c = prefix_row_c.clone();
                fd.select_folder(Some(&parent_c), None::<&gio::Cancellable>, move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        row_c.set_text(&path.to_string_lossy());
                    }
                });
            });
        }

        {
            let config_c = Rc::clone(&config);
            let game_id_c = game_id_for_umu.clone();
            let toast_c = toast_overlay.clone();
            prefix_row.connect_apply(move |row| {
                let text = row.text().to_string();
                let new_prefix = if text.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(&text))
                };
                let mut cfg = config_c.borrow_mut();
                if let Some(ref p) = new_prefix
                    && !p.is_dir()
                {
                    toast_c.add_toast(adw::Toast::new("Prefix path must be an existing folder."));
                    return;
                }
                if let Some(game) = cfg.games.iter_mut().find(|g| g.id == game_id_c) {
                    if let Some(ref mut umu) = game.umu_config {
                        umu.prefix_path = new_prefix;
                    }
                }
                cfg.save();
                toast_c.add_toast(adw::Toast::new("Prefix saved."));
            });
        }

        umu_group.add(&prefix_row);

        // ── Proton Path ───────────────────────────────────────────────────
        let proton_row = adw::EntryRow::builder()
            .title("Proton Path — default: auto-download latest GE-Proton")
            .show_apply_button(true)
            .build();

        if let Some(ref p) = umu_cfg.proton_path {
            proton_row.set_text(&p.to_string_lossy());
        }

        let browse_proton_btn = gtk4::Button::new();
        browse_proton_btn.set_icon_name("folder-open-symbolic");
        browse_proton_btn.set_valign(gtk4::Align::Center);
        browse_proton_btn.set_tooltip_text(Some("Browse for Proton installation folder"));
        proton_row.add_suffix(&browse_proton_btn);

        {
            let proton_row_c = proton_row.clone();
            let parent_c = parent_window.clone();
            browse_proton_btn.connect_clicked(move |_| {
                let fd = gtk4::FileDialog::new();
                fd.set_title("Select Proton Installation Folder");
                let row_c = proton_row_c.clone();
                fd.select_folder(Some(&parent_c), None::<&gio::Cancellable>, move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        row_c.set_text(&path.to_string_lossy());
                    }
                });
            });
        }

        {
            let config_c = Rc::clone(&config);
            let game_id_c = game_id_for_umu.clone();
            let toast_c = toast_overlay.clone();
            proton_row.connect_apply(move |row| {
                let text = row.text().to_string();
                let new_proton = if text.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(&text))
                };
                let mut cfg = config_c.borrow_mut();
                if let Some(ref p) = new_proton
                    && !p.is_dir()
                {
                    toast_c.add_toast(adw::Toast::new("Proton path must be an existing folder."));
                    return;
                }
                if let Some(game) = cfg.games.iter_mut().find(|g| g.id == game_id_c) {
                    if let Some(ref mut umu) = game.umu_config {
                        umu.proton_path = new_proton;
                    }
                }
                cfg.save();
                toast_c.add_toast(adw::Toast::new("Proton path saved."));
            });
        }

        umu_group.add(&proton_row);

        let exe_row = adw::EntryRow::builder()
            .title("Game Executable (.exe)")
            .show_apply_button(true)
            .build();
        exe_row.set_text(&umu_cfg.exe_path.to_string_lossy());
        {
            let config_c = Rc::clone(&config);
            let game_id_c = game_id_for_umu.clone();
            let toast_c = toast_overlay.clone();
            exe_row.connect_apply(move |row| {
                let new_path = PathBuf::from(row.text().as_str());
                if !new_path.is_file() {
                    toast_c.add_toast(adw::Toast::new("Executable path must be an existing file."));
                    return;
                }
                let mut cfg = config_c.borrow_mut();
                if let Some(game) = cfg.games.iter_mut().find(|g| g.id == game_id_c)
                    && let Some(ref mut umu) = game.umu_config
                {
                    umu.exe_path = new_path;
                }
                cfg.save();
                toast_c.add_toast(adw::Toast::new("Executable saved."));
            });
        }
        umu_group.add(&exe_row);

        content_box.append(&umu_group);
    }

    clamp.set_child(Some(&content_box));
    scrolled.set_child(Some(&clamp));
    toast_overlay.set_child(Some(&scrolled));
    toolbar_view.set_content(Some(&toast_overlay));

    toolbar_view.upcast()
}
