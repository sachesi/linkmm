use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

use gio;
use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::Game;
use crate::core::nexus::{NexusClient, NexusModInfo};

// ── Public entry-point ────────────────────────────────────────────────────────

/// Build the Downloads page.
///
/// Shows tabs for Trending mods and Latest Added mods (fetched from the Nexus
/// API), plus a "Find by ID" panel for looking up a specific mod.
pub fn build_downloads_page(
    game: Option<&Game>,
    config: Rc<RefCell<AppConfig>>,
) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let title_widget = match game {
        Some(g) => adw::WindowTitle::new("Downloads", &g.name),
        None => adw::WindowTitle::new("Downloads", ""),
    };
    header.set_title_widget(Some(&title_widget));

    toolbar_view.add_top_bar(&header);

    let Some(game) = game else {
        let status = adw::StatusPage::builder()
            .title("No Game Selected")
            .description("Select a game from the sidebar to browse mods.")
            .icon_name("applications-games-symbolic")
            .build();
        status.set_vexpand(true);
        toolbar_view.set_content(Some(&status));
        return toolbar_view.upcast();
    };

    let api_key = config.borrow().nexus_api_key.clone();
    let Some(api_key) = api_key else {
        let status = adw::StatusPage::builder()
            .title("API Key Required")
            .description(
                "Set your Nexus Mods API key in Preferences to browse and download mods.",
            )
            .icon_name("dialog-password-symbolic")
            .build();
        status.set_vexpand(true);
        toolbar_view.set_content(Some(&status));
        return toolbar_view.upcast();
    };

    let game_domain = game.kind.nexus_game_id().to_string();
    let game_rc = Rc::new(game.clone());

    // ── Tab stack ─────────────────────────────────────────────────────────────
    let tab_stack = gtk4::Stack::new();
    tab_stack.set_transition_type(gtk4::StackTransitionType::SlideLeftRight);
    tab_stack.set_vexpand(true);

    let switcher = gtk4::StackSwitcher::new();
    switcher.set_stack(Some(&tab_stack));
    switcher.set_halign(gtk4::Align::Center);

    // Trending tab
    let trending_box = build_mod_list_tab();
    tab_stack.add_titled(&trending_box, Some("trending"), "Trending");

    // Latest Added tab
    let latest_box = build_mod_list_tab();
    tab_stack.add_titled(&latest_box, Some("latest"), "Latest Added");

    // Find by ID tab
    let find_box = build_find_by_id_tab(&game_rc, &api_key, Rc::clone(&config));
    tab_stack.add_titled(&find_box, Some("find"), "Find by ID");

    let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    outer.set_vexpand(true);

    // Switcher bar
    let switcher_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    switcher_box.set_margin_top(8);
    switcher_box.set_margin_bottom(8);
    switcher_box.append(&switcher);
    outer.append(&switcher_box);

    outer.append(&tab_stack);
    toolbar_view.set_content(Some(&outer));

    // Kick off initial data loads (network request → update list box on main thread)
    load_mod_list(&trending_box, &game_domain, &api_key, NexusListKind::Trending);
    load_mod_list(&latest_box, &game_domain, &api_key, NexusListKind::Latest);

    toolbar_view.upcast()
}

// ── Tab builders ──────────────────────────────────────────────────────────────

/// Create a vertically-scrolling box that will be filled by `load_mod_list`.
fn build_mod_list_tab() -> gtk4::Box {
    let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    outer.set_vexpand(true);

    // Spinner shown while loading
    let spinner = gtk4::Spinner::new();
    spinner.set_spinning(true);
    spinner.set_halign(gtk4::Align::Center);
    spinner.set_valign(gtk4::Align::Center);
    spinner.set_margin_top(48);
    spinner.set_vexpand(true);
    outer.append(&spinner);

    outer
}

enum NexusListKind {
    Trending,
    Latest,
}

/// Spawn a background thread to fetch the mod list and populate `container`.
fn load_mod_list(container: &gtk4::Box, game_domain: &str, api_key: &str, kind: NexusListKind) {
    let (tx, rx) = mpsc::channel::<Result<Vec<NexusModInfo>, String>>();
    let gd = game_domain.to_string();
    let ak = api_key.to_string();

    std::thread::spawn(move || {
        let client = NexusClient::new(&ak);
        let result = match kind {
            NexusListKind::Trending => client.list_trending_mods(&gd),
            NexusListKind::Latest => client.list_latest_added_mods(&gd),
        };
        let _ = tx.send(result);
    });

    // Poll from main thread
    let container_c = container.clone();
    let gd2 = game_domain.to_string();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        match rx.try_recv() {
            Ok(Ok(mods)) => {
                populate_mod_list(&container_c, &mods, &gd2);
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                show_error_in_container(&container_c, &format!("Failed to fetch mods: {e}"));
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                show_error_in_container(&container_c, "Connection lost");
                glib::ControlFlow::Break
            }
        }
    });
}

