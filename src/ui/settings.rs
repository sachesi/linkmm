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

    let get_key_row = adw::ActionRow::builder()
        .title("Get your API key on NexusMods")
        .activatable(true)
        .build();
    let external_icon = gtk4::Image::from_icon_name("external-link-symbolic");
    get_key_row.add_suffix(&external_icon);
    get_key_row.connect_activated(|_| {
        let _ = gtk4::gio::AppInfo::launch_default_for_uri("https://www.nexusmods.com/settings/api-keys", None::<&gtk4::gio::AppLaunchContext>);
    });

    if let Some(key) = config.borrow().nexus_api_key.as_deref() {
        api_key_row.set_text(key);
    }

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
    nexus_group.add(&get_key_row);
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


