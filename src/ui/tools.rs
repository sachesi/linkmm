use std::cell::RefCell;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;

use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::{AppConfig, ToolConfig};
use crate::core::games::Game;

/// Build the Tools page for managing external Windows tools (e.g., BodySlide, xEdit).
pub fn build_tools_page(
    game: Option<&Game>,
    config: Rc<RefCell<AppConfig>>,
) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let title_widget = adw::WindowTitle::new("Tools", "");
    header.set_title_widget(Some(&title_widget));
    toolbar_view.add_top_bar(&header);

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

    if let Some(game) = game {
        // Tools group
        let tools_group = adw::PreferencesGroup::builder()
            .title("External Tools")
            .description(format!("Configure Windows-native utilities for {}", game.name))
            .build();

        let add_tool_btn = gtk4::Button::new();
        add_tool_btn.set_icon_name("list-add-symbolic");
        add_tool_btn.add_css_class("flat");
        add_tool_btn.set_tooltip_text(Some("Add Tool"));
        tools_group.set_header_suffix(Some(&add_tool_btn));

        let tools_list = gtk4::ListBox::new();
        tools_list.add_css_class("boxed-list");
        tools_list.set_selection_mode(gtk4::SelectionMode::None);
        tools_group.add(&tools_list);

        // Rebuild function to refresh the tool list
        let rebuild: Rc<RefCell<Box<dyn Fn()>>> = Rc::new(RefCell::new(Box::new(|| {})));
        let rebuild_weak = Rc::downgrade(&rebuild);

        {
            let tools_list_c = tools_list.clone();
            let config_c = Rc::clone(&config);
            let game_id = game.id.clone();
            let toast_overlay_c = toast_overlay.clone();

            *rebuild.borrow_mut() = Box::new(move || {
                // Clear existing rows
                while let Some(child) = tools_list_c.first_child() {
                    tools_list_c.remove(&child);
                }

                let cfg = config_c.borrow();
                let game_settings = cfg.game_settings.get(&game_id);
                let empty_tools = Vec::new();
                let tools = game_settings.map(|gs| &gs.tools).unwrap_or(&empty_tools);

                if tools.is_empty() {
                    let empty_row = adw::ActionRow::builder()
                        .title("No tools configured")
                        .build();
                    empty_row.set_sensitive(false);
                    tools_list_c.append(&empty_row);
                    return;
                }

                for tool in tools {
                    let row = adw::ActionRow::builder()
                        .title(&tool.name)
                        .subtitle(tool.exe_path.to_string_lossy().as_ref())
                        .build();

                    // Launch button
                    let launch_btn = gtk4::Button::new();
                    launch_btn.set_icon_name("media-playback-start-symbolic");
                    launch_btn.add_css_class("flat");
                    launch_btn.set_valign(gtk4::Align::Center);
                    launch_btn.set_tooltip_text(Some("Launch Tool"));

                    {
                        let tool_clone = tool.clone();
                        let toast_overlay_c2 = toast_overlay_c.clone();
                        launch_btn.connect_clicked(move |btn| {
                            launch_tool(&tool_clone, btn, &toast_overlay_c2);
                        });
                    }

                    row.add_suffix(&launch_btn);

                    // Edit button
                    let edit_btn = gtk4::Button::new();
                    edit_btn.set_icon_name("document-edit-symbolic");
                    edit_btn.add_css_class("flat");
                    edit_btn.set_valign(gtk4::Align::Center);
                    edit_btn.set_tooltip_text(Some("Edit Tool"));

                    {
                        let tool_id = tool.id.clone();
                        let config_c2 = Rc::clone(&config_c);
                        let game_id_c = game_id.clone();
                        let rebuild_w = rebuild_weak.clone();
                        let toast_overlay_c3 = toast_overlay_c.clone();

                        edit_btn.connect_clicked(move |_| {
                            show_tool_dialog(
                                &config_c2,
                                &game_id_c,
                                Some(&tool_id),
                                &rebuild_w,
                                &toast_overlay_c3,
                            );
                        });
                    }

                    row.add_suffix(&edit_btn);

                    // Delete button
                    let delete_btn = gtk4::Button::new();
                    delete_btn.set_icon_name("user-trash-symbolic");
                    delete_btn.add_css_class("flat");
                    delete_btn.add_css_class("destructive-action");
                    delete_btn.set_valign(gtk4::Align::Center);
                    delete_btn.set_tooltip_text(Some("Delete Tool"));

                    {
                        let tool_id = tool.id.clone();
                        let config_c3 = Rc::clone(&config_c);
                        let game_id_c2 = game_id.clone();
                        let rebuild_w2 = rebuild_weak.clone();

                        delete_btn.connect_clicked(move |_| {
                            let mut cfg = config_c3.borrow_mut();
                            let game_settings = cfg.game_settings_mut(&game_id_c2);
                            game_settings.tools.retain(|t| t.id != tool_id);
                            cfg.save();
                            drop(cfg);

                            if let Some(rb) = rebuild_w2.upgrade() {
                                (rb.borrow())();
                            }
                        });
                    }

                    row.add_suffix(&delete_btn);

                    tools_list_c.append(&row);
                }
            });
        }

        // Add Tool button handler
        {
            let config_c = Rc::clone(&config);
            let game_id = game.id.clone();
            let rebuild_c = Rc::clone(&rebuild);
            let toast_overlay_c = toast_overlay.clone();

            add_tool_btn.connect_clicked(move |_| {
                let rebuild_w = Rc::downgrade(&rebuild_c);
                show_tool_dialog(&config_c, &game_id, None, &rebuild_w, &toast_overlay_c);
            });
        }

        // Initial build
        (rebuild.borrow())();

        content_box.append(&tools_group);
    } else {
        // No game selected
        let status_page = adw::StatusPage::builder()
            .title("No Game Selected")
            .description("Select a game to configure its external tools.")
            .icon_name("applications-games-symbolic")
            .vexpand(true)
            .build();
        content_box.append(&status_page);
    }

    clamp.set_child(Some(&content_box));
    scrolled.set_child(Some(&clamp));
    toast_overlay.set_child(Some(&scrolled));
    toolbar_view.set_content(Some(&toast_overlay));

    toolbar_view.upcast()
}

