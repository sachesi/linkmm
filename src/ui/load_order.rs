use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::deployment;
use crate::core::games::Game;
use crate::core::mods::{ModDatabase, ModManager, PluginFile};
use crate::core::workspace;
use crate::ui::ordering;

#[derive(Debug, Clone)]
struct LoadOrderStageSummary {
    summary: String,
    redeploy_available: bool,
    discard_available: bool,
}

fn is_event_for_game(event: &workspace::WorkspaceEvent, game_id: &str) -> bool {
    match event {
        workspace::WorkspaceEvent::ProfileStateChanged { game_id: id, .. }
        | workspace::WorkspaceEvent::WorkspaceStateChanged { game_id: id, .. }
        | workspace::WorkspaceEvent::DeployStarted { game_id: id, .. }
        | workspace::WorkspaceEvent::DeployFinished { game_id: id, .. }
        | workspace::WorkspaceEvent::DeployFailed { game_id: id, .. }
        | workspace::WorkspaceEvent::ProfileSwitched { game_id: id, .. }
        | workspace::WorkspaceEvent::RevertCompleted { game_id: id, .. } => id == game_id,
    }
}

fn attach_load_order_workspace_listener<F>(mut on_event: F)
where
    F: FnMut(workspace::WorkspaceEvent) + 'static,
{
    let rx = workspace::subscribe_events();
    let (tx_ui, rx_ui) = std::sync::mpsc::channel::<workspace::WorkspaceEvent>();
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            if tx_ui.send(event).is_err() {
                break;
            }
        }
    });
    gtk4::glib::idle_add_local(move || {
        while let Ok(event) = rx_ui.try_recv() {
            on_event(event);
        }
        gtk4::glib::ControlFlow::Continue
    });
}

fn load_order_stage_summary(game: &Game) -> LoadOrderStageSummary {
    let state = workspace::workspace_state_for_game(game);
    let db = ModDatabase::load(game);
    let preview = deployment::deployment_preview(game, &db).ok();
    let plugin_changes = if state.pending_changes.plugin_order_changed {
        "plugins.txt will change on redeploy"
    } else {
        "plugins.txt unchanged"
    };
    let replace_count = preview
        .as_ref()
        .map(|p| p.links_to_replace.len())
        .unwrap_or(0);
    let mut summary = format!(
        "{} · {} · Asset links to replace: {}",
        workspace::format_workspace_compact_summary(&state),
        plugin_changes,
        replace_count
    );
    if let Some(example) = state.integrity_examples.first() {
        summary.push_str(" · Integrity example: ");
        summary.push_str(example);
    }
    LoadOrderStageSummary {
        summary,
        redeploy_available: state.safe_redeploy_required,
        discard_available: state.safe_redeploy_required
            && workspace::has_profile_baseline(game, &state.profile_id),
    }
}

