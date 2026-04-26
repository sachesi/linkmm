use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gio;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::Game;
use crate::core::mods::{Mod, ModDatabase, ModManager};
use crate::ui::ordering;

use super::conflicts::ConflictState;

/// Shared page-level state captured by every closure in `build_mod_row`.
#[derive(Clone)]
pub(super) struct RowCtx {
    pub(super) game: Rc<Game>,
    pub(super) config: Rc<RefCell<AppConfig>>,
    pub(super) search: Rc<RefCell<String>>,
    pub(super) selected: Rc<RefCell<Option<String>>>,
    pub(super) container: gtk4::Box,
    pub(super) hint: gtk4::Label,
}

impl RowCtx {
    pub(super) fn refresh(&self) {
        super::refresh_library_content_with_search(
            &self.container,
            &self.game,
            Rc::clone(&self.config),
            &self.search.borrow(),
            Rc::clone(&self.search),
            Rc::clone(&self.selected),
            &self.hint,
            false,
        );
    }
}

pub(super) enum ReorderDir {
    Up,
    Down,
}

pub(super) fn make_reorder_handler(
    ctx: RowCtx,
    mod_id: String,
    dir: ReorderDir,
) -> impl Fn(&gtk4::Button) {
    move |_| {
        let mut db = ModDatabase::load(&ctx.game);
        let result = match dir {
            ReorderDir::Up => ordering::move_up_by_id(&db.mods, &mod_id, 0, |m| &m.id),
            ReorderDir::Down => ordering::move_down_by_id(&db.mods, &mod_id, 0, |m| &m.id),
        };
        if let Ok(updated) = result {
            db.mods = updated;
            db.save(&ctx.game);
            ctx.refresh();
        }
    }
}

pub(super) fn make_bulk_toggle_handler(ctx: RowCtx, enable: bool) -> impl Fn(&gtk4::Button) {
    move |_| {
        let mut db = ModDatabase::load(&ctx.game);
        for m in db.mods.iter_mut() {
            m.enabled = enable;
        }
        db.save(&ctx.game);
        ctx.refresh();
    }
}

// ── Per-row data needed for in-place DnD reordering ────────────────────────

pub(super) struct DndRowData {
    pub(super) mod_id: String,
    pub(super) row: adw::SwitchRow,
    pub(super) index_label: gtk4::Label,
    pub(super) up_btn: gtk4::Button,
    pub(super) down_btn: gtk4::Button,
}

