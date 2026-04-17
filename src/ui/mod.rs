use std::cell::RefCell;

use std::rc::Rc;
use std::sync::mpsc;

use gio;
use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::GameLauncherSource;
use crate::core::mods::ModDatabase;
use crate::core::runtime::global_runtime_manager;

pub mod downloads;
pub mod library;
pub mod load_order;
pub mod logs;
pub mod mod_list;
pub mod ordering;
pub mod settings;
pub mod setup_wizard;
pub mod tools;

pub fn build_ui(app: &libadwaita::Application) {
    let config = Rc::new(RefCell::new(AppConfig::load_or_default()));
    let (window, on_setup_done) = build_main_window(app, Rc::clone(&config));
    window.present();
    if config.borrow().first_run || config.borrow().games.is_empty() {
        setup_wizard::show_setup_wizard(&window, Rc::clone(&config), on_setup_done);
    }
    // Ensure this app is registered as the NXM protocol handler so that
    // clicking "Download with NXM" in the browser sends links here.
    register_nxm_handler();

    // If any game is configured via UMU, check for a newer umu-run release in
    // the background and re-download automatically when the tag has changed.
    let has_umu_game = config
        .borrow()
        .games
        .iter()
        .any(|g| g.launcher_source == GameLauncherSource::NonSteamUmu);
    if has_umu_game {
        let installed_version = config.borrow().umu_installed_version.clone();
        let config_for_update = Rc::clone(&config);
        crate::core::umu::check_and_update_in_background(installed_version, move |new_tag| {
            let mut cfg = config_for_update.borrow_mut();
            cfg.umu_installed_version = Some(new_tag);
            cfg.save();
        });
    }
}

// ── Navigation constants ───────────────────────────────────────────────────

const NAV_LIBRARY: i32 = 0;
const NAV_LOAD_ORDER: i32 = 1;
const NAV_DOWNLOADS: i32 = 2;
const NAV_TOOLS: i32 = 3;
const NAV_PREFERENCES: i32 = 4;

// ── Main window ────────────────────────────────────────────────────────────