/// Show a dialog to add or edit a tool.
fn show_tool_dialog(
    config: &Rc<RefCell<AppConfig>>,
    game_id: &str,
    tool_id: Option<&str>,
    rebuild: &std::rc::Weak<RefCell<Box<dyn Fn()>>>,
    toast_overlay: &adw::ToastOverlay,
) {
    let dialog = adw::Dialog::new();
    dialog.set_title(if tool_id.is_some() { "Edit Tool" } else { "Add Tool" });

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_show_title(false);
    toolbar_view.add_top_bar(&header);

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content_box.set_margin_top(12);
    content_box.set_margin_bottom(12);
    content_box.set_margin_start(12);
    content_box.set_margin_end(12);

    let preferences_group = adw::PreferencesGroup::new();

    // Tool Name
    let name_row = adw::EntryRow::builder().title("Tool Name").build();

    // Executable Path
    let exe_row = adw::ActionRow::builder()
        .title("Executable Path")
        .subtitle("Windows .exe inside a mod or game folder")
        .activatable(true)
        .build();

    let exe_label = gtk4::Label::new(Some("Not selected"));
    exe_label.add_css_class("dim-label");
    exe_label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    exe_row.add_suffix(&exe_label);

    let exe_path_ref: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));

    {
        let exe_path_ref_c = Rc::clone(&exe_path_ref);
        let exe_label_c = exe_label.clone();

        exe_row.connect_activated(move |_| {
            let file_dialog = gtk4::FileDialog::new();
            file_dialog.set_title("Select Executable");

            let exe_path_ref_c2 = Rc::clone(&exe_path_ref_c);
            let exe_label_c2 = exe_label_c.clone();

            file_dialog.open(
                gtk4::Window::NONE,
                gio::Cancellable::NONE,
                move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path() {
                            exe_label_c2.set_label(&path.to_string_lossy());
                            *exe_path_ref_c2.borrow_mut() = Some(path);
                        }
                },
            );
        });
    }

    // Arguments
    let args_row = adw::EntryRow::builder()
        .title("Arguments")
        .build();

    // App ID (get from current game)
    let app_id: u32 = {
        let cfg = config.borrow();
        cfg.current_game()
            .and_then(|g| g.kind.steam_app_id())
            .unwrap_or(0)
    };

    let app_id_row = adw::ActionRow::builder()
        .title("Steam App ID")
        .subtitle(format!("{}", app_id))
        .build();

    preferences_group.add(&name_row);
    preferences_group.add(&exe_row);
    preferences_group.add(&args_row);
    preferences_group.add(&app_id_row);

    content_box.append(&preferences_group);

    // If editing, populate fields
    if let Some(tool_id) = tool_id {
        let cfg = config.borrow();
        if let Some(game_settings) = cfg.game_settings.get(game_id)
            && let Some(tool) = game_settings.tools.iter().find(|t| t.id == tool_id) {
                name_row.set_text(&tool.name);
                args_row.set_text(&tool.arguments);
                exe_label.set_label(&tool.exe_path.to_string_lossy());
                *exe_path_ref.borrow_mut() = Some(tool.exe_path.clone());
            }
    }

    // Buttons
    let button_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    button_box.set_halign(gtk4::Align::End);
    button_box.set_margin_top(12);

    let cancel_btn = gtk4::Button::with_label("Cancel");
    let save_btn = gtk4::Button::with_label("Save");
    save_btn.add_css_class("suggested-action");

    button_box.append(&cancel_btn);
    button_box.append(&save_btn);
    content_box.append(&button_box);

    toolbar_view.set_content(Some(&content_box));
    dialog.set_child(Some(&toolbar_view));

    // Cancel button handler
    {
        let dialog_c = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_c.close();
        });
    }

    // Save button handler
    {
        let dialog_c = dialog.clone();
        let config_c = Rc::clone(config);
        let game_id = game_id.to_string();
        let rebuild = rebuild.clone();
        let toast_overlay = toast_overlay.clone();
        let tool_id_opt = tool_id.map(|s| s.to_string());

        save_btn.connect_clicked(move |_| {
            let name = name_row.text().to_string();
            let args = args_row.text().to_string();
            let exe_path_opt = exe_path_ref.borrow().clone();

            if name.is_empty() {
                toast_overlay.add_toast(adw::Toast::new("Tool name is required"));
                return;
            }

            let exe_path = match exe_path_opt {
                Some(p) => p,
                None => {
                    toast_overlay.add_toast(adw::Toast::new("Executable path is required"));
                    return;
                }
            };

            let mut cfg = config_c.borrow_mut();
            let game_settings = cfg.game_settings_mut(&game_id);

            if let Some(ref tid) = tool_id_opt {
                // Edit existing tool
                if let Some(tool) = game_settings.tools.iter_mut().find(|t| &t.id == tid) {
                    tool.name = name;
                    tool.exe_path = exe_path;
                    tool.arguments = args;
                    tool.app_id = app_id;
                }
            } else {
                // Add new tool
                let id = format!("tool_{}", std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis());

                let tool = ToolConfig {
                    id,
                    name,
                    exe_path,
                    arguments: args,
                    app_id,
                };
                game_settings.tools.push(tool);
            }

            cfg.save();
            drop(cfg);

            if let Some(rb) = rebuild.upgrade() {
                (rb.borrow())();
            }

            dialog_c.close();
        });
    }

    // Show the dialog
    if let Some(window) = toast_overlay.root().and_then(|r| r.downcast::<gtk4::Window>().ok()) {
        dialog.present(Some(&window));
    }
}

