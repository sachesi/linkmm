use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::{AppConfig, ToolConfig, ToolPresetKind, ToolRunProfile};
use crate::core::games::{Game, GameLauncherSource};
use crate::core::mods::{ModDatabase, ModManager};
use crate::core::runtime::{SessionStatus, global_runtime_manager};

/// Build the Tools page for managing external Windows tools (e.g., BodySlide, xEdit).
pub fn build_tools_page(game: Option<&Game>, config: Rc<RefCell<AppConfig>>) -> gtk4::Widget {
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

    if let Some(game) = game.cloned() {
        let profile_group = adw::PreferencesGroup::builder()
            .title("Active Profile")
            .description("Tool runs and generated output management are scoped to this profile.")
            .build();
        let profile_row = adw::ComboRow::new();
        profile_row.set_title("Profile");
        let profile_names = gtk4::StringList::new(&[]);
        {
            let cfg = config.borrow();
            if let Some(gs) = cfg.game_settings.get(&game.id) {
                for p in &gs.profiles {
                    profile_names.append(&p.name);
                }
                if let Some(idx) = gs
                    .profiles
                    .iter()
                    .position(|p| p.id == gs.active_profile_id)
                    .map(|i| i as u32)
                {
                    profile_row.set_selected(idx);
                }
            }
        }
        profile_row.set_model(Some(&profile_names));
        profile_group.add(&profile_row);

        // Tools group
        let tools_group = adw::PreferencesGroup::builder()
            .title("External Tools")
            .description(format!(
                "Configure Windows-native utilities for {}",
                game.name
            ))
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

        let generated_group = adw::PreferencesGroup::builder()
            .title("Generated Outputs")
            .description("Managed output packages produced by tool runs.")
            .build();
        let cleanup_generated_btn = gtk4::Button::new();
        cleanup_generated_btn.set_icon_name("edit-clear-symbolic");
        cleanup_generated_btn.add_css_class("flat");
        cleanup_generated_btn.set_tooltip_text(Some("Cleanup stale generated outputs"));
        generated_group.set_header_suffix(Some(&cleanup_generated_btn));
        let generated_list = gtk4::ListBox::new();
        generated_list.add_css_class("boxed-list");
        generated_list.set_selection_mode(gtk4::SelectionMode::None);
        generated_group.add(&generated_list);

        // Rebuild function to refresh the tool list
        let rebuild: Rc<RefCell<Box<dyn Fn()>>> = Rc::new(RefCell::new(Box::new(|| {})));
        let rebuild_weak = Rc::downgrade(&rebuild);

        {
            let tools_list_c = tools_list.clone();
            let config_c = Rc::clone(&config);
            let game_id = game.id.clone();
            let toast_overlay_c = toast_overlay.clone();
            let generated_list_c = generated_list.clone();
            let game_for_generated = game.clone();
            let game_for_rebuild = game.clone();

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
                        let game_for_launch = game_for_rebuild.clone();
                        let config_for_launch = Rc::clone(&config_c);
                        launch_btn.connect_clicked(move |btn| {
                            launch_tool(
                                &game_for_launch,
                                &tool_clone,
                                Rc::clone(&config_for_launch),
                                btn,
                                &toast_overlay_c2,
                            );
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

                    let adopt_btn = gtk4::Button::new();
                    adopt_btn.set_icon_name("folder-download-symbolic");
                    adopt_btn.add_css_class("flat");
                    adopt_btn.set_tooltip_text(Some("Adopt unmanaged generated output"));
                    {
                        let tool_for_adopt = tool.clone();
                        let game_for_adopt = game_for_rebuild.clone();
                        let rebuild_adopt = rebuild_weak.clone();
                        adopt_btn.connect_clicked(move |_| {
                            let profile = tool_for_adopt.primary_profile();
                            let mut db = ModDatabase::load(&game_for_adopt);
                            match crate::core::tool_runs::detect_unmanaged_outputs(
                                &game_for_adopt,
                                &db,
                                &tool_for_adopt,
                                &profile,
                            ) {
                                Ok(files) if files.is_empty() => {
                                    log::info!(
                                        "No unmanaged output candidates found for {}",
                                        tool_for_adopt.name
                                    );
                                }
                                Ok(files) => {
                                    if let Err(e) = crate::core::tool_runs::adopt_unmanaged_outputs(
                                        &game_for_adopt,
                                        &mut db,
                                        &tool_for_adopt,
                                        &profile,
                                        &files,
                                    ) {
                                        log::error!("Failed adopting unmanaged outputs: {e}");
                                    }
                                }
                                Err(e) => log::error!("Unmanaged output detection failed: {e}"),
                            }
                            if let Some(rb) = rebuild_adopt.upgrade() {
                                (rb.borrow())();
                            }
                        });
                    }
                    row.add_suffix(&adopt_btn);

                    tools_list_c.append(&row);
                }

                while let Some(child) = generated_list_c.first_child() {
                    generated_list_c.remove(&child);
                }
                let db = ModDatabase::load(&game_for_generated);
                if db.generated_outputs.is_empty() {
                    let row = adw::ActionRow::builder()
                        .title("No generated outputs captured")
                        .build();
                    row.set_sensitive(false);
                    generated_list_c.append(&row);
                } else {
                    for output in &db.generated_outputs {
                        let subtitle =
                            format!("Tool: {} · Profile: {}", output.tool_id, output.run_profile);
                        let row = adw::ActionRow::builder()
                            .title(&output.name)
                            .subtitle(&subtitle)
                            .build();
                        let remove_btn = gtk4::Button::new();
                        remove_btn.set_icon_name("user-trash-symbolic");
                        remove_btn.add_css_class("flat");
                        remove_btn.add_css_class("destructive-action");
                        let output_id = output.id.clone();
                        let game_for_remove = game_for_generated.clone();
                        let rebuild_remove = rebuild_weak.clone();
                        remove_btn.connect_clicked(move |_| {
                            let mut db = ModDatabase::load(&game_for_remove);
                            if let Err(e) =
                                crate::core::generated_outputs::remove_generated_output_package(
                                    &game_for_remove,
                                    &mut db,
                                    &output_id,
                                )
                            {
                                log::error!("Failed to remove generated output package: {e}");
                            }
                            let _ = ModManager::rebuild_all(&game_for_remove);
                            if let Some(rb) = rebuild_remove.upgrade() {
                                (rb.borrow())();
                            }
                        });
                        row.add_suffix(&remove_btn);
                        generated_list_c.append(&row);
                    }
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

        {
            let game_cleanup = game.clone();
            let rebuild_cleanup = Rc::clone(&rebuild);
            cleanup_generated_btn.connect_clicked(move |_| {
                let mut db = ModDatabase::load(&game_cleanup);
                crate::core::generated_outputs::cleanup_stale_generated_outputs(
                    &game_cleanup,
                    &mut db,
                );
                let _ = ModManager::rebuild_all(&game_cleanup);
                (rebuild_cleanup.borrow())();
            });
        }

        {
            let config_profile = Rc::clone(&config);
            let game_profile = game.clone();
            let rebuild_profile = Rc::clone(&rebuild);
            profile_row.connect_selected_notify(move |row| {
                if global_runtime_manager().any_active() {
                    return;
                }
                let selected = row.selected() as usize;
                let mut cfg = config_profile.borrow_mut();
                let selected_profile_id = {
                    let gs = cfg.game_settings_mut(&game_profile.id);
                    gs.profiles.get(selected).map(|p| p.id.clone())
                };
                if let Some(profile_id) = selected_profile_id {
                    cfg.game_settings_mut(&game_profile.id).active_profile_id = profile_id.clone();
                    cfg.save();
                    if let Err(e) = ModManager::switch_profile(&game_profile, &profile_id) {
                        log::error!("Failed switching profile: {e}");
                    }
                    (rebuild_profile.borrow())();
                }
            });
        }

        {
            let profile_row_c = profile_row.clone();
            let add_tool_btn_c = add_tool_btn.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
                let locked = global_runtime_manager().any_active();
                profile_row_c.set_sensitive(!locked);
                add_tool_btn_c.set_sensitive(!locked);
                glib::ControlFlow::Continue
            });
        }

        // Initial build
        (rebuild.borrow())();

        content_box.append(&profile_group);
        content_box.append(&tools_group);
        content_box.append(&generated_group);
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
    dialog.set_title(if tool_id.is_some() {
        "Edit Tool"
    } else {
        "Add Tool"
    });

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

            file_dialog.open(gtk4::Window::NONE, gio::Cancellable::NONE, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    exe_label_c2.set_label(&path.to_string_lossy());
                    *exe_path_ref_c2.borrow_mut() = Some(path);
                }
            });
        });
    }

    // Arguments
    let args_row = adw::EntryRow::builder().title("Arguments").build();

    // App ID (get from current game)
    let app_id: u32 = {
        let cfg = config.borrow();
        cfg.current_game()
            .and_then(|g| {
                if g.launcher_source == GameLauncherSource::Steam {
                    g.steam_instance_app_id()
                } else {
                    g.kind.primary_steam_app_id()
                }
            })
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
            && let Some(tool) = game_settings.tools.iter().find(|t| t.id == tool_id)
        {
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
                let id = format!(
                    "tool_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis()
                );

                let tool = ToolConfig {
                    id,
                    name: name.clone(),
                    exe_path,
                    arguments: args,
                    app_id,
                    preset: infer_tool_preset(&name),
                    run_profiles: default_profiles_for_name(&name),
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
    if let Some(window) = toast_overlay
        .root()
        .and_then(|r| r.downcast::<gtk4::Window>().ok())
    {
        dialog.present(Some(&window));
    }
}

fn infer_tool_preset(name: &str) -> ToolPresetKind {
    let lower = name.to_lowercase();
    if lower.contains("bodyslide") {
        ToolPresetKind::BodySlide
    } else if lower.contains("pandora") {
        ToolPresetKind::Pandora
    } else if lower.contains("nemesis") {
        ToolPresetKind::Nemesis
    } else {
        ToolPresetKind::Generic
    }
}

fn default_profiles_for_name(name: &str) -> Vec<ToolRunProfile> {
    use crate::core::tool_adapters::adapter_for_tool;
    let tool = ToolConfig {
        id: "preset_probe".to_string(),
        name: name.to_string(),
        exe_path: std::path::PathBuf::new(),
        arguments: String::new(),
        app_id: 0,
        preset: infer_tool_preset(name),
        run_profiles: Vec::new(),
    };
    adapter_for_tool(&tool).default_profiles(&tool)
}

/// Launch a tool in the selected game instance context.
///
/// Both Steam and UMU games go through the managed session path so that
/// tool output (BodySlide meshes, Nemesis animations, etc.) is captured and
/// registered as a generated-output package regardless of launcher type.
fn launch_tool(
    game: &Game,
    tool: &ToolConfig,
    config: Rc<RefCell<AppConfig>>,
    btn: &gtk4::Button,
    toast_overlay: &adw::ToastOverlay,
) {
    let manager = global_runtime_manager();
    if let Some(active) = manager.current_tool_session(&game.id, &tool.id) {
        if let Err(e) = manager.stop_session(active.id) {
            toast_overlay.add_toast(adw::Toast::new(&e));
        }
        return;
    }

    let active_profile_id = {
        let cfg = config.borrow();
        cfg.game_settings
            .get(&game.id)
            .map(|gs| gs.active_profile_id.clone())
            .unwrap_or_else(|| "default".to_string())
    };
    let profile = tool.primary_profile();
    let (session_id, rx) = match manager.start_tool_session(
        game.clone(),
        active_profile_id,
        tool.clone(),
        profile.clone(),
    ) {
        Ok(v) => v,
        Err(e) => {
            toast_overlay.add_toast(adw::Toast::new(&e));
            return;
        }
    };
    log::info!("Started managed tool session {}", session_id);
    btn.set_sensitive(false);

    let btn_c = btn.clone();
    let game_id = game.id.clone();
    let tool_id = tool.id.clone();
    let tool_name = tool.name.clone();
    let toast_overlay_c = toast_overlay.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
        let manager = global_runtime_manager();
        if let Some(s) = manager.current_tool_session(&game_id, &tool_id)
            && matches!(s.status, SessionStatus::Running | SessionStatus::Starting)
        {
            btn_c.set_icon_name("media-playback-stop-symbolic");
            btn_c.set_tooltip_text(Some("Stop Tool"));
            btn_c.set_sensitive(true);
            return glib::ControlFlow::Continue;
        }
        match rx.try_recv() {
            Ok(Ok(run)) => {
                let msg = if let Some(pkg) = run.package_id {
                    format!("{} complete; generated package {}", tool_name, pkg)
                } else {
                    format!("{} complete", tool_name)
                };
                toast_overlay_c.add_toast(adw::Toast::new(&msg));
                btn_c.set_icon_name("media-playback-start-symbolic");
                btn_c.set_tooltip_text(Some("Launch Tool"));
                btn_c.set_sensitive(true);
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                toast_overlay_c.add_toast(adw::Toast::new(&format!("Tool run failed: {e}")));
                btn_c.set_icon_name("media-playback-start-symbolic");
                btn_c.set_tooltip_text(Some("Launch Tool"));
                btn_c.set_sensitive(true);
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                btn_c.set_icon_name("media-playback-start-symbolic");
                btn_c.set_tooltip_text(Some("Launch Tool"));
                btn_c.set_sensitive(true);
                glib::ControlFlow::Break
            }
        }
    });
}