fn build_main_window(
    app: &libadwaita::Application,
    config: Rc<RefCell<AppConfig>>,
) -> (adw::ApplicationWindow, impl Fn() + 'static) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Linkmm")
        .default_width(1100)
        .default_height(720)
        .build();

    // Register app-level About action
    register_about_action(app, &window);
    register_logs_action(app, &window, Rc::clone(&config));

    let split_view = adw::NavigationSplitView::new();
    split_view.set_min_sidebar_width(200.0);
    split_view.set_max_sidebar_width(320.0);
    split_view.set_sidebar_width_fraction(0.24);
    split_view.set_collapsed(false);

    // ── Sidebar ───────────────────────────────────────────────────────────

    let sidebar_toolbar = adw::ToolbarView::new();
    let sidebar_header = adw::HeaderBar::new();
    // Keep window controls only on the content side
    sidebar_header.set_show_end_title_buttons(false);

    let menu_btn = gtk4::MenuButton::new();
    menu_btn.set_icon_name("open-menu-symbolic");
    let menu = gio::Menu::new();
    menu.append(Some("View Logs"), Some("app.logs"));
    menu.append(Some("About Linkmm"), Some("app.about"));
    menu_btn.set_menu_model(Some(&menu));
    sidebar_header.pack_end(&menu_btn);

    sidebar_toolbar.add_top_bar(&sidebar_header);

    // Scrollable sidebar content
    let sidebar_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

    // ── Active Game section ───────────────────────────────────────────────
    sidebar_box.append(&make_section_label("Active Game"));

    let active_game_list = gtk4::ListBox::new();
    active_game_list.add_css_class("boxed-list");
    active_game_list.set_margin_start(12);
    active_game_list.set_margin_end(12);
    active_game_list.set_margin_bottom(12);
    active_game_list.set_selection_mode(gtk4::SelectionMode::None);

    let active_game_row = adw::ActionRow::builder().activatable(true).build();
    let game_icon = gtk4::Image::from_icon_name("applications-games-symbolic");
    active_game_row.add_prefix(&game_icon);

    {
        let cfg = config.borrow();
        update_active_game_row(
            &active_game_row,
            cfg.current_game().map(|g| g.instance_label()),
        );
    }

    active_game_list.append(&active_game_row);
    sidebar_box.append(&active_game_list);

    // ── Play Game button ──────────────────────────────────────────────────

    let play_btn = gtk4::Button::builder().label("Play").hexpand(true).build();
    play_btn.add_css_class("suggested-action");
    play_btn.set_margin_start(12);
    play_btn.set_margin_end(12);
    play_btn.set_margin_bottom(8);

    // Show the button for Steam games (known App ID) and UMU-configured games.
    {
        let cfg = config.borrow();
        play_btn.set_visible(
            cfg.current_game()
                .map(|g| {
                    g.steam_instance_app_id().is_some()
                        || g.launcher_source == GameLauncherSource::NonSteamUmu
                })
                .unwrap_or(false),
        );
    }

    {
        let config_c = Rc::clone(&config);
        play_btn.connect_clicked(move |_| {
            let game = config_c.borrow().current_game().cloned();
            let Some(g) = game else { return };
            if g.launcher_source == GameLauncherSource::Steam {
                if let Err(e) = crate::core::steam::launch_game(&g) {
                    log::warn!("Could not launch {}: {e}", g.name);
                }
                return;
            }
            let manager = global_runtime_manager();
            if let Some(session) = manager.current_game_session(&g.id) {
                if let Err(e) = manager.stop_session(session.id) {
                    log::error!("Failed stopping game session: {e}");
                }
                return;
            }
            let profile_id = config_c
                .borrow()
                .game_settings
                .get(&g.id)
                .map(|gs| gs.active_profile_id.clone());
            if let Err(e) = manager.start_game_session(g.clone(), profile_id) {
                log::warn!("Could not launch {}: {e}", g.name);
            }
        });
    }

    sidebar_box.append(&play_btn);

    // ── Navigation section ────────────────────────────────────────────────
    sidebar_box.append(&make_section_label("Navigation"));

    let nav_list = gtk4::ListBox::new();
    nav_list.add_css_class("boxed-list");
    nav_list.set_selection_mode(gtk4::SelectionMode::Single);
    nav_list.set_margin_start(12);
    nav_list.set_margin_end(12);
    nav_list.set_margin_bottom(12);

    for (name, icon) in &[
        ("Library", "applications-games-symbolic"),
        ("Load Order", "format-justify-left-symbolic"),
        ("Downloads", "folder-download-symbolic"),
        ("Tools", "applications-utilities-symbolic"),
        ("Preferences", "preferences-system-symbolic"),
    ] {
        let row = adw::ActionRow::builder()
            .title(*name)
            .activatable(true)
            .build();
        let img = gtk4::Image::from_icon_name(icon);
        row.add_prefix(&img);
        nav_list.append(&row);
    }

    sidebar_box.append(&nav_list);

    // ── Stats section ─────────────────────────────────────────────────────
    sidebar_box.append(&make_section_label("Stats"));

    let stats_list = gtk4::ListBox::new();
    stats_list.add_css_class("boxed-list");
    stats_list.set_selection_mode(gtk4::SelectionMode::None);
    stats_list.set_margin_start(12);
    stats_list.set_margin_end(12);
    stats_list.set_margin_bottom(12);

    let installed_label = gtk4::Label::new(Some("0"));
    installed_label.add_css_class("dim-label");
    let installed_row = adw::ActionRow::builder().title("Installed").build();
    installed_row.add_suffix(&installed_label);

    let enabled_label = gtk4::Label::new(Some("0"));
    enabled_label.add_css_class("dim-label");
    let enabled_row = adw::ActionRow::builder().title("Enabled").build();
    enabled_row.add_suffix(&enabled_label);

    stats_list.append(&installed_row);
    stats_list.append(&enabled_row);
    sidebar_box.append(&stats_list);

    refresh_stats(&config.borrow(), &installed_label, &enabled_label);

    // Refresh stats every 2 seconds so the counts stay current after mod
    // installs, uninstalls, enables, and disables.
    {
        let installed_t = installed_label.clone();
        let enabled_t = enabled_label.clone();
        let config_t = Rc::clone(&config);
        glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
            refresh_stats(&config_t.borrow(), &installed_t, &enabled_t);
            glib::ControlFlow::Continue
        });
    }

    {
        let config_t = Rc::clone(&config);
        let play_btn_t = play_btn.clone();
        let active_game_row_t = active_game_row.clone();
        let nav_list_t = nav_list.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            let manager = global_runtime_manager();
            let active_any = manager.any_active();
            nav_list_t.set_sensitive(!active_any);
            active_game_row_t.set_sensitive(!active_any);

            let current_game = config_t.borrow().current_game().cloned();
            if let Some(game) = current_game {
                let game_session = manager.current_game_session(&game.id);
                play_btn_t.set_visible(
                    game.steam_instance_app_id().is_some()
                        || game.launcher_source == GameLauncherSource::NonSteamUmu,
                );
                play_btn_t.set_label(if game.launcher_source == GameLauncherSource::Steam {
                    "Launch"
                } else if game_session.is_some() {
                    "Stop"
                } else {
                    "Play"
                });
                play_btn_t.set_sensitive(true);
            }
            glib::ControlFlow::Continue
        });
    }

    let sidebar_scroll = gtk4::ScrolledWindow::new();
    sidebar_scroll.set_vexpand(true);
    sidebar_scroll.set_hscrollbar_policy(gtk4::PolicyType::Never);
    sidebar_scroll.set_child(Some(&sidebar_box));

    sidebar_toolbar.set_content(Some(&sidebar_scroll));

    let sidebar_page = adw::NavigationPage::builder()
        .title("Linkmm")
        .child(&sidebar_toolbar)
        .build();
    split_view.set_sidebar(Some(&sidebar_page));

    // ── Content area ──────────────────────────────────────────────────────
    let install_lock_revealer = gtk4::Revealer::new();
    install_lock_revealer.set_transition_type(gtk4::RevealerTransitionType::SlideDown);
    install_lock_revealer.set_reveal_child(false);

    let install_lock_banner = adw::Banner::new(
        "Installing mod archive… navigation is temporarily locked. You can cancel from Downloads.",
    );
    install_lock_banner.set_revealed(true);
    install_lock_banner.set_button_label(None::<&str>);
    install_lock_revealer.set_child(Some(&install_lock_banner));

    let content_stack = gtk4::Stack::new();
    content_stack.set_transition_type(gtk4::StackTransitionType::None);
    content_stack.set_vexpand(true);
    content_stack.set_hexpand(true);
    let content_layout = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content_layout.append(&install_lock_revealer);
    content_layout.append(&content_stack);

    let content_page = adw::NavigationPage::builder()
        .title("Library")
        .child(&content_layout)
        .build();
    split_view.set_content(Some(&content_page));

    let install_locked = Rc::new(RefCell::new(false));
    // Lock callback used by long-running tasks (downloads/install).
    let nav_list_for_lock = nav_list.clone();
    let active_game_row_for_lock = active_game_row.clone();
    let play_btn_for_lock = play_btn.clone();
    let content_stack_for_lock = content_stack.clone();
    let content_page_for_lock = content_page.clone();
    let install_lock_revealer_for_lock = install_lock_revealer.clone();
    let install_locked_for_cb = Rc::clone(&install_locked);
    let nav_lock: Rc<dyn Fn(bool)> = Rc::new(move |locked: bool| {
        *install_locked_for_cb.borrow_mut() = locked;
        nav_list_for_lock.set_sensitive(!locked);
        active_game_row_for_lock.set_sensitive(!locked);
        play_btn_for_lock.set_sensitive(!locked);
        install_lock_revealer_for_lock.set_reveal_child(locked);
        if locked {
            // Safety: keep Downloads visible while install work is in flight.
            content_page_for_lock.set_title("Downloads");
            content_stack_for_lock.set_visible_child_name("downloads");
            nav_list_for_lock.select_row(nav_list_for_lock.row_at_index(NAV_DOWNLOADS).as_ref());
        }
    });

    let current_game = {
        let cfg = config.borrow();
        cfg.current_game().cloned()
    };

    let content_stack_for_mods_changed = content_stack.clone();
    let config_for_mods_changed = Rc::clone(&config);
    let on_mods_changed: Rc<dyn Fn()> = Rc::new(move || {
        let game_info = {
            let cfg = config_for_mods_changed.borrow();
            cfg.current_game().cloned()
        };
        let visible = content_stack_for_mods_changed
            .visible_child_name()
            .map(|n| n.to_string());

        let new_library: gtk4::Widget = match &game_info {
            Some(g) => library::build_library_page(g, Rc::clone(&config_for_mods_changed)),
            None => build_no_game_page(
                "No Game Selected",
                "Select or add a game to manage its mods.",
            ),
        };
        if let Some(old) = content_stack_for_mods_changed.child_by_name("library") {
            content_stack_for_mods_changed.remove(&old);
        }
        content_stack_for_mods_changed.add_named(&new_library, Some("library"));

        let new_load_order = load_order::build_load_order_page(game_info.as_ref());
        if let Some(old) = content_stack_for_mods_changed.child_by_name("load_order") {
            content_stack_for_mods_changed.remove(&old);
        }
        content_stack_for_mods_changed.add_named(&new_load_order, Some("load_order"));

        if let Some(name) = visible.as_deref()
            && (name == "library" || name == "load_order")
        {
            content_stack_for_mods_changed.set_visible_child_name(name);
        }
    });

    // Library
    let library_widget: gtk4::Widget = match &current_game {
        Some(g) => library::build_library_page(g, Rc::clone(&config)),
        None => build_no_game_page(
            "No Game Selected",
            "Select or add a game to manage its mods.",
        ),
    };
    content_stack.add_named(&library_widget, Some("library"));

    // Load Order
    let load_order_widget = load_order::build_load_order_page(current_game.as_ref());
    content_stack.add_named(&load_order_widget, Some("load_order"));

    // Downloads
    let downloads_widget = downloads::build_downloads_page(
        current_game.as_ref(),
        Rc::clone(&config),
        Rc::clone(&nav_lock),
        Rc::clone(&on_mods_changed),
    );
    content_stack.add_named(&downloads_widget, Some("downloads"));

    // Tools
    let tools_widget = tools::build_tools_page(current_game.as_ref(), Rc::clone(&config));
    content_stack.add_named(&tools_widget, Some("tools"));

    // Preferences
    let preferences_widget =
        settings::build_settings_page(Rc::clone(&config), window.upcast_ref::<gtk4::Window>());
    content_stack.add_named(&preferences_widget, Some("preferences"));

    // Allow mobile-like narrow layouts: collapse the sidebar when the window is small.
    split_view.add_tick_callback(|sv, _| {
        let should_collapse = sv.width() < 860;
        if sv.is_collapsed() != should_collapse {
            sv.set_collapsed(should_collapse);
            if should_collapse {
                sv.set_show_content(true);
            }
        }
        glib::ControlFlow::Continue
    });

    window.set_content(Some(&split_view));

    // Select Library by default
    nav_list.select_row(nav_list.row_at_index(NAV_LIBRARY).as_ref());

    // ── Navigation signal ─────────────────────────────────────────────────
    {
        let content_stack_c = content_stack.clone();
        let content_page_c = content_page.clone();
        let install_locked_c = Rc::clone(&install_locked);

        nav_list.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            if *install_locked_c.borrow() {
                return;
            }
            match row.index() {
                NAV_LIBRARY => {
                    content_page_c.set_title("Library");
                    content_stack_c.set_visible_child_name("library");
                }
                NAV_LOAD_ORDER => {
                    content_page_c.set_title("Load Order");
                    content_stack_c.set_visible_child_name("load_order");
                }
                NAV_DOWNLOADS => {
                    content_page_c.set_title("Downloads");
                    content_stack_c.set_visible_child_name("downloads");
                }
                NAV_TOOLS => {
                    content_page_c.set_title("Tools");
                    content_stack_c.set_visible_child_name("tools");
                }
                NAV_PREFERENCES => {
                    content_page_c.set_title("Preferences");
                    content_stack_c.set_visible_child_name("preferences");
                }
                _ => {}
            }
        });
    }

    // ── on_setup_done callback ──────────────────────────────────────────────
    let active_game_row_r = active_game_row.clone();
    let installed_r = installed_label.clone();
    let enabled_r = enabled_label.clone();
    let content_stack_r = content_stack.clone();
    let content_page_r = content_page.clone();
    let config_r = Rc::clone(&config);
    let nav_list_r = nav_list.clone();
    let play_btn_r = play_btn.clone();
    let nav_lock_r = Rc::clone(&nav_lock);
    let on_mods_changed_r = Rc::clone(&on_mods_changed);
    let window_r = window.clone();

    let on_setup_done_rc: Rc<dyn Fn()> = Rc::new(move || {
        let game_info = {
            let cfg = config_r.borrow();
            cfg.current_game().cloned()
        };

        if let Some(game) = &game_info {
            update_active_game_row(&active_game_row_r, Some(game.instance_label()));
        } else {
            update_active_game_row(&active_game_row_r, None);
        }

        // Show Play button for Steam games (has App ID) and UMU-configured games.
        play_btn_r.set_visible(
            game_info
                .as_ref()
                .map(|g| {
                    g.steam_instance_app_id().is_some()
                        || g.launcher_source == GameLauncherSource::NonSteamUmu
                })
                .unwrap_or(false),
        );

        // Rebuild Library page for the new game
        let new_library: gtk4::Widget = match &game_info {
            Some(g) => library::build_library_page(g, Rc::clone(&config_r)),
            None => build_no_game_page(
                "No Game Selected",
                "Select or add a game to manage its mods.",
            ),
        };
        if let Some(old) = content_stack_r.child_by_name("library") {
            content_stack_r.remove(&old);
        }
        content_stack_r.add_named(&new_library, Some("library"));

        // Rebuild Load Order page
        let new_load_order = load_order::build_load_order_page(game_info.as_ref());
        if let Some(old) = content_stack_r.child_by_name("load_order") {
            content_stack_r.remove(&old);
        }
        content_stack_r.add_named(&new_load_order, Some("load_order"));

        // Rebuild Downloads page
        let new_downloads = downloads::build_downloads_page(
            game_info.as_ref(),
            Rc::clone(&config_r),
            Rc::clone(&nav_lock_r),
            Rc::clone(&on_mods_changed_r),
        );
        if let Some(old) = content_stack_r.child_by_name("downloads") {
            content_stack_r.remove(&old);
        }
        content_stack_r.add_named(&new_downloads, Some("downloads"));

        // Rebuild Tools page
        let new_tools = tools::build_tools_page(game_info.as_ref(), Rc::clone(&config_r));
        if let Some(old) = content_stack_r.child_by_name("tools") {
            content_stack_r.remove(&old);
        }
        content_stack_r.add_named(&new_tools, Some("tools"));

        // Rebuild Preferences page so the UMU section reflects the new game.
        let new_prefs = settings::build_settings_page(
            Rc::clone(&config_r),
            &window_r.clone().upcast::<gtk4::Window>(),
        );
        if let Some(old) = content_stack_r.child_by_name("preferences") {
            content_stack_r.remove(&old);
        }
        content_stack_r.add_named(&new_prefs, Some("preferences"));

        // Switch to Library
        content_page_r.set_title("Library");
        content_stack_r.set_visible_child_name("library");
        nav_list_r.select_row(nav_list_r.row_at_index(NAV_LIBRARY).as_ref());

        // Update stats
        refresh_stats(&config_r.borrow(), &installed_r, &enabled_r);
    });

    // ── Active-game row click → open wizard / game picker ─────────────────
    {
        let window_c = window.clone();
        let config_c = Rc::clone(&config);
        let on_finish_c = Rc::clone(&on_setup_done_rc);

        active_game_row.connect_activated(move |_| {
            let has_games = !config_c.borrow().games.is_empty();
            if !has_games {
                let f = Rc::clone(&on_finish_c);
                setup_wizard::show_setup_wizard(&window_c, Rc::clone(&config_c), move || f());
            } else {
                show_game_picker(&window_c, Rc::clone(&config_c), Rc::clone(&on_finish_c));
            }
        });
    }

    let on_setup_done_final = Rc::clone(&on_setup_done_rc);
    (window, move || on_setup_done_final())
}

