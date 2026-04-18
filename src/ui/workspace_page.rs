use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use gio;
use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::deployment;
use crate::core::games::Game;
use crate::core::mods::{ModDatabase, ModManager};
use crate::core::runtime_scan::{self, RuntimeEntryClassification};
use crate::core::workspace;

fn attach_workspace_event_listener<F>(mut on_event: F)
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
    glib::idle_add_local(move || {
        while let Ok(event) = rx_ui.try_recv() {
            on_event(event);
        }
        glib::ControlFlow::Continue
    });
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

fn preview_examples<T: ToString>(items: &[T]) -> String {
    if items.is_empty() {
        return "None".to_string();
    }
    const LIMIT: usize = 5;
    let mut listed = items
        .iter()
        .take(LIMIT)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if items.len() > LIMIT {
        listed.push(format!("…and {} more", items.len() - LIMIT));
    }
    listed.join(" · ")
}

fn append_group_row(list: &gtk4::ListBox, title: &str, values: &[String]) {
    let row = adw::ActionRow::builder()
        .title(format!("{title} ({})", values.len()))
        .subtitle(preview_examples(values))
        .build();
    list.append(&row);
}

fn append_paths_row(list: &gtk4::ListBox, title: &str, paths: &[PathBuf]) {
    let values = paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    append_group_row(list, title, &values);
}