/// What `build_mod_row` returns so the caller can wire up DnD.
pub(super) struct ModRowResult {
    pub(super) row: adw::SwitchRow,
    pub(super) index_label: gtk4::Label,
    pub(super) up_btn: gtk4::Button,
    pub(super) down_btn: gtk4::Button,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_mod_row(
    mod_entry: &Mod,
    full_idx: usize,
    total: usize,
    allow_reorder: bool,
    game: &Rc<Game>,
    container: &gtk4::Box,
    config: Rc<RefCell<AppConfig>>,
    search_state: Rc<RefCell<String>>,
    selected_mod_id: Rc<RefCell<Option<String>>>,
    reorder_hint: &gtk4::Label,
    conflict_state: Option<&ConflictState>,
) -> ModRowResult {
    let ctx = RowCtx {
        game: Rc::clone(game),
        config,
        search: search_state,
        selected: selected_mod_id,
        container: container.clone(),
        hint: reorder_hint.clone(),
    };

    let row = adw::SwitchRow::builder()
        .title(&mod_entry.name)
        .active(mod_entry.enabled)
        .build();

    let subtitle = match (&mod_entry.version, mod_entry.installed_from_nexus) {
        (Some(v), true) => format!("{v} · From Nexus Mods"),
        (Some(v), false) => v.clone(),
        (None, true) => "From Nexus Mods".to_string(),
        (None, false) => String::new(),
    };
    if !subtitle.is_empty() {
        row.set_subtitle(&subtitle);
    }

    // ── Drag handle ───────────────────────────────────────────────────────────
    let handle = gtk4::Button::new();
    handle.set_icon_name("list-drag-handle-symbolic");
    handle.add_css_class("flat");
    handle.add_css_class("drag-handle");
    handle.set_valign(gtk4::Align::Center);
    handle.set_sensitive(allow_reorder);
    handle.set_tooltip_text(Some(if allow_reorder {
        "Drag to reorder"
    } else {
        "Clear search to reorder"
    }));
    row.add_prefix(&handle);

    // DragSource on the handle
    if allow_reorder {
        let drag_source = gtk4::DragSource::new();
        drag_source.set_actions(gtk4::gdk::DragAction::MOVE);
        let content = gtk4::gdk::ContentProvider::for_value(&mod_entry.id.to_value());
        drag_source.set_content(Some(&content));

        let row_begin = row.clone();
        drag_source.connect_drag_begin(move |src, _| {
            let paintable = gtk4::WidgetPaintable::new(Some(&row_begin));
            src.set_icon(Some(&paintable), 16, 28);
            row_begin.add_css_class("dnd-source");
        });

        let row_end = row.clone();
        drag_source.connect_drag_end(move |_, _, _| {
            row_end.remove_css_class("dnd-source");
        });

        handle.add_controller(drag_source);
    }

    // ── Index label ───────────────────────────────────────────────────────────
    let index_label = gtk4::Label::new(Some(&format!("{}", full_idx + 1)));
    index_label.add_css_class("dim-label");
    index_label.add_css_class("numeric");
    index_label.set_width_chars(3);
    row.add_prefix(&index_label);

    if let Some(state) = conflict_state {
        if state.overwritten {
            row.add_css_class("error");
        } else if state.overwrites {
            row.add_css_class("success");
        } else if !state.files.is_empty() {
            row.add_css_class("accent");
        }
    }

    let mod_id = mod_entry.id.clone();
    let game_toggle = Rc::clone(game);
    row.connect_active_notify(move |switch_row| {
        let enabled = switch_row.is_active();
        if let Err(e) = ModManager::set_mod_enabled(&game_toggle, &mod_id, enabled) {
            log::error!("Failed to rebuild deployment after toggle: {e}");
            switch_row.set_active(!enabled);
        }
    });

    // ── Move up / down ────────────────────────────────────────────────────────
    let up_btn = gtk4::Button::new();
    up_btn.set_icon_name("go-up-symbolic");
    up_btn.set_valign(gtk4::Align::Center);
    up_btn.add_css_class("flat");
    up_btn.set_tooltip_text(Some(if allow_reorder {
        "Move up"
    } else {
        "Clear search to reorder"
    }));
    up_btn.set_sensitive(allow_reorder && full_idx > 0);

    let down_btn = gtk4::Button::new();
    down_btn.set_icon_name("go-down-symbolic");
    down_btn.set_valign(gtk4::Align::Center);
    down_btn.add_css_class("flat");
    down_btn.set_tooltip_text(Some(if allow_reorder {
        "Move down"
    } else {
        "Clear search to reorder"
    }));
    down_btn.set_sensitive(allow_reorder && full_idx + 1 < total);

    row.add_suffix(&up_btn);
    row.add_suffix(&down_btn);

    up_btn.connect_clicked(make_reorder_handler(
        ctx.clone(),
        mod_entry.id.clone(),
        ReorderDir::Up,
    ));
    down_btn.connect_clicked(make_reorder_handler(
        ctx.clone(),
        mod_entry.id.clone(),
        ReorderDir::Down,
    ));

    // ── Uninstall button ──────────────────────────────────────────────────────
    let delete_btn = gtk4::Button::new();
    delete_btn.set_icon_name("user-trash-symbolic");
    delete_btn.set_tooltip_text(Some("Uninstall mod"));
    delete_btn.add_css_class("flat");
    delete_btn.set_valign(gtk4::Align::Center);
    row.add_suffix(&delete_btn);

    let mod_id_del = mod_entry.id.clone();
    let mod_name_del = mod_entry.name.clone();
    let ctx_del = ctx.clone();
    delete_btn.connect_clicked(move |btn| {
        let parent = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());

        let dialog = adw::AlertDialog::builder()
            .heading("Remove Mod?")
            .body(format!(
                "\u{201c}{}\u{201d} will be permanently removed from disk.",
                mod_name_del
            ))
            .build();

        dialog.add_response("cancel", "Cancel");
        dialog.add_response("remove", "Remove");
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let mod_id_c = mod_id_del.clone();
        let ctx_c = ctx_del.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "remove" {
                return;
            }
            let db = ModDatabase::load(&ctx_c.game);
            if let Some(m) = db.mods.iter().find(|m| m.id == mod_id_c) {
                if let Err(e) = ModManager::uninstall_mod(&ctx_c.game, m) {
                    log::error!("Failed to uninstall mod: {e}");
                } else {
                    let mut cfg = ctx_c.config.borrow_mut();
                    let gs = cfg.game_settings_mut(&ctx_c.game.id);
                    if let Some(archive_name) = &m.archive_name {
                        gs.installed_archives
                            .retain(|archive| archive != archive_name);
                    } else {
                        let mod_name_lower = m.name.to_lowercase();
                        gs.installed_archives.retain(|archive| {
                            let archive_stem = Path::new(archive)
                                .file_stem()
                                .map(|s| s.to_string_lossy().to_lowercase())
                                .unwrap_or_default();
                            archive_stem != mod_name_lower
                        });
                    }
                    cfg.save();
                }
            }
            if ctx_c
                .selected
                .borrow()
                .as_ref()
                .map(|id| id == &mod_id_c)
                .unwrap_or(false)
            {
                *ctx_c.selected.borrow_mut() = None;
            }
            ctx_c.refresh();
        });