#[cfg(test)]
mod tests {
    use super::{NAV_DOWNLOADS, NAV_LIBRARY, NAV_LOAD_ORDER, NAV_PREFERENCES, NAV_TOOLS};

    fn page_for_nav(index: i32) -> Option<(&'static str, &'static str)> {
        match index {
            NAV_LIBRARY => Some(("Library", "library")),
            NAV_LOAD_ORDER => Some(("Load Order", "load_order")),
            NAV_DOWNLOADS => Some(("Downloads", "downloads")),
            NAV_TOOLS => Some(("Tools", "tools")),
            NAV_PREFERENCES => Some(("Preferences", "preferences")),
            _ => None,
        }
    }

    #[test]
    fn nav_index_maps_to_stable_stack_pages() {
        assert_eq!(page_for_nav(NAV_LIBRARY), Some(("Library", "library")));
        assert_eq!(
            page_for_nav(NAV_LOAD_ORDER),
            Some(("Load Order", "load_order"))
        );
        assert_eq!(
            page_for_nav(NAV_DOWNLOADS),
            Some(("Downloads", "downloads"))
        );
        assert_eq!(page_for_nav(NAV_TOOLS), Some(("Tools", "tools")));
        assert_eq!(
            page_for_nav(NAV_PREFERENCES),
            Some(("Preferences", "preferences"))
        );
        assert_eq!(page_for_nav(999), None);
    }
}