/// Build the Load Order page for `game`.
///
/// Shows all `.esm` / `.esl` / `.esp` files found in the game's `Data`
/// directory, ordered by `ModDatabase::get_ordered_plugins`.  Vanilla masters
/// are pinned at the top and cannot be moved.
pub fn build_load_order_page(game: Option<&Game>) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let title_widget = adw::WindowTitle::new("Load Order", "");
    header.set_title_widget(Some(&title_widget));

    let search_entry = gtk4::SearchEntry::new();
    search_entry.set_placeholder_text(Some("Search plugins"));
    search_entry.set_width_chars(24);
    search_entry.set_sensitive(game.is_some());
    header.pack_start(&search_entry);

    // Sort button – sorts non-vanilla plugins using LOOT metadata.
    let sort_btn = gtk4::Button::new();
    sort_btn.set_icon_name("view-sort-ascending-symbolic");
    sort_btn.set_sensitive(game.is_some());
    header.pack_end(&sort_btn);

    toolbar_view.add_top_bar(&header);

    let content_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content_container.set_vexpand(true);

    match game {
        None => {
            sort_btn.set_tooltip_text(Some("No game selected"));
            let status = adw::StatusPage::builder()
                .title("No Game Selected")
                .description("Select a game from the sidebar to manage its load order.")
                .icon_name("applications-games-symbolic")
                .build();
            status.set_vexpand(true);
            content_container.append(&status);
        }
        Some(g) => {
            sort_btn.set_tooltip_text(Some("Sort plugins using LOOT load order rules"));
            let game_rc = Rc::new(g.clone());
            let search_query = Rc::new(RefCell::new(String::new()));

            let staged_row = adw::ActionRow::builder()
                .title("Staged Plugin Changes")
                .subtitle("Loading staged load-order state…")
                .build();
            let staged_redeploy_btn = gtk4::Button::with_label("Redeploy now");
            let staged_discard_btn = gtk4::Button::with_label("Discard staged");
            staged_row.add_suffix(&staged_redeploy_btn);
            staged_row.add_suffix(&staged_discard_btn);
            content_container.append(&staged_row);

            let reorder_hint = gtk4::Label::new(Some("Clear search to reorder."));
            reorder_hint.add_css_class("dim-label");
            reorder_hint.set_margin_top(8);
            reorder_hint.set_margin_bottom(4);
            reorder_hint.set_margin_start(16);
            reorder_hint.set_margin_end(16);
            reorder_hint.set_halign(gtk4::Align::Start);
            reorder_hint.set_visible(false);
            content_container.append(&reorder_hint);

            let stack = gtk4::Stack::new();
            stack.set_vexpand(true);

            let list_box = gtk4::ListBox::new();
            list_box.add_css_class("boxed-list");
            list_box.set_selection_mode(gtk4::SelectionMode::None);

            let clamp = adw::Clamp::new();
            clamp.set_maximum_size(900);
            clamp.set_margin_top(12);
            clamp.set_margin_bottom(12);
            clamp.set_margin_start(12);
            clamp.set_margin_end(12);
            clamp.set_child(Some(&list_box));

            let scrolled = gtk4::ScrolledWindow::new();
            scrolled.set_vexpand(true);
            scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
            scrolled.set_child(Some(&clamp));

            let status_page = adw::StatusPage::new();
            status_page.set_vexpand(true);

            stack.add_named(&scrolled, Some("list"));
            stack.add_named(&status_page, Some("status"));
            stack.set_visible_child_name("list");
            content_container.append(&stack);

            {
                let game_c = Rc::clone(&game_rc);
                let list_c = list_box.clone();
                let status_c = status_page.clone();
                let stack_c = stack.clone();
                let search_c = Rc::clone(&search_query);
                let hint_c = reorder_hint.clone();
                search_entry.connect_search_changed(move |entry| {
                    *search_c.borrow_mut() = entry.text().to_string();
                    refresh_load_order_content(
                        &list_c,
                        &status_c,
                        &stack_c,
                        &hint_c,
                        &game_c,
                        &search_c.borrow(),
                    );
                });
            }

            {
                let game_c = Rc::clone(&game_rc);
                let list_c = list_box.clone();
                let status_c = status_page.clone();
                let stack_c = stack.clone();
                let search_c = Rc::clone(&search_query);
                let hint_c = reorder_hint.clone();
                sort_btn.connect_clicked(move |_| {
                    let mut db = ModDatabase::load(&game_c);
                    if db.plugin_load_order.is_empty() {
                        db.sync_from_plugins_txt(&game_c);
                    }
                    db.sort_plugins_by_type(&game_c);
                    db.save(&game_c);
                    refresh_load_order_content(
                        &list_c,
                        &status_c,
                        &stack_c,
                        &hint_c,
                        &game_c,
                        &search_c.borrow(),
                    );
                });
            }

            {
                let game_stage = Rc::clone(&game_rc);
                let staged_row_c = staged_row.clone();
                let staged_redeploy_btn_c = staged_redeploy_btn.clone();
                let staged_discard_btn_c = staged_discard_btn.clone();
                let refresh_stage = move || {
                    let stage = load_order_stage_summary(&game_stage);
                    staged_row_c.set_subtitle(&stage.summary);
                    staged_redeploy_btn_c.set_sensitive(stage.redeploy_available);
                    staged_discard_btn_c.set_sensitive(stage.discard_available);
                };
                refresh_stage();
                attach_load_order_workspace_listener(move |event| {
                    if is_event_for_game(&event, &game_stage.id) {
                        refresh_stage();
                    }
                });
            }

            {
                let game_c = Rc::clone(&game_rc);
                staged_redeploy_btn.connect_clicked(move |_| {
                    if let Err(e) = ModManager::rebuild_all(&game_c) {
                        log::error!("Load Order staged redeploy failed: {e}");
                    }
                });
            }

            {
                let game_c = Rc::clone(&game_rc);
                staged_discard_btn.connect_clicked(move |btn| {
                    let parent = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
                    let dialog = adw::AlertDialog::builder()
                        .heading("Discard staged changes?")
                        .body("This restores profile state to the last deployed baseline. Deployed files on disk are unchanged until explicit redeploy.")
                        .build();
                    dialog.add_response("cancel", "Cancel");
                    dialog.add_response("discard", "Discard");
                    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
                    dialog.set_default_response(Some("cancel"));
                    dialog.set_close_response("cancel");
                    let game_response = Rc::clone(&game_c);
                    dialog.connect_response(None, move |_, response| {
                        if response != "discard" {
                            return;
                        }
                        if let Err(e) = workspace::revert_active_profile_to_baseline(&game_response)
                        {
                            log::error!("Failed to discard staged changes from Load Order: {e}");
                        }
                    });
                    dialog.present(parent.as_ref());
                });
            }

            {
                let game_c = Rc::clone(&game_rc);
                let list_c = list_box.clone();
                let status_c = status_page.clone();
                let stack_c = stack.clone();
                let search_c = Rc::clone(&search_query);
                let hint_c = reorder_hint.clone();
                attach_load_order_workspace_listener(move |event| {
                    if is_event_for_game(&event, &game_c.id) {
                        refresh_load_order_content(
                            &list_c,
                            &status_c,
                            &stack_c,
                            &hint_c,
                            &game_c,
                            &search_c.borrow(),
                        );
                    }
                });
            }

            refresh_load_order_content(
                &list_box,
                &status_page,
                &stack,
                &reorder_hint,
                &game_rc,
                &search_query.borrow(),
            );
        }
    };

    toolbar_view.set_content(Some(&content_container));
    toolbar_view.upcast()
}