        dialog.present(parent.as_ref());
    });

    // ── Left click: select mod for conflict highlighting ───────────────────────
    let left_click = gtk4::GestureClick::new();
    left_click.set_button(1);
    {
        let mod_id_sel = mod_entry.id.clone();
        let ctx_sel = ctx.clone();
        left_click.connect_released(move |_, _, _, _| {
            {
                let mut selected = ctx_sel.selected.borrow_mut();
                if selected.as_ref() == Some(&mod_id_sel) {
                    *selected = None;
                } else {
                    *selected = Some(mod_id_sel.clone());
                }
            }
            let ctx_idle = ctx_sel.clone();
            gtk4::glib::idle_add_local_once(move || ctx_idle.refresh());
        });
    }
    row.add_controller(left_click);

    // ── Right-click context menu ──────────────────────────────────────────────
    let right_click = gtk4::GestureClick::new();
    right_click.set_button(3);
    {
        let row_c = row.clone();
        let source_path = mod_entry.source_path.clone();
        let nexus_id = mod_entry.nexus_id;
        let mod_id_rclick = mod_entry.id.clone();
        let ctx_rclick = ctx.clone();
        let conflict_entries = conflict_state
            .map(|state| {
                state
                    .conflict_mods_by_file
                    .iter()
                    .map(|(file, mods)| (file.clone(), mods.iter().cloned().collect::<Vec<_>>()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

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

            let open_dir_item = gtk4::Button::with_label("Open Mod Directory");
            open_dir_item.add_css_class("flat");
            open_dir_item.set_halign(gtk4::Align::Fill);
            open_dir_item.set_hexpand(true);
            menu_box.append(&open_dir_item);

            let open_nexus_item = gtk4::Button::with_label("Visit on Nexus Mods");
            open_nexus_item.add_css_class("flat");
            open_nexus_item.set_halign(gtk4::Align::Fill);
            open_nexus_item.set_hexpand(true);
            open_nexus_item.set_sensitive(nexus_id.is_some());
            menu_box.append(&open_nexus_item);

            let show_conflicts_item = gtk4::Button::with_label("Show Conflicting Files");
            show_conflicts_item.add_css_class("flat");
            show_conflicts_item.set_halign(gtk4::Align::Fill);
            show_conflicts_item.set_hexpand(true);
            show_conflicts_item.set_sensitive(!conflict_entries.is_empty());
            menu_box.append(&show_conflicts_item);

            let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
            menu_box.append(&sep);

            let move_item = gtk4::Button::with_label("Move to Position\u{2026}");
            move_item.add_css_class("flat");
            move_item.set_halign(gtk4::Align::Fill);
            move_item.set_hexpand(true);
            move_item.set_sensitive(allow_reorder);
            move_item.set_tooltip_text(Some(if allow_reorder {
                "Move to a specific precedence position"
            } else {
                "Clear search to reorder"
            }));
            menu_box.append(&move_item);

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

            let popover_dir = popover.clone();
            let source_dir = source_path.clone();
            open_dir_item.connect_clicked(move |_| {
                popover_dir.popdown();
                open_in_file_manager(&source_dir);
            });

            let popover_nexus = popover.clone();
            let game_nexus = Rc::clone(&ctx_rclick.game);
            open_nexus_item.connect_clicked(move |_| {
                popover_nexus.popdown();
                if let Some(id) = nexus_id {
                    let uri = format!(
                        "https://www.nexusmods.com/{}/mods/{}",
                        game_nexus.kind.nexus_game_id(),
                        id
                    );
                    let _ =
                        gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
                }
            });

            let popover_conflicts = popover.clone();
            let row_for_dialog = row_c.clone();
            let conflict_entries_for_menu = conflict_entries.clone();
            show_conflicts_item.connect_clicked(move |_| {
                popover_conflicts.popdown();
                if conflict_entries_for_menu.is_empty() {
                    return;
                }
                let body = conflict_entries_for_menu
                    .iter()
                    .map(|(file, mods)| {
                        if mods.is_empty() {
                            format!("• {file}")
                        } else {
                            format!("• {file}\n  conflicts with: {}", mods.join(", "))
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let dialog = adw::AlertDialog::builder()
                    .heading("Conflicting Files")
                    .body(&body)
                    .build();
                dialog.add_response("ok", "OK");
                dialog.set_default_response(Some("ok"));
                dialog.set_close_response("ok");
                let parent = row_for_dialog
                    .root()
                    .and_then(|r| r.downcast::<gtk4::Window>().ok());
                dialog.present(parent.as_ref());
            });

            let popover_move = popover.clone();
            let row_move = row_c.clone();
            let ctx_move = ctx_rclick.clone();
            let mod_id_move = mod_id_rclick.clone();
            move_item.connect_clicked(move |_| {
                popover_move.popdown();
                if let Some(root) = row_move.root()
                    && let Ok(window) = root.downcast::<gtk4::Window>()
                {
                    show_move_to_position_dialog_for_mod(
                        &window,
                        mod_id_move.clone(),
                        ctx_move.clone(),
                    );
                }
            });

            let popover_enable = popover.clone();
            let ctx_enable = ctx_rclick.clone();
            enable_all_item.connect_clicked(move |_| {
                popover_enable.popdown();
                make_bulk_toggle_handler(ctx_enable.clone(), true)(&gtk4::Button::new());
            });

            let popover_disable = popover.clone();
            let ctx_disable = ctx_rclick.clone();
            disable_all_item.connect_clicked(move |_| {
                popover_disable.popdown();
                make_bulk_toggle_handler(ctx_disable.clone(), false)(&gtk4::Button::new());
            });

            popover.popup();
        });
    }
    row.add_controller(right_click);

    ModRowResult {
        row,
        index_label,
        up_btn,
        down_btn,
    }
}

pub(super) fn show_move_to_position_dialog_for_mod(
    parent: &gtk4::Window,
    mod_id: String,
    ctx: RowCtx,
) {
    let db = ModDatabase::load(&ctx.game);
    let total = db.mods.len();
    if total == 0 {
        return;
    }
    let Some(current_pos) = db.mods.iter().position(|m| m.id == mod_id) else {
        return;
    };
    let mod_name = db.mods[current_pos].name.clone();
    let body =
        format!("Enter the new position for \"{mod_name}\".\nValid range: 1\u{2013}{total}.");

    ordering::show_position_dialog(
        parent,
        "Move to Position",
        &body,
        1,
        total,
        current_pos + 1,
        move |target_idx| {
            let mut db = ModDatabase::load(&ctx.game);
            if let Ok(updated) =
                ordering::move_to_absolute_position_by_id(&db.mods, &mod_id, target_idx, 0, |m| {
                    &m.id
                })
            {
                db.mods = updated;
                db.save(&ctx.game);
                ctx.refresh();
            }
        },
    );
}


fn open_in_file_manager(path: &Path) {
    let file = gio::File::for_path(path);
    let uri = file.uri();
    let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
}