// ── Helper widgets ─────────────────────────────────────────────────────────

fn make_section_label(text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("heading");
    label.set_halign(gtk4::Align::Start);
    label.set_margin_start(18);
    label.set_margin_top(16);
    label.set_margin_bottom(4);
    label
}

fn update_active_game_row(row: &adw::ActionRow, name: Option<String>) {
    match name {
        Some(n) => {
            row.set_title(&n);
            row.set_subtitle("Click to switch game");
        }
        None => {
            row.set_title("No Game Selected");
            row.set_subtitle("Click to add a game");
        }
    }
}

fn build_no_game_page(title: &str, description: &str) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let status = adw::StatusPage::builder()
        .title(title)
        .description(description)
        .icon_name("applications-games-symbolic")
        .build();
    status.set_vexpand(true);
    toolbar_view.set_content(Some(&status));
    toolbar_view.upcast()
}

fn refresh_stats(cfg: &AppConfig, installed: &gtk4::Label, enabled: &gtk4::Label) {
    if let Some(game) = cfg.current_game() {
        let db = ModDatabase::load(game);
        installed.set_text(&db.mods.len().to_string());
        enabled.set_text(&db.mods.iter().filter(|m| m.enabled).count().to_string());
    } else {
        installed.set_text("0");
        enabled.set_text("0");
    }
}

