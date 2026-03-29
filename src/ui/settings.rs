use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::{AppConfig, Profile};
use crate::core::nexus::NexusClient;

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

    // Toast overlay so validation results and profile actions can show inline
    // notifications without needing the old dialog's toast mechanism.
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
                        toast_overlay_c2.add_toast(adw::Toast::new(&format!(
                            "Validation failed: {e}"
                        )));
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
    content_box.append(&nexus_group);

    // ── Profiles group ────────────────────────────────────────────────────
    let profiles_group = adw::PreferencesGroup::builder()
        .title("Mod Profiles")
        .description("Profiles let you save and switch between different mod configurations.")
        .build();

    let add_profile_btn = gtk4::Button::new();
    add_profile_btn.set_icon_name("list-add-symbolic");
    add_profile_btn.add_css_class("flat");
    add_profile_btn.set_tooltip_text(Some("Add Profile"));
    profiles_group.set_header_suffix(Some(&add_profile_btn));

    let profiles_list = gtk4::ListBox::new();
    profiles_list.add_css_class("boxed-list");
    profiles_list.set_selection_mode(gtk4::SelectionMode::None);
    profiles_group.add(&profiles_list);

    let rebuild: Rc<RefCell<Box<dyn Fn()>>> = Rc::new(RefCell::new(Box::new(|| {})));
    let rebuild_weak = Rc::downgrade(&rebuild);

    {
        let profiles_list_c = profiles_list.clone();
        let config_c = Rc::clone(&config);
        let rebuild_weak_c = rebuild_weak.clone();
        let toast_overlay_c = toast_overlay.clone();

        *rebuild.borrow_mut() = Box::new(move || {
            while let Some(child) = profiles_list_c.first_child() {
                profiles_list_c.remove(&child);
            }

            // Load profiles from per-game settings for the active game.
            let (profiles, active_id) = {
                let cfg = config_c.borrow();
                if let Some(game_id) = cfg.current_game_id.as_deref() {
                    let gs = cfg.game_settings_ref(game_id)
                        .cloned()
                        .unwrap_or_default();
                    (gs.profiles, gs.active_profile_id)
                } else {
                    (crate::core::config::default_active_profile_id_vec(), "default".to_string())
                }
            };

            for profile in &profiles {
                let is_active = profile.id == active_id;

                let row = adw::ActionRow::builder()
                    .title(&profile.name)
                    .activatable(!is_active)
                    .build();

                if is_active {
                    let icon = gtk4::Image::from_icon_name("object-select-symbolic");
                    row.add_suffix(&icon);
                    row.set_subtitle("Active");
                } else {
                    let del_btn = gtk4::Button::new();
                    del_btn.set_icon_name("edit-delete-symbolic");
                    del_btn.add_css_class("flat");
                    del_btn.set_valign(gtk4::Align::Center);
                    del_btn.set_tooltip_text(Some("Delete this profile"));

                    let profile_id_d = profile.id.clone();
                    let config_d = Rc::clone(&config_c);
                    let rebuild_d = rebuild_weak_c.clone();
                    del_btn.connect_clicked(move |_| {
                        let mut cfg = config_d.borrow_mut();
                        if let Some(game_id) = cfg.current_game_id.clone() {
                            cfg.game_settings_mut(&game_id)
                                .profiles
                                .retain(|p| p.id != profile_id_d);
                        }
                        cfg.save();
                        drop(cfg);
                        if let Some(rb) = rebuild_d.upgrade() {
                            (rb.borrow())();
                        }
                    });
                    row.add_suffix(&del_btn);

                    let profile_id_s = profile.id.clone();
                    let config_s = Rc::clone(&config_c);
                    let rebuild_s = rebuild_weak_c.clone();
                    let toast_overlay_s = toast_overlay_c.clone();
                    row.connect_activated(move |_| {
                        let mut cfg = config_s.borrow_mut();
                        if let Some(game_id) = cfg.current_game_id.clone() {
                            cfg.game_settings_mut(&game_id).active_profile_id =
                                profile_id_s.clone();
                        }
                        cfg.save();
                        drop(cfg);
                        toast_overlay_s.add_toast(adw::Toast::new("Profile switched."));
                        if let Some(rb) = rebuild_s.upgrade() {
                            (rb.borrow())();
                        }
                    });
                }

                profiles_list_c.append(&row);
            }
        });
    }

    (rebuild.borrow())();

    {
        let rebuild_strong = Rc::clone(&rebuild);
        let config_c = Rc::clone(&config);
        let parent_c = parent_window.clone();

        add_profile_btn.connect_clicked(move |_| {
            show_add_profile_dialog(&parent_c, Rc::clone(&config_c), Rc::clone(&rebuild_strong));
        });
    }

    content_box.append(&profiles_group);

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

    clamp.set_child(Some(&content_box));
    scrolled.set_child(Some(&clamp));
    toast_overlay.set_child(Some(&scrolled));
    toolbar_view.set_content(Some(&toast_overlay));

    toolbar_view.upcast()
}

// ── Add-profile dialog ────────────────────────────────────────────────────────

fn show_add_profile_dialog(
    parent: &gtk4::Window,
    config: Rc<RefCell<AppConfig>>,
    rebuild: Rc<RefCell<Box<dyn Fn()>>>,
) {
    let add_dialog = adw::Window::builder()
        .title("Add Profile")
        .modal(true)
        .transient_for(parent)
        .default_width(400)
        .default_height(200)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content_box.set_margin_start(24);
    content_box.set_margin_end(24);
    content_box.set_margin_top(12);
    content_box.set_margin_bottom(12);

    let name_entry = gtk4::Entry::new();
    name_entry.set_placeholder_text(Some("Profile name"));
    name_entry.set_hexpand(true);
    content_box.append(&name_entry);

    let add_btn = gtk4::Button::with_label("Add");
    add_btn.add_css_class("suggested-action");
    add_btn.set_halign(gtk4::Align::End);
    content_box.append(&add_btn);

    toolbar_view.set_content(Some(&content_box));
    add_dialog.set_content(Some(&toolbar_view));

    // Confirm: add the profile and rebuild the list
    let do_add = {
        let config_c = Rc::clone(&config);
        let rebuild_c = Rc::clone(&rebuild);
        let name_entry_c = name_entry.clone();
        let add_dialog_c = add_dialog.clone();
        move || {
            let name = name_entry_c.text().trim().to_string();
            if !name.is_empty() {
                let profile = Profile::new(name);
                let mut cfg = config_c.borrow_mut();
                if let Some(game_id) = cfg.current_game_id.clone() {
                    cfg.game_settings_mut(&game_id).profiles.push(profile);
                }
                cfg.save();
                drop(cfg);
                (rebuild_c.borrow())();
            }
            add_dialog_c.destroy();
        }
    };

    {
        let do_add_btn = do_add.clone();
        add_btn.connect_clicked(move |_| do_add_btn());
    }

    // Also confirm when the user presses Enter in the text field
    name_entry.connect_activate(move |_| do_add());

    add_dialog.present();
}