/// Launch a tool with Proton.
fn launch_tool(tool: &ToolConfig, btn: &gtk4::Button, toast_overlay: &adw::ToastOverlay) {
    btn.set_sensitive(false);

    let tool_clone = tool.clone();
    let btn_c = btn.clone();
    let toast_overlay_c = toast_overlay.clone();

    let (tx, rx) = mpsc::channel::<Result<String, String>>();

    // Spawn a thread to launch the tool
    thread::spawn(move || {
        log::info!("Launching tool: {}", tool_clone.name);

        match crate::core::steam::launch_tool_with_proton(
            &tool_clone.exe_path,
            &tool_clone.arguments,
            tool_clone.app_id,
        ) {
            Ok(mut child) => {
                // Capture and log stdout/stderr
                if let Some(stdout) = child.stdout.take() {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines().flatten() {
                        log::info!("[{}] {}", tool_clone.name, line);
                    }
                }

                if let Some(stderr) = child.stderr.take() {
                    let reader = BufReader::new(stderr);
                    for line in reader.lines().flatten() {
                        log::warn!("[{}] {}", tool_clone.name, line);
                    }
                }

                match child.wait() {
                    Ok(status) => {
                        if status.success() {
                            let _ = tx.send(Ok(format!("{} exited successfully", tool_clone.name)));
                        } else {
                            let _ = tx.send(Err(format!(
                                "{} exited with status: {}",
                                tool_clone.name, status
                            )));
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(format!("Failed to wait for {}: {}", tool_clone.name, e)));
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(Err(format!("Failed to launch {}: {}", tool_clone.name, e)));
            }
        }
    });

    // Poll for completion
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        match rx.try_recv() {
            Ok(Ok(msg)) => {
                btn_c.set_sensitive(true);
                toast_overlay_c.add_toast(adw::Toast::new(&msg));
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                btn_c.set_sensitive(true);
                log::error!("{}", e);
                toast_overlay_c.add_toast(adw::Toast::new(&e));
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                btn_c.set_sensitive(true);
                glib::ControlFlow::Break
            }
        }
    });
}