fn refresh_load_order_content(
    list_box: &gtk4::ListBox,
    status_page: &adw::StatusPage,
    stack: &gtk4::Stack,
    reorder_hint: &gtk4::Label,
    game: &Rc<Game>,
    search_query: &str,
) {
    let is_filtered = !search_query.trim().is_empty();
    reorder_hint.set_visible(is_filtered);

    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let mut db = ModDatabase::load(game);
    if db.plugin_load_order.is_empty() {
        db.sync_from_plugins_txt(game);
    }

    let ordered_plugins = db.get_ordered_plugins(game);
    if ordered_plugins.is_empty() {
        status_page.set_title("No Plugins Found");
        status_page.set_description(Some(
            "No .esm / .esl / .esp files were found in the game's Data directory.\nInstall and deploy mods first, or check that the game path is correct.",
        ));
        status_page.set_icon_name(Some("format-justify-left-symbolic"));
        stack.set_visible_child_name("status");
        return;
    }

    let filtered: Vec<PluginFile> = ordered_plugins
        .iter()
        .filter(|p| {
            matches_query(&p.name, search_query) || matches_query(p.kind.label(), search_query)
        })
        .cloned()
        .collect();

    if filtered.is_empty() {
        status_page.set_title("No Matching Plugins");
        status_page.set_description(Some("No plugins match your search."));
        status_page.set_icon_name(Some("system-search-symbolic"));
        stack.set_visible_child_name("status");
        return;
    }

    let pinned_prefix_len = ordered_plugins.iter().take_while(|p| p.is_vanilla).count();

    for plugin in &filtered {
        let Some(full_index) = ordered_plugins.iter().position(|p| p.name == plugin.name) else {
            continue;
        };
        let row = build_plugin_row(
            plugin,
            full_index,
            ordered_plugins.len(),
            pinned_prefix_len,
            !is_filtered,
            game,
            list_box,
            status_page,
            stack,
            reorder_hint,
            search_query,
        );
        list_box.append(&row);
    }

    stack.set_visible_child_name("list");
}

