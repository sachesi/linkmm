use glib;

use crate::core::workspace;

pub fn attach_workspace_event_listener<F>(mut on_event: F)
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

pub fn is_event_for_game(event: &workspace::WorkspaceEvent, game_id: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_event_filter_matches_expected_game() {
        let event = workspace::WorkspaceEvent::DeployFinished {
            game_id: "game_a".to_string(),
            profile_id: "default".to_string(),
        };
        assert!(is_event_for_game(&event, "game_a"));
        assert!(!is_event_for_game(&event, "game_b"));
    }
}