// ── Game picker dialog ─────────────────────────────────────────────────────

fn show_game_picker(
    parent: &adw::ApplicationWindow,
    config: Rc<RefCell<AppConfig>>,
    on_game_changed: Rc<dyn Fn()>,
) {
    let dialog = adw::Window::builder()
        .title("Switch Game")
        .modal(true)
        .transient_for(parent)
        .default_width(480)
        .default_height(400)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content_box.set_margin_start(24);
    content_box.set_margin_end(24);
    content_box.set_margin_top(12);
    content_box.set_margin_bottom(12);

    let game_list = gtk4::ListBox::new();
    game_list.add_css_class("boxed-list");
    game_list.set_selection_mode(gtk4::SelectionMode::None);

    let current_id = config.borrow().current_game_id.clone();

    {
        let games = config.borrow().games.clone();
        for game in &games {
            let subtitle = format!(
                "{} · {}",
                game.kind.display_name(),
                game.root_path.display()
            );
            let row = adw::ActionRow::builder()
                .title(game.instance_label())
                .subtitle(&subtitle)
                .activatable(true)
                .build();

            if current_id.as_deref() == Some(&game.id) {
                let check = gtk4::Image::from_icon_name("object-select-symbolic");
                row.add_suffix(&check);
            }

            let game_id = game.id.clone();
            let config_c = Rc::clone(&config);
            let dialog_c = dialog.clone();
            let on_changed_c = Rc::clone(&on_game_changed);
            row.connect_activated(move |_| {
                config_c.borrow_mut().current_game_id = Some(game_id.clone());
                config_c.borrow().save();
                dialog_c.destroy();
                on_changed_c();
            });

            game_list.append(&row);
        }
    }

    content_box.append(&game_list);

    let add_btn = gtk4::Button::with_label("Add New Game\u{2026}");
    add_btn.add_css_class("suggested-action");
    add_btn.set_halign(gtk4::Align::Center);
    add_btn.set_margin_top(8);

    let parent_c = parent.clone();
    let config_c = Rc::clone(&config);
    let dialog_c = dialog.clone();
    let on_changed_c = Rc::clone(&on_game_changed);
    add_btn.connect_clicked(move |_| {
        dialog_c.destroy();
        let f = Rc::clone(&on_changed_c);
        setup_wizard::show_setup_wizard(&parent_c, Rc::clone(&config_c), move || f());
    });
    content_box.append(&add_btn);

    toolbar_view.set_content(Some(&content_box));
    dialog.set_content(Some(&toolbar_view));
    dialog.present();
}

