#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReorderError {
    MissingItem,
    MissingTarget,
    IllegalMoveAcrossPinnedPrefix,
    OutOfBoundsTarget,
}

pub fn insertion_index_after_removal(source_index: usize, target_index: usize) -> usize {
    if source_index < target_index {
        target_index.saturating_sub(1)
    } else {
        target_index
    }
}

pub fn clamp_target_index(target_index: usize, len: usize, pinned_prefix_len: usize) -> usize {
    target_index.max(pinned_prefix_len).min(len)
}

pub fn is_legal_move(
    source_index: usize,
    target_index: usize,
    len: usize,
    pinned_prefix_len: usize,
) -> bool {
    source_index < len
        && target_index <= len
        && source_index >= pinned_prefix_len
        && target_index >= pinned_prefix_len
}

pub fn move_up_by_id<T: Clone, F: Fn(&T) -> &str>(
    ordered: &[T],
    id: &str,
    pinned_prefix_len: usize,
    id_of: F,
) -> Result<Vec<T>, ReorderError> {
    let Some(source) = ordered.iter().position(|item| id_of(item) == id) else {
        return Err(ReorderError::MissingItem);
    };
    if source == 0 || source <= pinned_prefix_len.saturating_sub(1) {
        return Err(ReorderError::IllegalMoveAcrossPinnedPrefix);
    }
    move_to_position_by_index(ordered, source, source - 1, pinned_prefix_len)
}

pub fn move_down_by_id<T: Clone, F: Fn(&T) -> &str>(
    ordered: &[T],
    id: &str,
    pinned_prefix_len: usize,
    id_of: F,
) -> Result<Vec<T>, ReorderError> {
    let Some(source) = ordered.iter().position(|item| id_of(item) == id) else {
        return Err(ReorderError::MissingItem);
    };
    if source + 1 >= ordered.len() {
        return Err(ReorderError::OutOfBoundsTarget);
    }
    move_to_position_by_index(ordered, source, source + 2, pinned_prefix_len)
}

pub fn move_before_by_id<T: Clone, F: Fn(&T) -> &str>(
    ordered: &[T],
    source_id: &str,
    target_id: &str,
    pinned_prefix_len: usize,
    id_of: F,
) -> Result<Vec<T>, ReorderError> {
    let Some(source) = ordered.iter().position(|item| id_of(item) == source_id) else {
        return Err(ReorderError::MissingItem);
    };
    let Some(target) = ordered.iter().position(|item| id_of(item) == target_id) else {
        return Err(ReorderError::MissingTarget);
    };
    move_to_position_by_index(ordered, source, target, pinned_prefix_len)
}

pub fn move_to_absolute_position_by_id<T: Clone, F: Fn(&T) -> &str>(
    ordered: &[T],
    id: &str,
    target_index: usize,
    pinned_prefix_len: usize,
    id_of: F,
) -> Result<Vec<T>, ReorderError> {
    let Some(source) = ordered.iter().position(|item| id_of(item) == id) else {
        return Err(ReorderError::MissingItem);
    };
    move_to_position_by_index(ordered, source, target_index, pinned_prefix_len)
}

pub fn move_to_position_by_index<T: Clone>(
    ordered: &[T],
    source_index: usize,
    target_index: usize,
    pinned_prefix_len: usize,
) -> Result<Vec<T>, ReorderError> {
    if !is_legal_move(source_index, target_index, ordered.len(), pinned_prefix_len) {
        return Err(ReorderError::IllegalMoveAcrossPinnedPrefix);
    }
    let mut reordered = ordered.to_vec();
    let item = reordered.remove(source_index);
    let insert_pos = clamp_target_index(
        insertion_index_after_removal(source_index, target_index),
        reordered.len(),
        pinned_prefix_len,
    );
    reordered.insert(insert_pos, item);
    Ok(reordered)
}

/// Show a "Move to Position" `adw::AlertDialog` with a spin-button.
///
/// * `min_pos` / `max_pos` are 1-indexed human-readable bounds.
/// * `current_pos` is the 1-indexed current position shown in the spinner.
/// * `on_confirm` receives the 0-indexed target position chosen by the user.
pub fn show_position_dialog<F>(
    parent: &gtk4::Window,
    heading: &str,
    body: &str,
    min_pos: usize,
    max_pos: usize,
    current_pos: usize,
    on_confirm: F,
) where
    F: Fn(usize) + 'static,
{
    use libadwaita as adw;
    use libadwaita::prelude::*;

    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .build();

    let spin = gtk4::SpinButton::with_range(min_pos as f64, max_pos as f64, 1.0);
    spin.set_value(current_pos as f64);
    spin.set_numeric(true);
    dialog.set_extra_child(Some(&spin));

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("move", "Move");
    dialog.set_response_appearance("move", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("move"));
    dialog.set_close_response("cancel");

    dialog.connect_response(None, move |_, response| {
        if response == "move" {
            on_confirm((spin.value() as usize).saturating_sub(1));
        }
    });

    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::{
        ReorderError, insertion_index_after_removal, move_before_by_id, move_down_by_id,
        move_to_absolute_position_by_id, move_up_by_id,
    };

    #[test]
    fn move_up_down_in_plain_ordered_list() {
        let ordered = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let up = move_up_by_id(&ordered, "c", 0, |s| s).unwrap();
        assert_eq!(up, vec!["a", "c", "b"]);

        let down = move_down_by_id(&up, "c", 0, |s| s).unwrap();
        assert_eq!(down, vec!["a", "b", "c"]);
    }

    #[test]
    fn move_before_target_accounts_for_source_removal() {
        let ordered = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let moved = move_before_by_id(&ordered, "a", "c", 0, |s| s).unwrap();
        assert_eq!(moved, vec!["b", "a", "c"]);
        assert_eq!(insertion_index_after_removal(0, 2), 1);
    }

    #[test]
    fn move_to_absolute_position_works() {
        let ordered = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let moved = move_to_absolute_position_by_id(&ordered, "c", 0, 0, |s| s).unwrap();
        assert_eq!(moved, vec!["c", "a", "b"]);
    }

    #[test]
    fn pinned_prefix_blocks_crossing_boundary() {
        let ordered = vec![
            "Skyrim.esm".to_string(),
            "Update.esm".to_string(),
            "A.esp".to_string(),
        ];
        let err = move_to_absolute_position_by_id(&ordered, "A.esp", 0, 2, |s| s).unwrap_err();
        assert_eq!(err, ReorderError::IllegalMoveAcrossPinnedPrefix);
    }

    #[test]
    fn library_reorder_helper_uses_mod_ids() {
        #[derive(Clone)]
        struct ModLike {
            id: String,
        }
        let ordered = vec![
            ModLike {
                id: "m1".to_string(),
            },
            ModLike {
                id: "m2".to_string(),
            },
        ];
        let moved = move_down_by_id(&ordered, "m1", 0, |m| &m.id).unwrap();
        assert_eq!(moved[1].id, "m1");
    }

    #[test]
    fn load_order_reorder_helper_uses_plugin_names() {
        let ordered = vec![
            "Skyrim.esm".to_string(),
            "A.esp".to_string(),
            "B.esp".to_string(),
        ];
        let moved = move_up_by_id(&ordered, "B.esp", 1, |s| s).unwrap();
        assert_eq!(moved, vec!["Skyrim.esm", "B.esp", "A.esp"]);
    }
}