pub fn build_workspace_page(game: Option<&Game>, _config: Rc<RefCell<AppConfig>>) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let title_widget = adw::WindowTitle::new("Workspace", "Review and apply staged changes");
    header.set_title_widget(Some(&title_widget));
    toolbar_view.add_top_bar(&header);

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(980);
    clamp.set_margin_top(12);
    clamp.set_margin_bottom(12);
    clamp.set_margin_start(12);
    clamp.set_margin_end(12);

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 24);

    if let Some(game) = game.cloned() {
        let review_group = adw::PreferencesGroup::builder()
            .title("Workspace Status")
            .description("Primary review/apply surface for staged edits, integrity, and recovery.")
            .build();
        let workspace_row = adw::ActionRow::builder()
            .title("Summary")
            .subtitle("Loading workspace state…")
            .build();
        review_group.add(&workspace_row);

        let actions_row = adw::ActionRow::builder()
            .title("Apply / Recover")
            .subtitle("Review preview/integrity before applying redeploy")
            .build();
        let redeploy_btn = gtk4::Button::with_label("Redeploy now");
        let discard_btn = gtk4::Button::with_label("Discard staged");
        let verify_btn = gtk4::Button::with_label("Verify deployed state");
        actions_row.add_suffix(&verify_btn);
        actions_row.add_suffix(&discard_btn);
        actions_row.add_suffix(&redeploy_btn);
        review_group.add(&actions_row);

        let details_group = adw::PreferencesGroup::builder()
            .title("Review Details")
            .description("Deploy preview, runtime/output review, backups, and integrity")
            .build();
        let details_list = gtk4::ListBox::new();
        details_list.add_css_class("boxed-list");
        details_list.set_selection_mode(gtk4::SelectionMode::None);
        details_group.add(&details_list);

        let rebuild: Rc<RefCell<Box<dyn Fn()>>> = Rc::new(RefCell::new(Box::new(|| {})));
        let rebuild_weak = Rc::downgrade(&rebuild);
        {
            let game_c = game.clone();
            let workspace_row_c = workspace_row.clone();
            let details_list_c = details_list.clone();
            let redeploy_btn_c = redeploy_btn.clone();
            let discard_btn_c = discard_btn.clone();
            let verify_btn_c = verify_btn.clone();
            *rebuild.borrow_mut() = Box::new(move || {
                while let Some(child) = details_list_c.first_child() {
                    details_list_c.remove(&child);
                }
                let db = ModDatabase::load(&game_c);
                let state = workspace::workspace_state_for_game(&game_c);
                workspace_row_c.set_subtitle(&workspace::format_workspace_banner_summary(&state));

                let preview = deployment::deployment_preview(&game_c, &db).ok();
                redeploy_btn_c.set_sensitive(
                    state.safe_redeploy_required
                        && preview.as_ref().is_none_or(|p| p.blocked_paths.is_empty()),
                );
                discard_btn_c.set_sensitive(
                    state.safe_redeploy_required
                        && workspace::has_profile_baseline(&game_c, &state.profile_id),
                );
                verify_btn_c.set_label(
                    if workspace::latest_integrity_report(&game_c.id, &state.profile_id).is_some() {
                        "Recheck integrity"
                    } else {
                        "Verify deployed state"
                    },
                );

                let guidance_row = adw::ActionRow::builder()
                    .title("Redeploy Guidance")
                    .subtitle(workspace::format_redeploy_guidance(
                        &state,
                        preview.as_ref(),
                    ))
                    .build();
                details_list_c.append(&guidance_row);

                if let Some(preview) = &preview {
                    details_list_c.append(
                        &adw::ActionRow::builder()
                            .title("Deploy Preview")
                            .subtitle(preview.summary_line())
                            .build(),
                    );
                    append_paths_row(&details_list_c, "Create links", &preview.links_to_create);
                    append_paths_row(&details_list_c, "Replace links", &preview.links_to_replace);
                    append_paths_row(&details_list_c, "Remove links", &preview.links_to_remove);
                    append_paths_row(
                        &details_list_c,
                        "Backup real files",
                        &preview.real_files_to_backup,
                    );
                    append_paths_row(
                        &details_list_c,
                        "Restore preserved files",
                        &preview.backups_to_restore,
                    );
                    append_group_row(&details_list_c, "Blocked paths", &preview.blocked_paths);
                }

                if let Ok(scan_report) = runtime_scan::scan_profile_runtime_changes(&game_c, &db) {
                    workspace::update_runtime_scan_status(
                        &game_c.id,
                        &db.active_profile_id,
                        &scan_report,
                    );
                    let unresolved = scan_report.unresolved_review_count();
                    details_list_c.append(
                        &adw::ActionRow::builder()
                            .title("Runtime / unmanaged changes")
                            .subtitle(if unresolved > 0 {
                                format!("{unresolved} unresolved item(s)")
                            } else {
                                "No unresolved runtime/unmanaged changes".to_string()
                            })
                            .build(),
                    );
                    let runtime_examples = scan_report
                        .entries
                        .iter()
                        .filter(|e| {
                            !matches!(
                                e.classification,
                                RuntimeEntryClassification::ManagedOwnedPresent
                            )
                        })
                        .take(5)
                        .map(|e| format!("{} ({:?})", e.relative_path.display(), e.classification))
                        .collect::<Vec<_>>();
                    if !runtime_examples.is_empty() {
                        append_group_row(&details_list_c, "Runtime examples", &runtime_examples);
                    }
                }

                let active_profile = db.active_profile_id.clone();
                let output_rows = db
                    .generated_outputs
                    .iter()
                    .filter(|o| o.manager_profile_id == active_profile)
                    .map(|o| {
                        format!(
                            "{} · files: {} · {}{}",
                            o.name,
                            o.owned_files.len(),
                            if o.enabled { "enabled" } else { "disabled" },
                            if o.pending_removal {
                                " · pending removal"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect::<Vec<_>>();
                append_group_row(&details_list_c, "Generated outputs", &output_rows);

                if let Ok(backup_status) =
                    deployment::deployment_backup_status(&game_c, &db.active_profile_id)
                {
                    let backup_row = adw::ActionRow::builder()
                        .title("Deployment backups")
                        .subtitle(format!(
                            "Preserved originals: {} entry/entries ({} payload file(s))",
                            backup_status.backup_entries, backup_status.existing_payload_files
                        ))
                        .build();
                    let reveal_btn = gtk4::Button::with_label("Reveal");
                    let backup_root = backup_status.backup_root.clone();
                    reveal_btn.connect_clicked(move |_| {
                        let file = gio::File::for_path(&backup_root);
                        let _ = gio::AppInfo::launch_default_for_uri(
                            &file.uri(),
                            None::<&gio::AppLaunchContext>,
                        );
                    });
                    backup_row.add_suffix(&reveal_btn);
                    details_list_c.append(&backup_row);
                }

                let integrity_row = adw::ActionRow::builder()
                    .title("Deployment integrity")
                    .subtitle(&state.integrity_summary)
                    .build();
                details_list_c.append(&integrity_row);
                if state.integrity_issue_total > 0 {
                    details_list_c.append(
                        &adw::ActionRow::builder()
                            .title("Integrity guidance")
                            .subtitle(workspace::format_integrity_guidance(&state))
                            .build(),
                    );
                    append_group_row(
                        &details_list_c,
                        "Integrity examples",
                        &state.integrity_examples,
                    );
                }
                if let Some(report) =
                    workspace::latest_integrity_report(&game_c.id, &state.profile_id)
                    && !report.issues.is_empty()
                {
                    let mut by_kind = BTreeMap::<String, usize>::new();
                    for issue in &report.issues {
                        *by_kind
                            .entry(deployment::integrity_issue_kind_label(&issue.kind).to_string())
                            .or_insert(0) += 1;
                    }
                    let grouped = by_kind
                        .into_iter()
                        .map(|(label, count)| format!("{label}: {count}"))
                        .collect::<Vec<_>>();
                    append_group_row(&details_list_c, "Integrity issue categories", &grouped);
                }
            });
        }

        {
            let game_c = game.clone();
            let rebuild_c = rebuild_weak.clone();
            redeploy_btn.connect_clicked(move |_| {
                if let Err(e) = ModManager::rebuild_all(&game_c) {
                    log::error!("Workspace redeploy failed: {e}");
                }
                if let Some(rb) = rebuild_c.upgrade() {
                    (rb.borrow())();
                }
            });
        }
        {
            let game_c = game.clone();
            let rebuild_c = rebuild_weak.clone();
            discard_btn.connect_clicked(move |btn| {
                let parent = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
                let dialog = adw::AlertDialog::builder()
                    .heading("Discard staged changes?")
                    .body("This restores profile state to the deployed baseline. Deployed files are unchanged until explicit redeploy.")
                    .build();
                dialog.add_response("cancel", "Cancel");
                dialog.add_response("discard", "Discard");
                dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
                dialog.set_default_response(Some("cancel"));
                dialog.set_close_response("cancel");
                let game_r = game_c.clone();
                let rebuild_r = rebuild_c.clone();
                dialog.connect_response(None, move |_, response| {
                    if response != "discard" {
                        return;
                    }
                    if let Err(e) = workspace::revert_active_profile_to_baseline(&game_r) {
                        log::error!("Failed to discard staged changes: {e}");
                    }
                    if let Some(rb) = rebuild_r.upgrade() {
                        (rb.borrow())();
                    }
                });
                dialog.present(parent.as_ref());
            });
        }
        {
            let game_c = game.clone();
            let rebuild_c = rebuild_weak.clone();
            verify_btn.connect_clicked(move |_| {
                if let Err(e) = workspace::verify_and_store_integrity(&game_c) {
                    log::error!("Workspace integrity verification failed: {e}");
                }
                if let Some(rb) = rebuild_c.upgrade() {
                    (rb.borrow())();
                }
            });
        }

        {
            let game_c = game.clone();
            let rebuild_c = rebuild_weak.clone();
            attach_workspace_event_listener(move |event| {
                if is_event_for_game(&event, &game_c.id)
                    && let Some(rb) = rebuild_c.upgrade()
                {
                    (rb.borrow())();
                }
            });
        }

        (rebuild.borrow())();
        content_box.append(&review_group);
        content_box.append(&details_group);
    } else {
        let status_page = adw::StatusPage::builder()
            .title("No Game Selected")
            .description("Select a game to review workspace and deployment state.")
            .icon_name("applications-games-symbolic")
            .vexpand(true)
            .build();
        content_box.append(&status_page);
    }

    clamp.set_child(Some(&content_box));
    scrolled.set_child(Some(&clamp));
    toolbar_view.set_content(Some(&scrolled));
    toolbar_view.upcast::<gtk4::Widget>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_event_filter_matches_game_id() {
        let hit = workspace::WorkspaceEvent::WorkspaceStateChanged {
            game_id: "g".to_string(),
            profile_id: "p".to_string(),
        };
        let miss = workspace::WorkspaceEvent::DeployFailed {
            game_id: "other".to_string(),
            profile_id: "p".to_string(),
        };
        assert!(is_event_for_game(&hit, "g"));
        assert!(!is_event_for_game(&miss, "g"));
    }
}