// ── About action ───────────────────────────────────────────────────────────

fn register_about_action(app: &libadwaita::Application, window: &adw::ApplicationWindow) {
    let action = gio::SimpleAction::new("about", None);
    let window_c = window.clone();
    action.connect_activate(move |_, _| {
        show_about_dialog(window_c.upcast_ref::<gtk4::Window>());
    });
    app.add_action(&action);
}

// ── Logs action ────────────────────────────────────────────────────────────

fn register_logs_action(
    app: &libadwaita::Application,
    window: &adw::ApplicationWindow,
    config: Rc<RefCell<AppConfig>>,
) {
    let action = gio::SimpleAction::new("logs", None);
    let window_c = window.clone();
    action.connect_activate(move |_, _| {
        logs::show_log_window(window_c.upcast_ref::<gtk4::Window>(), Rc::clone(&config));
    });
    app.add_action(&action);
}

fn show_about_dialog(parent: &gtk4::Window) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Linkmm")
        .application_icon("applications-games-symbolic")
        .developer_name("Linkmm Contributors")
        .version(env!("CARGO_PKG_VERSION"))
        .website("https://github.com/sachesi/linkmm")
        .issue_url("https://github.com/sachesi/linkmm/issues")
        .license_type(gtk4::License::Gpl30)
        .build();
    dialog.present(Some(parent));
}

// ── NXM protocol handler ──────────────────────────────────────────────────

const NXM_DESKTOP_ID: &str = "io.github.sachesi.linkmm.desktop";
const NXM_SCHEME: &str = "x-scheme-handler/nxm";

/// Ensure this application is registered as the `nxm://` protocol handler.
///
/// Checks the current handler via GIO; if it is not already set to this app,
/// writes the `.desktop` file to `~/.local/share/applications/` (if absent)
/// and calls `xdg-mime default` to register it — the same operation a user
/// would run manually.  Both steps are no-ops when already set, so calling
/// this on every startup is safe.
fn register_nxm_handler() {
    // Check whether we are already the registered handler.
    let already_set = gio::AppInfo::default_for_type(NXM_SCHEME, false)
        .and_then(|a| a.id())
        .map(|id| id.as_str() == NXM_DESKTOP_ID)
        .unwrap_or(false);
    if already_set {
        return;
    }

    // Ensure the .desktop file exists somewhere GIO can find it.  If it was
    // installed system-wide (e.g. via a package) this directory write is
    // skipped; we only write it when it is genuinely absent.
    let apps_dir = match dirs::data_local_dir() {
        Some(d) => d.join("applications"),
        None => {
            log::warn!("register_nxm_handler: could not determine local applications directory");
            return;
        }
    };

    let desktop_path = apps_dir.join(NXM_DESKTOP_ID);
    if !desktop_path.exists() {
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                log::warn!("register_nxm_handler: could not determine executable path: {e}");
                return;
            }
        };

        let desktop_content = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Linkmm\n\
             Comment=Link-based mod manager for Bethesda games\n\
             Exec={} %u\n\
             Icon=applications-games-symbolic\n\
             Terminal=false\n\
             Categories=Game;Utility;\n\
             MimeType=x-scheme-handler/nxm;\n\
             StartupNotify=true\n",
            exe.display()
        );

        if let Err(e) = std::fs::create_dir_all(&apps_dir) {
            log::warn!("register_nxm_handler: could not create applications directory: {e}");
            return;
        }
        if let Err(e) = std::fs::write(&desktop_path, &desktop_content) {
            log::warn!("register_nxm_handler: could not write desktop file: {e}");
            return;
        }

        // Refresh the desktop database so xdg-mime and GIO find the new file.
        let _ = std::process::Command::new("update-desktop-database")
            .arg(&apps_dir)
            .status();
    }

    // Register as the default handler — equivalent to running:
    //   xdg-mime default io.github.sachesi.linkmm.desktop x-scheme-handler/nxm
    match std::process::Command::new("xdg-mime")
        .args(["default", NXM_DESKTOP_ID, NXM_SCHEME])
        .status()
    {
        Ok(s) if s.success() => log::info!("Registered as nxm:// handler via xdg-mime"),
        Ok(s) => log::warn!("xdg-mime default exited with status {s}"),
        Err(e) => log::warn!("register_nxm_handler: xdg-mime not available: {e}"),
    }
}

