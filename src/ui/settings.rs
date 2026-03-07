use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;

/// Show the Preferences / Settings dialog.
pub fn show_settings_dialog(parent: &gtk4::Window, config: Rc<RefCell<AppConfig>>) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Preferences");

    // ── General page ──────────────────────────────────────────────────────
    let general_page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();

    // NexusMods group
    let nexus_group = adw::PreferencesGroup::builder()
        .title("NexusMods")
        .description("Your NexusMods API key is used to browse and download mods.")
        .build();

    let api_key_row = adw::PasswordEntryRow::builder()
        .title("API Key")
        .build();

    if let Some(key) = config.borrow().nexus_api_key.as_deref() {
        api_key_row.set_text(key);
    }

    // Save API key when the entry loses focus / text changes
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

    nexus_group.add(&api_key_row);
    general_page.add(&nexus_group);
    dialog.add(&general_page);

    // ── Profiles page ─────────────────────────────────────────────────────
    let profiles_page = adw::PreferencesPage::builder()
        .title("Profiles")
        .icon_name("people-symbolic")
        .build();

    let profiles_group = adw::PreferencesGroup::builder()
        .title("Mod Profiles")
        .description(
            "Profiles let you save and switch between different mod configurations.",
        )
        .build();

    // "Add Profile" button in the group header
    let add_profile_btn = gtk4::Button::new();
    add_profile_btn.set_icon_name("list-add-symbolic");
    add_profile_btn.add_css_class("flat");
    add_profile_btn.set_tooltip_text(Some("Add Profile"));
    profiles_group.set_header_suffix(Some(&add_profile_btn));

    // Default profile row
    let default_row = adw::ActionRow::builder()
        .title("Default")
        .subtitle("Active profile")
        .build();
    let active_icon = gtk4::Image::from_icon_name("object-select-symbolic");
    default_row.add_suffix(&active_icon);
    profiles_group.add(&default_row);

    // Placeholder: show a note that profile switching is coming soon
    let coming_soon = adw::ActionRow::builder()
        .title("More profiles")
        .subtitle("Profile management will be expanded in a future release.")
        .build();
    profiles_group.add(&coming_soon);

    profiles_page.add(&profiles_group);
    dialog.add(&profiles_page);

    dialog.present(Some(parent));
}