#[allow(clippy::too_many_arguments)]
fn build_plugin_row(
    plugin: &PluginFile,
    full_index: usize,
    total: usize,
    pinned_prefix_len: usize,
    allow_reorder: bool,
    game: &Rc<Game>,
    list_box: &gtk4::ListBox,
    status_page: &adw::StatusPage,
    stack: &gtk4::Stack,
    reorder_hint: &gtk4::Label,
    search_query: &str,
) -> adw::ActionRow {
    let subtitle = if plugin.is_vanilla {
        format!("{} · Vanilla master (pinned)", plugin.kind.label())
    } else {
        plugin.kind.label().to_string()
    };

    let row = adw::ActionRow::builder()
        .title(&plugin.name)
        .subtitle(&subtitle)
        .build();

    let index_label = gtk4::Label::new(Some(&format!("{}", full_index + 1)));
    index_label.add_css_class("dim-label");
    index_label.add_css_class("numeric");
    index_label.set_width_chars(3);
    row.add_prefix(&index_label);

    let badge = gtk4::Label::new(Some(plugin.kind.label()));
    badge.add_css_class("caption");
    badge.add_css_class("dim-label");
    badge.set_valign(gtk4::Align::Center);
    row.add_suffix(&badge);

    if plugin.is_vanilla {
        let lock = gtk4::Image::from_icon_name("changes-prevent-symbolic");
        lock.set_tooltip_text(Some("Vanilla master – cannot be moved or disabled"));
        lock.add_css_class("dim-label");
        row.add_suffix(&lock);
        return row;
    }

    let enabled_btn = gtk4::CheckButton::new();
    enabled_btn.set_active(plugin.enabled);
    enabled_btn.set_tooltip_text(Some(if plugin.enabled {
        "Disable plugin"
    } else {
        "Enable plugin"
    }));
    enabled_btn.set_valign(gtk4::Align::Center);
    {
        let game_c = Rc::clone(game);
        let list_c = list_box.clone();
        let status_c = status_page.clone();
        let stack_c = stack.clone();
        let hint_c = reorder_hint.clone();
        let plugin_name = plugin.name.clone();
        let search_q = search_query.to_string();
        enabled_btn.connect_toggled(move |btn| {
            let enabled = btn.is_active();
            let mut db = ModDatabase::load(&game_c);
            if enabled {
                db.plugin_disabled.remove(&plugin_name);
            } else {
                db.plugin_disabled.insert(plugin_name.clone());
            }
            db.save(&game_c);
            refresh_load_order_content(&list_c, &status_c, &stack_c, &hint_c, &game_c, &search_q);
        });
    }
    row.add_suffix(&enabled_btn);

    let up_btn = gtk4::Button::new();
    up_btn.set_icon_name("go-up-symbolic");
    up_btn.set_valign(gtk4::Align::Center);
    up_btn.add_css_class("flat");
    up_btn.set_tooltip_text(Some(if allow_reorder {
        "Move up"
    } else {
        "Clear search to reorder"
    }));
    up_btn.set_sensitive(allow_reorder && full_index > pinned_prefix_len);

    let down_btn = gtk4::Button::new();
    down_btn.set_icon_name("go-down-symbolic");
    down_btn.set_valign(gtk4::Align::Center);
    down_btn.add_css_class("flat");
    down_btn.set_tooltip_text(Some(if allow_reorder {
        "Move down"
    } else {
        "Clear search to reorder"
    }));
    down_btn.set_sensitive(allow_reorder && full_index + 1 < total);

    row.add_suffix(&up_btn);
    row.add_suffix(&down_btn);

    {
        let game_c = Rc::clone(game);
        let list_c = list_box.clone();
        let status_c = status_page.clone();
        let stack_c = stack.clone();
        let hint_c = reorder_hint.clone();
        let plugin_name = plugin.name.clone();
        let search_q = search_query.to_string();
        up_btn.connect_clicked(move |_| {
            let mut db = ModDatabase::load(&game_c);
            let ordered = db.get_ordered_plugins(&game_c);
            if let Ok(updated) =
                ordering::move_up_by_id(&ordered, &plugin_name, pinned_prefix_len, |p| &p.name)
            {
                db.set_plugin_order(&updated);
                db.save(&game_c);
            }
            refresh_load_order_content(&list_c, &status_c, &stack_c, &hint_c, &game_c, &search_q);
        });
    }

    {
        let game_c = Rc::clone(game);
        let list_c = list_box.clone();
        let status_c = status_page.clone();
        let stack_c = stack.clone();
        let hint_c = reorder_hint.clone();
        let plugin_name = plugin.name.clone();
        let search_q = search_query.to_string();
        down_btn.connect_clicked(move |_| {
            let mut db = ModDatabase::load(&game_c);
            let ordered = db.get_ordered_plugins(&game_c);
            if let Ok(updated) =
                ordering::move_down_by_id(&ordered, &plugin_name, pinned_prefix_len, |p| &p.name)
            {
                db.set_plugin_order(&updated);
                db.save(&game_c);
            }
            refresh_load_order_content(&list_c, &status_c, &stack_c, &hint_c, &game_c, &search_q);
        });
    }

    let right_click = gtk4::GestureClick::new();
    right_click.set_button(3);
    {
        let row_c = row.clone();
        let game_rclick = Rc::clone(game);
        let list_rclick = list_box.clone();
        let status_rclick = status_page.clone();
        let stack_rclick = stack.clone();
        let hint_rclick = reorder_hint.clone();
        let plugin_name_rclick = plugin.name.clone();
        let search_q = search_query.to_string();
        right_click.connect_pressed(move |gesture, _, x, y| {
            gesture.set_state(gtk4::EventSequenceState::Claimed);

            let popover = gtk4::Popover::new();
            popover.set_parent(&row_c);
            let rect = gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover.set_pointing_to(Some(&rect));
            popover.set_has_arrow(false);

            let menu_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
            menu_box.set_margin_top(4);
            menu_box.set_margin_bottom(4);
            menu_box.set_margin_start(4);
            menu_box.set_margin_end(4);

            let move_item = gtk4::Button::with_label("Move to Position…");
            move_item.add_css_class("flat");
            move_item.set_halign(gtk4::Align::Fill);
            move_item.set_hexpand(true);
            move_item.set_sensitive(allow_reorder);
            move_item.set_tooltip_text(Some(if allow_reorder {
                "Move to a specific load-order position"
            } else {
                "Clear search to reorder"
            }));
            menu_box.append(&move_item);

            let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
            menu_box.append(&sep);

            let enable_all_item = gtk4::Button::with_label("Enable All");
            enable_all_item.add_css_class("flat");
            enable_all_item.set_halign(gtk4::Align::Fill);
            enable_all_item.set_hexpand(true);
            menu_box.append(&enable_all_item);

            let disable_all_item = gtk4::Button::with_label("Disable All");
            disable_all_item.add_css_class("flat");
            disable_all_item.set_halign(gtk4::Align::Fill);
            disable_all_item.set_hexpand(true);
            menu_box.append(&disable_all_item);

            popover.set_child(Some(&menu_box));

            let popover_c = popover.clone();
            let row_btn = row_c.clone();
            let game_btn = Rc::clone(&game_rclick);
            let list_btn = list_rclick.clone();
            let status_btn = status_rclick.clone();
            let stack_btn = stack_rclick.clone();
            let hint_btn = hint_rclick.clone();
            let plugin_name_btn = plugin_name_rclick.clone();
            let search_btn = search_q.clone();
            move_item.connect_clicked(move |_| {
                popover_c.popdown();
                if let Some(root) = row_btn.root()
                    && let Ok(window) = root.downcast::<gtk4::Window>()
                {
                    show_move_to_position_dialog(
                        &window,
                        plugin_name_btn.clone(),
                        pinned_prefix_len,
                        Rc::clone(&game_btn),
                        list_btn.clone(),
                        status_btn.clone(),
                        stack_btn.clone(),
                        hint_btn.clone(),
                        search_btn.clone(),
                    );
                }
            });

            let popover_enable = popover.clone();
            let game_enable = Rc::clone(&game_rclick);
            let list_enable = list_rclick.clone();
            let status_enable = status_rclick.clone();
            let stack_enable = stack_rclick.clone();
            let hint_enable = hint_rclick.clone();
            let search_enable = search_q.clone();
            enable_all_item.connect_clicked(move |_| {
                popover_enable.popdown();
                let mut db = ModDatabase::load(&game_enable);
                db.plugin_disabled.clear();
                db.save(&game_enable);
                refresh_load_order_content(
                    &list_enable,
                    &status_enable,
                    &stack_enable,
                    &hint_enable,
                    &game_enable,
                    &search_enable,
                );
            });

            let popover_disable = popover.clone();
            let game_disable = Rc::clone(&game_rclick);
            let list_disable = list_rclick.clone();
            let status_disable = status_rclick.clone();
            let stack_disable = stack_rclick.clone();
            let hint_disable = hint_rclick.clone();
            let search_disable = search_q.clone();
            disable_all_item.connect_clicked(move |_| {
                popover_disable.popdown();
                let mut db = ModDatabase::load(&game_disable);
                let ordered = db.get_ordered_plugins(&game_disable);
                for p in &ordered {
                    if !p.is_vanilla && !db.plugin_disabled.contains(&p.name) {
                        db.plugin_disabled.insert(p.name.clone());
                    }
                }
                db.save(&game_disable);
                refresh_load_order_content(
                    &list_disable,
                    &status_disable,
                    &stack_disable,
                    &hint_disable,
                    &game_disable,
                    &search_disable,
                );
            });

            popover.popup();
        });
    }
    row.add_controller(right_click);

    row
}