/// Handle an `nxm://` URL received from the browser.
///
/// Parses the URL, fetches the download link from the Nexus API, downloads the
/// file to the configured downloads directory, and shows a toast on the active
/// window.  When the Nexus domain does not match the currently active game, a
/// warning dialog is shown before the download begins.
pub fn handle_nxm_url(app: &libadwaita::Application, url: &str) {
    use crate::core::nxm::NxmUrl;

    let config = AppConfig::load_or_default();
    let Some(api_key) = config.nexus_api_key.clone() else {
        log::error!("Cannot handle NXM URL: no API key configured");
        if let Some(window) = app.active_window() {
            show_nxm_toast(&window, "Set your NexusMods API key in Preferences first.");
        }
        return;
    };

    let nxm = match NxmUrl::parse(url) {
        Ok(n) => n,
        Err(e) => {
            log::error!("Failed to parse NXM URL: {e}");
            if let Some(window) = app.active_window() {
                show_nxm_toast(&window, &format!("Invalid NXM link: {e}"));
            }
            return;
        }
    };

    log::info!(
        "Handling NXM URL: game={}, mod={}, file={}",
        nxm.game_domain,
        nxm.mod_id,
        nxm.file_id
    );

    // Find the game whose Nexus domain matches the NXM URL
    let matching_game = config
        .games
        .iter()
        .find(|g| g.kind.nexus_game_id() == nxm.game_domain)
        .cloned();
    let current_game = config.current_game().cloned();

    // Determine which game folder receives the download
    let download_target = matching_game.clone().or_else(|| current_game.clone());
    let downloads_dir = config.downloads_dir(download_target.as_ref().map(|g| g.id.as_str()));
    if let Err(e) = std::fs::create_dir_all(&downloads_dir) {
        log::error!("Failed to create downloads directory: {e}");
        return;
    }

    // Check whether the NXM domain mismatches the currently active game
    let mismatch_msg: Option<String> = match (&matching_game, &current_game) {
        // A different configured game matches the domain
        (Some(matched), Some(current)) if matched.id != current.id => Some(format!(
            "This mod is for \"{}\", but your currently active game is \"{}\".\n\nThe archive will be saved to {}'s download folder.",
            matched.name, current.name, matched.name
        )),
        // No configured game matches the domain at all
        (None, _) => Some(format!(
            "No game configured for Nexus domain \"{}\".\n\nThe archive will be saved to the default download folder.",
            nxm.game_domain
        )),
        _ => None,
    };

    let app_c = app.clone();
    let nxm_c = nxm.clone();
    let api_key_c = api_key.clone();
    let downloads_dir_c = downloads_dir.clone();
    let download_target_c = download_target.clone();

    if let Some(msg) = mismatch_msg {
        let window = app.active_window();
        show_nxm_game_mismatch_dialog(
            window.as_ref(),
            &msg,
            app,
            nxm,
            api_key,
            downloads_dir,
            download_target,
        );
    } else {
        start_nxm_download(&app_c, nxm_c, api_key_c, downloads_dir_c, download_target_c);
    }
}

/// Show a warning dialog when the NXM download's game domain does not match
/// the currently active game.  Starts the download only if the user chooses
/// "Download Anyway".
fn show_nxm_game_mismatch_dialog(
    parent: Option<&gtk4::Window>,
    message: &str,
    app: &libadwaita::Application,
    nxm: crate::core::nxm::NxmUrl,
    api_key: String,
    downloads_dir: std::path::PathBuf,
    download_target: Option<crate::core::games::Game>,
) {
    let dialog = adw::AlertDialog::builder()
        .heading("Game Mismatch")
        .body(message)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("download", "Download Anyway");
    dialog.set_response_appearance("download", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let app_c = app.clone();
    let nxm_c = nxm.clone();
    let api_key_c = api_key.clone();
    let downloads_dir_c = downloads_dir.clone();
    let download_target_c = download_target.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "download" {
            start_nxm_download(
                &app_c,
                nxm_c.clone(),
                api_key_c.clone(),
                downloads_dir_c.clone(),
                download_target_c.clone(),
            );
        }
    });
    dialog.present(parent);
}

