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
use crate::core::workspace::{self, ProfileSwitchPolicy};
use crate::ui::workspace_events;

/// Build the Tools page for managing external Windows tools (e.g., BodySlide, xEdit).
pub fn build_tools_page(game: Option<&Game>, config: Rc<RefCell<AppConfig>>) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let title_widget = adw::WindowTitle::new("Tools", "Run and configure external tools");
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
            .description("Tool runs are scoped to this profile.")
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

        let workspace_group = adw::PreferencesGroup::builder()
            .title("Workflow Handoff")
            .description("Review/apply/recovery lives on Workspace.")
            .build();
        let workspace_row = adw::ActionRow::builder()
            .title("Workspace")
            .subtitle(
                "Use Workspace to review staged changes, runtime status, backups, and integrity.",
            )
            .build();
        workspace_group.add(&workspace_row);
        let last_run_row = adw::ActionRow::builder()
            .title("Last Tool Run")
            .subtitle("No runs recorded in this session.")
            .build();
        workspace_group.add(&last_run_row);

        let last_captured_package_id = Rc::new(RefCell::new(None::<String>));
        let rebuild: Rc<RefCell<Box<dyn Fn()>>> = Rc::new(RefCell::new(Box::new(|| {})));
        let rebuild_weak = Rc::downgrade(&rebuild);

        {
            let tools_list_c = tools_list.clone();
            let config_c = Rc::clone(&config);
            let game_id = game.id.clone();
            let toast_overlay_c = toast_overlay.clone();
            let game_for_rebuild = game.clone();
            let last_run_row_c = last_run_row.clone();
            let last_captured_package_id_c = Rc::clone(&last_captured_package_id);

            *rebuild.borrow_mut() = Box::new(move || {
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
                        let last_run_row_inner = last_run_row_c.clone();
                        let rebuild_after_run = rebuild_weak.clone();
                        let last_pkg_id = Rc::clone(&last_captured_package_id_c);
                        launch_btn.connect_clicked(move |btn| {
                            launch_tool(
                                &game_for_launch,
                                &tool_clone,
                                Rc::clone(&config_for_launch),
                                btn,
                                &toast_overlay_c2,
                                Some(&last_run_row_inner),
                                rebuild_after_run.clone(),
                                Rc::clone(&last_pkg_id),
                            );
                        });
                    }
                    row.add_suffix(&launch_btn);

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
            let config_profile = Rc::clone(&config);
            let game_profile = game.clone();
            let rebuild_profile = Rc::clone(&rebuild);
            profile_row.connect_selected_notify(move |row| {
                if global_runtime_manager().any_active() {
                    return;
                }
                let selected = row.selected() as usize;
                let workspace_state = workspace::workspace_state_for_game(&game_profile);
                let (selected_profile_id, selected_profile_name, previous_profile_id, previous_idx) = {
                    let mut cfg = config_profile.borrow_mut();
                    let gs = cfg.game_settings_mut(&game_profile.id);
                    let selected_profile = gs.profiles.get(selected).cloned();
                    let prev_id = gs.active_profile_id.clone();
                    let prev_idx = gs
                        .profiles
                        .iter()
                        .position(|p| p.id == prev_id)
                        .map(|v| v as u32)
                        .unwrap_or(0);
                    (
                        selected_profile.clone().map(|p| p.id),
                        selected_profile.map(|p| p.name),
                        prev_id,
                        prev_idx,
                    )
                };
                let Some(profile_id) = selected_profile_id else {
                    row.set_selected(previous_idx);
                    return;
                };
                if profile_id == previous_profile_id {
                    return;
                }

                match workspace::profile_switch_policy(&workspace_state) {
                    ProfileSwitchPolicy::Blocked(reason) => {
                        log::warn!("Profile switch blocked: {reason}");
                        row.set_selected(previous_idx);
                    }
                    ProfileSwitchPolicy::Warn(reason) => {
                        let parent = row.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
                        let target = selected_profile_name
                            .as_deref()
                            .unwrap_or("selected profile");
                        let dialog = adw::AlertDialog::builder()
                            .heading("Switch Profile?")
                            .body(format!(
                                "{reason}

Switching to “{target}” may leave undeployed changes unapplied."
                            ))
                            .build();
                        dialog.add_response("cancel", "Cancel");
                        dialog.add_response("switch", "Switch Anyway");
                        dialog.set_response_appearance("switch", adw::ResponseAppearance::Destructive);
                        dialog.set_default_response(Some("cancel"));
                        dialog.set_close_response("cancel");
                        let row_c = row.clone();
                        let config_c = Rc::clone(&config_profile);
                        let game_c = game_profile.clone();
                        let rebuild_c = Rc::clone(&rebuild_profile);
                        let profile_id_c = profile_id.clone();
                        dialog.connect_response(None, move |_, response| {
                            if response == "switch" {
                                apply_profile_switch(
                                    &config_c,
                                    &game_c,
                                    &profile_id_c,
                                    &rebuild_c,
                                );
                            } else {
                                row_c.set_selected(previous_idx);
                            }
                        });
                        dialog.present(parent.as_ref());
                    }
                    ProfileSwitchPolicy::Allowed => apply_profile_switch(
                        &config_profile,
                        &game_profile,
                        &profile_id,
                        &rebuild_profile,
                    ),
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

        {
            let game_workspace = game.clone();
            let workspace_row_c = workspace_row.clone();
            let refresh_workspace_row = move || {
                let state = workspace::workspace_state_for_game(&game_workspace);
                let summary = workspace::format_workspace_compact_summary(&state);
                workspace_row_c.set_subtitle(&summary);
            };
            refresh_workspace_row();
            workspace_events::attach_workspace_event_listener(move |event| {
                if matches!(
                    event,
                    workspace::WorkspaceEvent::WorkspaceStateChanged { .. }
                        | workspace::WorkspaceEvent::ProfileStateChanged { .. }
                        | workspace::WorkspaceEvent::ProfileSwitched { .. }
                        | workspace::WorkspaceEvent::DeployFinished { .. }
                        | workspace::WorkspaceEvent::DeployFailed { .. }
                        | workspace::WorkspaceEvent::RevertCompleted { .. }
                ) {
                    refresh_workspace_row();
                }
            });
        }

        {
            let game_events = game.clone();
            let rebuild_events = Rc::downgrade(&rebuild);
            workspace_events::attach_workspace_event_listener(move |event| {
                let relevant = match event {
                    workspace::WorkspaceEvent::ProfileStateChanged { game_id, .. }
                    | workspace::WorkspaceEvent::DeployFinished { game_id, .. }
                    | workspace::WorkspaceEvent::DeployFailed { game_id, .. }
                    | workspace::WorkspaceEvent::RevertCompleted { game_id, .. }
                    | workspace::WorkspaceEvent::ProfileSwitched { game_id, .. } => {
                        game_id == game_events.id
                    }
                    workspace::WorkspaceEvent::DeployStarted { .. }
                    | workspace::WorkspaceEvent::WorkspaceStateChanged { .. } => false,
                };
                if relevant && let Some(rb) = rebuild_events.upgrade() {
                    (rb.borrow())();
                }
            });
        }

        (rebuild.borrow())();

        content_box.append(&profile_group);
        content_box.append(&workspace_group);
        content_box.append(&tools_group);
    } else {
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

fn apply_profile_switch(
    config: &Rc<RefCell<AppConfig>>,
    game: &Game,
    profile_id: &str,
    rebuild: &Rc<RefCell<Box<dyn Fn()>>>,
) {
    let mut cfg = config.borrow_mut();
    cfg.game_settings_mut(&game.id).active_profile_id = profile_id.to_string();
    cfg.save();
    if let Err(e) = ModManager::switch_profile(game, profile_id) {
        log::error!("Failed switching profile: {e}");
    }
    (rebuild.borrow())();
}

/// Launch a tool in the selected game instance context.
fn launch_tool(
    game: &Game,
    tool: &ToolConfig,
    config: Rc<RefCell<AppConfig>>,
    btn: &gtk4::Button,
    toast_overlay: &adw::ToastOverlay,
    last_run_row: Option<&adw::ActionRow>,
    rebuild_after_run: std::rc::Weak<RefCell<Box<dyn Fn()>>>,
    last_captured_package_id: Rc<RefCell<Option<String>>>,
) {
    if game.launcher_source == GameLauncherSource::Steam {
        let app_id = game.steam_instance_app_id().unwrap_or(tool.app_id);
        match crate::core::steam::launch_tool_with_proton(&tool.exe_path, &tool.arguments, app_id) {
            Ok(_child) => {
                if let Some(row) = last_run_row {
                    row.set_subtitle("Launched via Steam/Proton (capture unavailable).");
                }
                toast_overlay.add_toast(adw::Toast::new("Tool launched via Steam/Proton"))
            }
            Err(e) => toast_overlay.add_toast(adw::Toast::new(&format!("Launch failed: {e}"))),
        }
        return;
    }

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
        active_profile_id.clone(),
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
    workspace::mark_operation(
        &game.id,
        &active_profile_id,
        workspace::WorkspaceOperation::ToolRun,
    );
    btn.set_sensitive(false);

    let btn_c = btn.clone();
    let game_id = game.id.clone();
    let tool_id = tool.id.clone();
    let tool_name = tool.name.clone();
    let toast_overlay_c = toast_overlay.clone();
    let last_run_row_c = last_run_row.cloned();
    let active_profile_id_c = active_profile_id.clone();
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
                let msg = if let Some(ref pkg) = run.package_id {
                    format!("{} complete; generated package {}", tool_name, pkg)
                } else {
                    format!("{} complete", tool_name)
                };
                workspace::mark_operation(
                    &game_id,
                    &active_profile_id_c,
                    workspace::WorkspaceOperation::None,
                );
                workspace::set_status(
                    &game_id,
                    &active_profile_id_c,
                    workspace::StatusSeverity::Info,
                    msg.clone(),
                );
                if let Some(row) = &last_run_row_c {
                    row.set_subtitle(&format!(
                        "Captured {} files (plugins: {}, assets: {})",
                        run.captured_files, run.plugin_files, run.asset_files
                    ));
                }
                *last_captured_package_id.borrow_mut() = run.package_id.clone();
                if run.captured_files > 0 {
                    workspace::set_status(
                        &game_id,
                        &active_profile_id_c,
                        workspace::StatusSeverity::Info,
                        "Outputs captured. Review packages below and redeploy.".to_string(),
                    );
                }
                toast_overlay_c.add_toast(adw::Toast::new(&msg));
                btn_c.set_icon_name("media-playback-start-symbolic");
                btn_c.set_tooltip_text(Some("Launch Tool"));
                btn_c.set_sensitive(true);
                if let Some(rb) = rebuild_after_run.upgrade() {
                    (rb.borrow())();
                }
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                workspace::mark_operation(
                    &game_id,
                    &active_profile_id_c,
                    workspace::WorkspaceOperation::None,
                );
                workspace::set_status(
                    &game_id,
                    &active_profile_id_c,
                    workspace::StatusSeverity::Error,
                    format!("Tool run failed: {e}"),
                );
                if let Some(row) = &last_run_row_c {
                    row.set_subtitle(&format!("Failed: {e}"));
                }
                toast_overlay_c.add_toast(adw::Toast::new(&format!("Tool run failed: {e}")));
                btn_c.set_icon_name("media-playback-start-symbolic");
                btn_c.set_tooltip_text(Some("Launch Tool"));
                btn_c.set_sensitive(true);
                if let Some(rb) = rebuild_after_run.upgrade() {
                    (rb.borrow())();
                }
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                workspace::mark_operation(
                    &game_id,
                    &active_profile_id_c,
                    workspace::WorkspaceOperation::None,
                );
                btn_c.set_icon_name("media-playback-start-symbolic");
                btn_c.set_tooltip_text(Some("Launch Tool"));
                btn_c.set_sensitive(true);
                if let Some(rb) = rebuild_after_run.upgrade() {
                    (rb.borrow())();
                }
                glib::ControlFlow::Break
            }
        }
    });
}