/// Remove the spinner and fill `container` with mod cards.
fn populate_mod_list(container: &gtk4::Box, mods: &[NexusModInfo], game_domain: &str) {
    // Remove spinner
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    if mods.is_empty() {
        let status = adw::StatusPage::builder()
            .title("No Mods Found")
            .description("No mods were returned by the Nexus API for this game.")
            .icon_name("package-x-generic-symbolic")
            .build();
        status.set_vexpand(true);
        container.append(&status);
        return;
    }

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::None);

    for mod_info in mods {
        let row = build_mod_info_row(mod_info, game_domain);
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

fn show_error_in_container(container: &gtk4::Box, message: &str) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
    let status = adw::StatusPage::builder()
        .title("Error")
        .description(message)
        .icon_name("dialog-error-symbolic")
        .build();
    status.set_vexpand(true);
    container.append(&status);
}

// ── Mod info row ──────────────────────────────────────────────────────────────

fn build_mod_info_row(mod_info: &NexusModInfo, game_domain: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(&mod_info.name)
        .activatable(false)
        .build();

    // Subtitle: author + endorsements
    let mut parts: Vec<String> = Vec::new();
    if let Some(author) = &mod_info.author {
        parts.push(format!("by {author}"));
    }
    if mod_info.endorsement_count > 0 {
        parts.push(format!("★ {}", mod_info.endorsement_count));
    }
    if let Some(summary) = &mod_info.summary {
        let short: String = summary.chars().take(120).collect();
        parts.push(short);
    }
    if !parts.is_empty() {
        row.set_subtitle(&parts.join("  ·  "));
    }

    // "View on Nexus" button
    let url = NexusClient::mod_page_url(game_domain, mod_info.mod_id);
    let view_btn = gtk4::Button::new();
    view_btn.set_icon_name("web-browser-symbolic");
    view_btn.set_tooltip_text(Some("View on Nexus Mods"));
    view_btn.set_valign(gtk4::Align::Center);
    view_btn.add_css_class("flat");

    view_btn.connect_clicked(move |_| {
        if let Err(e) = gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>) {
            log::error!("Failed to open URL: {e}");
        }
    });

    row.add_suffix(&view_btn);
    row
}

// ── Find-by-ID tab ────────────────────────────────────────────────────────────

fn build_find_by_id_tab(
    game: &Rc<Game>,
    api_key: &str,
    _config: Rc<RefCell<AppConfig>>,
) -> gtk4::Box {
    let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    outer.set_margin_start(24);
    outer.set_margin_end(24);
    outer.set_margin_top(24);
    outer.set_margin_bottom(12);

    let header = gtk4::Label::new(Some("Look Up Mod by Nexus ID"));
    header.add_css_class("title-3");
    header.set_halign(gtk4::Align::Start);
    outer.append(&header);

    // Entry row
    let entry_group = adw::PreferencesGroup::new();
    let id_row = adw::EntryRow::builder()
        .title("Nexus Mod ID")
        .input_purpose(gtk4::InputPurpose::Digits)
        .build();
    entry_group.add(&id_row);
    outer.append(&entry_group);

    // Search button
    let search_btn = gtk4::Button::with_label("Search");
    search_btn.add_css_class("suggested-action");
    search_btn.set_halign(gtk4::Align::Center);
    outer.append(&search_btn);

    // Result area
    let result_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    result_box.set_vexpand(true);
    outer.append(&result_box);

    let game_domain = game.kind.nexus_game_id().to_string();
    let ak = api_key.to_string();

    search_btn.connect_clicked(move |_| {
        let text = id_row.text().to_string();
        let Ok(mod_id) = text.trim().parse::<u32>() else {
            show_error_in_container(&result_box, "Please enter a valid numeric mod ID.");
            return;
        };

        // Clear previous result and show spinner
        while let Some(child) = result_box.first_child() {
            result_box.remove(&child);
        }
        let spinner = gtk4::Spinner::new();
        spinner.set_spinning(true);
        spinner.set_halign(gtk4::Align::Center);
        spinner.set_margin_top(24);
        result_box.append(&spinner);

        let (tx, rx) = mpsc::channel::<Result<NexusModInfo, String>>();
        let gd = game_domain.clone();
        let ak2 = ak.clone();
        std::thread::spawn(move || {
            let client = NexusClient::new(&ak2);
            let result = client.get_mod(&gd, mod_id).map(|m| NexusModInfo {
                mod_id: m.mod_id,
                name: m.name,
                summary: m.summary,
                version: m.version,
                author: m.author,
                endorsement_count: 0,
                picture_url: None,
            });
            let _ = tx.send(result);
        });

        let rb = result_box.clone();
        let gd2 = game_domain.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            match rx.try_recv() {
                Ok(Ok(mod_info)) => {
                    while let Some(child) = rb.first_child() {
                        rb.remove(&child);
                    }
                    let list_box = gtk4::ListBox::new();
                    list_box.add_css_class("boxed-list");
                    list_box.set_selection_mode(gtk4::SelectionMode::None);
                    list_box.append(&build_mod_info_row(&mod_info, &gd2));
                    rb.append(&list_box);
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    show_error_in_container(&rb, &format!("Mod not found: {e}"));
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    show_error_in_container(&rb, "Connection lost");
                    glib::ControlFlow::Break
                }
            }
        });
    });

    outer
}