/// Spawn the background download thread for an NXM link and poll for
/// completion on the GTK main thread.
fn start_nxm_download(
    app: &libadwaita::Application,
    nxm: crate::core::nxm::NxmUrl,
    api_key: String,
    downloads_dir: std::path::PathBuf,
    download_target: Option<crate::core::games::Game>,
) {
    use crate::core::download;
    use crate::core::download_state;
    use crate::core::nexus::NexusClient;

    if let Some(window) = app.active_window() {
        show_nxm_toast(
            &window,
            &format!("Starting download for mod {}…", nxm.mod_id),
        );
    }

    let (tx, rx) = mpsc::channel::<Result<String, String>>();

    std::thread::spawn(move || {
        let result = (|| -> Result<String, String> {
            let client = NexusClient::new(&api_key);

            // Get file info to determine the file name
            let files = client.get_mod_files(&nxm.game_domain, nxm.mod_id as u32)?;
            let file_info = files.iter().find(|f| f.file_id == nxm.file_id);
            let raw_name = match file_info {
                Some(f) => f.file_name.clone(),
                None => format!("mod_{}_{}.zip", nxm.mod_id, nxm.file_id),
            };

            // Sanitize filename: strip path components, reject traversal
            let file_name = std::path::Path::new(&raw_name)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("mod_{}_{}.zip", nxm.mod_id, nxm.file_id));
            if file_name.contains("..") || file_name.starts_with('.') {
                return Err("Invalid filename from server".to_string());
            }

            let dest_path = downloads_dir.join(&file_name);
            if dest_path.exists() {
                if let Some(ref game) = download_target
                    && let Err(e) =
                        game.write_nxm_mod_id(&file_name, &nxm.game_domain, nxm.mod_id as u32)
                {
                    log::warn!("Failed to update NXM metadata for {}: {e}", file_name);
                }
                return Ok(format!("{file_name} (already downloaded)"));
            }

            // Get download link using NXM key/expires if available, otherwise
            // fall back to the premium-only direct API
            let links = match (&nxm.key, &nxm.expires) {
                (Some(key), Some(expires)) => client.get_download_links_nxm(
                    &nxm.game_domain,
                    nxm.mod_id as u32,
                    nxm.file_id,
                    key,
                    expires,
                )?,
                _ => client.get_download_links(&nxm.game_domain, nxm.mod_id as u32, nxm.file_id)?,
            };

            let (_cdn, url) = links
                .first()
                .ok_or_else(|| "No download links available".to_string())?;

            let download_id = download_state::set_active(file_name.clone());
            let download_result = download::download_file(url, &dest_path, |downloaded, total| {
                download_state::update_progress(download_id, downloaded, total);
                !download_state::is_cancel_requested(download_id)
            });
            download_state::clear_active(download_id);
            download_result?;

            if let Some(ref game) = download_target
                && let Err(e) =
                    game.write_nxm_mod_id(&file_name, &nxm.game_domain, nxm.mod_id as u32)
            {
                log::warn!("Failed to write NXM metadata for {}: {e}", file_name);
            }

            Ok(file_name)
        })();
        let _ = tx.send(result);
    });

    // Poll for completion and show toast
    let app_c = app.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        match rx.try_recv() {
            Ok(Ok(file_name)) => {
                log::info!("NXM download complete: {file_name}");
                if let Some(window) = app_c.active_window() {
                    show_nxm_toast(&window, &format!("Downloaded: {file_name}"));
                }
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                if let Some(window) = app_c.active_window() {
                    if e == download::DOWNLOAD_CANCELLED_ERROR {
                        log::info!("NXM download cancelled");
                        show_nxm_toast(&window, "Download cancelled");
                    } else {
                        log::error!("NXM download failed: {e}");
                        show_nxm_toast(&window, &format!("Download failed: {e}"));
                    }
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn show_nxm_toast(window: &gtk4::Window, message: &str) {
    // Try to find a ToastOverlay in the window hierarchy
    if let Some(child) = window.child() {
        walk_for_toast_overlay(&child, message);
    }
    log::info!("NXM: {message}");
}

fn walk_for_toast_overlay(widget: &gtk4::Widget, message: &str) {
    if let Ok(overlay) = widget.clone().downcast::<adw::ToastOverlay>() {
        let toast = adw::Toast::new(message);
        toast.set_timeout(4);
        overlay.add_toast(toast);
        return;
    }
    // Try first child recursively
    if let Some(child) = widget.first_child() {
        walk_for_toast_overlay(&child, message);
    }
}