#[allow(clippy::too_many_arguments)]
fn show_move_to_position_dialog(
    parent: &gtk4::Window,
    plugin_name: String,
    pinned_prefix_len: usize,
    game: Rc<Game>,
    list_box: gtk4::ListBox,
    status_page: adw::StatusPage,
    stack: gtk4::Stack,
    reorder_hint: gtk4::Label,
    search_query: String,
) {
    let db = ModDatabase::load(&game);
    let ordered = db.get_ordered_plugins(&game);
    let total = ordered.len();
    let Some(current_idx) = ordered.iter().position(|p| p.name == plugin_name) else {
        return;
    };

    let min_pos = pinned_prefix_len + 1;
    let body = if pinned_prefix_len > 0 {
        format!(
            "Enter the new load order position for \"{plugin_name}\".\nValid range: {min_pos}–{total} (positions 1–{pinned_prefix_len} are vanilla masters).",
        )
    } else {
        format!("Enter the new load order position for \"{plugin_name}\".\nValid range: 1–{total}.")
    };

    let dialog = adw::AlertDialog::builder()
        .heading("Move to Position")
        .body(&body)
        .build();

    let spin = gtk4::SpinButton::with_range(min_pos as f64, total as f64, 1.0);
    spin.set_value((current_idx + 1) as f64);
    spin.set_numeric(true);
    dialog.set_extra_child(Some(&spin));

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("move", "Move");
    dialog.set_response_appearance("move", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("move"));
    dialog.set_close_response("cancel");

    dialog.connect_response(None, move |_, response| {
        if response != "move" {
            return;
        }

        let target_idx = (spin.value() as usize).saturating_sub(1);
        let mut db = ModDatabase::load(&game);
        let ordered = db.get_ordered_plugins(&game);
        if let Ok(updated) = ordering::move_to_absolute_position_by_id(
            &ordered,
            &plugin_name,
            target_idx,
            pinned_prefix_len,
            |p| &p.name,
        ) {
            db.set_plugin_order(&updated);
            db.save(&game);
        }

        refresh_load_order_content(
            &list_box,
            &status_page,
            &stack,
            &reorder_hint,
            &game,
            &search_query,
        );
    });

    dialog.present(Some(parent));
}

fn matches_query(value: &str, query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return true;
    }
    value.to_lowercase().contains(&trimmed.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{is_event_for_game, matches_query};
    use crate::core::workspace::WorkspaceEvent;

    #[test]
    fn matches_query_is_case_insensitive() {
        assert!(matches_query("MyPatch.esp", "patch"));
        assert!(matches_query("MyPatch.esp", "  ESP  "));
        assert!(!matches_query("MyPatch.esp", "armor"));
    }

    #[test]
    fn event_filter_matches_expected_game_id() {
        let ev = WorkspaceEvent::ProfileStateChanged {
            game_id: "game_a".to_string(),
            profile_id: "default".to_string(),
        };
        assert!(is_event_for_game(&ev, "game_a"));
        assert!(!is_event_for_game(&ev, "game_b"));
    }
}
