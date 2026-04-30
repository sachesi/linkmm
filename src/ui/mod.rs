use std::cell::RefCell;
use std::rc::Rc;

use gio;
use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::mods::ModDatabase;
use crate::core::runtime::global_runtime_manager;
use crate::core::umu;

pub mod downloads;
pub mod library;
pub mod load_order;
pub mod logs;
pub mod mod_list;
pub mod ordering;
pub mod settings;
pub mod setup_wizard;
pub mod tools;

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

    let sidebar_toolbar = adw::ToolbarView::new();
    let sidebar_header = adw::HeaderBar::new();
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

    // Show the button if a game is selected.
    {
        let cfg = config.borrow();
        play_btn.set_visible(cfg.current_game_id.is_some());
    }

    {
        let config_c = Rc::clone(&config);
        let play_btn_c = play_btn.clone();
        play_btn.connect_clicked(move |_| {
            let manager = global_runtime_manager();

            // 1. Check if a session is already running
            let current_game = config_c.borrow().current_game().cloned();
            let Some(g) = current_game else { return };

            if let Some(session) = manager.current_game_session(&g.id) {
                if let Err(e) = manager.stop_session(session.id) {
                    log::error!("Failed stopping game session: {e}");
                }
                return;
            }

            play_btn_c.set_sensitive(false);
            play_btn_c.set_label("Launching…");

            let manager = global_runtime_manager();
            let g_c = g.clone();

            std::thread::spawn(move || {
                if let Err(e) = manager.start_game_session(g_c) {
                    log::error!("Launch failed: {e}");
                }
            });
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
    sidebar_box.append(&make_section_label("Library Stats"));

    let stats_list = gtk4::ListBox::new();
    stats_list.add_css_class("boxed-list");
    stats_list.set_margin_start(12);
    stats_list.set_margin_end(12);
    stats_list.set_margin_bottom(12);
    stats_list.set_selection_mode(gtk4::SelectionMode::None);

    let installed_label = gtk4::Label::new(Some("0"));
    let installed_row = adw::ActionRow::builder()
        .title("Total Mods")
        .subtitle("Number of extracted mods in library")
        .build();
    installed_row.add_suffix(&installed_label);

    let enabled_label = gtk4::Label::new(Some("0"));
    let enabled_row = adw::ActionRow::builder()
        .title("Enabled")
        .subtitle("Mods currently active in game")
        .build();
    enabled_row.add_suffix(&enabled_label);

    stats_list.append(&installed_row);
    stats_list.append(&enabled_row);
    sidebar_box.append(&stats_list);

    {
        let cfg = config.borrow();
        refresh_stats(&cfg, &installed_label, &enabled_label);
    }

    // Refresh stats and Play button state periodically
    {
        let config_t = Rc::clone(&config);
        let installed_t = installed_label.clone();
        let enabled_t = enabled_label.clone();
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
                play_btn_t.set_visible(true);
                play_btn_t.set_label(if game_session.is_some() {
                    "Stop"
                } else {
                    "Play"
                });
                play_btn_t.set_sensitive(true);

                refresh_stats(&config_t.borrow(), &installed_t, &enabled_t);
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

        // Show Play button if a game is selected.
        play_btn_r.set_visible(game_info.is_some());

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

        // Refresh stats
        refresh_stats(&config_r.borrow(), &installed_r, &enabled_r);

        // Reset sidebar selection to Library
        nav_list_r.select_row(nav_list_r.row_at_index(NAV_LIBRARY).as_ref());
        content_page_r.set_title("Library");
        content_stack_r.set_visible_child_name("library");
    });

    // ── Active game row click handler ─────────────────────────────────────────
    {
        let config_a = Rc::clone(&config);
        let on_setup_done_a = Rc::clone(&on_setup_done_rc);
        let window_a = window.clone();
        active_game_row.connect_activated(move |_| {
            show_game_picker(&window_a, Rc::clone(&config_a), Rc::clone(&on_setup_done_a));
        });
    }

    (window, move || on_setup_done_rc())
}

fn make_section_label(text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("caption-heading");
    label.set_halign(gtk4::Align::Start);
    label.set_margin_start(18);
    label.set_margin_top(12);
    label.set_margin_bottom(6);
    label
}

fn update_active_game_row(row: &adw::ActionRow, label: Option<String>) {
    match label {
        Some(l) => {
            row.set_title(&l);
            row.set_subtitle("Click to switch game");
        }
        None => {
            row.set_title("No Game Selected");
            row.set_subtitle("Click to configure");
        }
    }
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
    content_box.set_margin_start(12);
    content_box.set_margin_end(12);
    content_box.set_margin_top(12);
    content_box.set_margin_bottom(12);

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::None);

    let games = {
        let cfg = config.borrow();
        cfg.games.clone()
    };

    for game in games {
        let row = adw::ActionRow::builder()
            .title(&game.instance_label())
            .activatable(true)
            .build();

        let config_c = Rc::clone(&config);
        let on_game_changed_c = Rc::clone(&on_game_changed);
        let dialog_c = dialog.clone();
        let game_id = game.id.clone();

        row.connect_activated(move |_| {
            config_c.borrow_mut().current_game_id = Some(game_id.clone());
            config_c.borrow().save();
            on_game_changed_c();
            dialog_c.destroy();
        });

        list_box.append(&row);
    }

    let add_game_row = adw::ActionRow::builder()
        .title("Add New Game…")
        .activatable(true)
        .build();
    let add_icon = gtk4::Image::from_icon_name("list-add-symbolic");
    add_game_row.add_prefix(&add_icon);

    let config_a = Rc::clone(&config);
    let on_game_changed_a = Rc::clone(&on_game_changed);
    let dialog_a = dialog.clone();
    let parent_a = parent.clone();
    add_game_row.connect_activated(move |_| {
        dialog_a.destroy();
        setup_wizard::show_setup_wizard(&parent_a, Rc::clone(&config_a), {
            let cb = on_game_changed_a.clone();
            move || cb()
        });
    });

    list_box.append(&add_game_row);

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_child(Some(&list_box));
    content_box.append(&scrolled);

    toolbar_view.set_content(Some(&content_box));
    dialog.set_content(Some(&toolbar_view));
    dialog.present();
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

// ── About action ───────────────────────────────────────────────────────────

fn register_about_action(app: &libadwaita::Application, window: &adw::ApplicationWindow) {
    let action = gio::SimpleAction::new("about", None);
    let window_c = window.clone();
    action.connect_activate(move |_, _| {
        show_about_dialog(window_c.upcast_ref::<gtk4::Window>());
    });
    app.add_action(&action);
}

fn show_about_dialog(parent: &gtk4::Window) {
    adw::AboutWindow::builder()
        .application_name("Linkmm")
        .version("0.1.0")
        .developer_name("sachesi")
        .license_type(gtk4::License::Gpl30Only)
        .website("https://github.com/sachesi/linkmm")
        .issue_url("https://github.com/sachesi/linkmm/issues")
        .transient_for(parent)
        .modal(true)
        .build()
        .present();
}

pub fn build_ui(app: &libadwaita::Application) {
    let config = Rc::new(RefCell::new(AppConfig::load_or_default()));

    // Background update check for umu-launcher
    {
        let cfg = config.borrow();
        let installed = cfg.umu_installed_version.clone();
        let config_c = Rc::clone(&config);
        umu::check_and_update_in_background(installed, move |new_tag| {
            let mut cfg = config_c.borrow_mut();
            cfg.umu_installed_version = Some(new_tag);
            cfg.save();
        });
    }

    let (window, on_setup_done) = build_main_window(app, Rc::clone(&config));

    if config.borrow().first_run {
        setup_wizard::show_setup_wizard(&window, Rc::clone(&config), move || on_setup_done());
    }

    window.present();
}

fn build_no_game_page(title: &str, description: &str) -> gtk4::Widget {
    let status = adw::StatusPage::builder()
        .title(title)
        .description(description)
        .icon_name("applications-games-symbolic")
        .build();
    status.set_vexpand(true);
    status.upcast()
}

pub fn handle_nxm_url(app: &libadwaita::Application, url: &str) {
    let window = app
        .active_window()
        .and_then(|w| w.downcast::<adw::Window>().ok());

    // Pass to the downloads module or appropriate controller
    log::info!("Handling NXM URL: {}", url);
    if let Some(_window) = window {
        // Handle in window
    }
}
