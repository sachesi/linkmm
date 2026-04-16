use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;

pub const DEFAULT_EDGE_THRESHOLD_PX: f64 = 56.0;
pub const DEFAULT_MAX_STEP_PX: f64 = 22.0;
pub const DEFAULT_TICK_MS: u64 = 16;

#[derive(Debug, Clone, Copy)]
pub struct EdgeAutoScrollConfig {
    pub edge_threshold_px: f64,
    pub max_step_px: f64,
}

impl Default for EdgeAutoScrollConfig {
    fn default() -> Self {
        Self {
            edge_threshold_px: DEFAULT_EDGE_THRESHOLD_PX,
            max_step_px: DEFAULT_MAX_STEP_PX,
        }
    }
}

#[derive(Debug, Default)]
pub struct EdgeAutoScrollState {
    pub step_px_per_tick: f64,
    pub ticking: bool,
}

pub fn compute_edge_scroll_step(
    pointer_y: f64,
    viewport_height: f64,
    cfg: EdgeAutoScrollConfig,
) -> f64 {
    if viewport_height <= 0.0 {
        return 0.0;
    }
    if pointer_y < cfg.edge_threshold_px {
        let strength = (cfg.edge_threshold_px - pointer_y) / cfg.edge_threshold_px;
        return -cfg.max_step_px * strength.clamp(0.0, 1.0);
    }
    let bottom_start = (viewport_height - cfg.edge_threshold_px).max(0.0);
    if pointer_y > bottom_start {
        let strength = (pointer_y - bottom_start) / cfg.edge_threshold_px;
        return cfg.max_step_px * strength.clamp(0.0, 1.0);
    }
    0.0
}

pub fn apply_scroll_step(current_value: f64, upper: f64, page_size: f64, step: f64) -> f64 {
    let max_value = (upper - page_size).max(0.0);
    (current_value + step).clamp(0.0, max_value)
}

pub fn apply_row_offset_correction(
    current_value: f64,
    upper: f64,
    page_size: f64,
    current_row_y: f64,
    desired_row_y: f64,
) -> f64 {
    let delta = current_row_y - desired_row_y;
    apply_scroll_step(current_value, upper, page_size, delta)
}

pub fn target_index_from_pointer_y(
    pointer_y: f64,
    row_tops: &[f64],
    row_heights: &[f64],
) -> Option<usize> {
    if row_tops.len() != row_heights.len() || row_tops.is_empty() {
        return None;
    }
    for (idx, (top, height)) in row_tops.iter().zip(row_heights.iter()).enumerate() {
        if pointer_y <= *top + (*height * 0.5) {
            return Some(idx);
        }
    }
    Some(row_tops.len().saturating_sub(1))
}

pub fn stop_drag_autoscroll(state: &Rc<RefCell<EdgeAutoScrollState>>) {
    state.borrow_mut().step_px_per_tick = 0.0;
}

pub fn ensure_drag_autoscroll_tick(
    scrolled: &gtk4::ScrolledWindow,
    state: Rc<RefCell<EdgeAutoScrollState>>,
    tick_ms: u64,
) {
    if state.borrow().ticking {
        return;
    }
    state.borrow_mut().ticking = true;
    let scrolled_c = scrolled.clone();
    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(tick_ms), move || {
        let step = state.borrow().step_px_per_tick;
        if step.abs() < f64::EPSILON {
            state.borrow_mut().ticking = false;
            return gtk4::glib::ControlFlow::Break;
        }
        let adj = scrolled_c.vadjustment();
        let next = apply_scroll_step(adj.value(), adj.upper(), adj.page_size(), step);
        adj.set_value(next);
        gtk4::glib::ControlFlow::Continue
    });
}

pub fn attach_viewport_drag_autoscroll(
    scrolled: &gtk4::ScrolledWindow,
    state: Rc<RefCell<EdgeAutoScrollState>>,
    cfg: EdgeAutoScrollConfig,
    tick_ms: u64,
) {
    let controller = gtk4::DropControllerMotion::new();
    {
        let scrolled_c = scrolled.clone();
        let state_c = Rc::clone(&state);
        controller.connect_motion(move |_, _, y| {
            let step = compute_edge_scroll_step(y, scrolled_c.height() as f64, cfg);
            state_c.borrow_mut().step_px_per_tick = step;
            ensure_drag_autoscroll_tick(&scrolled_c, Rc::clone(&state_c), tick_ms);
        });
    }
    {
        let state_c = Rc::clone(&state);
        controller.connect_leave(move |_| {
            stop_drag_autoscroll(&state_c);
        });
    }
    scrolled.add_controller(controller);
}

#[cfg(test)]
mod tests {
    use super::{
        EdgeAutoScrollConfig, EdgeAutoScrollState, apply_row_offset_correction, apply_scroll_step,
        compute_edge_scroll_step, stop_drag_autoscroll, target_index_from_pointer_y,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn edge_step_has_expected_direction_and_center_zero() {
        let cfg = EdgeAutoScrollConfig::default();
        let h = 600.0;
        assert!(compute_edge_scroll_step(8.0, h, cfg) < 0.0);
        assert!(compute_edge_scroll_step(h - 8.0, h, cfg) > 0.0);
        assert_eq!(compute_edge_scroll_step(h / 2.0, h, cfg), 0.0);
    }

    #[test]
    fn edge_step_accelerates_closer_to_edge() {
        let cfg = EdgeAutoScrollConfig::default();
        let h = 600.0;
        let near = compute_edge_scroll_step(4.0, h, cfg).abs();
        let farther = compute_edge_scroll_step(40.0, h, cfg).abs();
        assert!(near > farther);
    }

    #[test]
    fn apply_scroll_step_clamps_to_range() {
        assert_eq!(apply_scroll_step(5.0, 100.0, 20.0, -50.0), 0.0);
        assert_eq!(apply_scroll_step(70.0, 100.0, 20.0, 30.0), 80.0);
    }

    #[test]
    fn stop_drag_autoscroll_zeroes_step() {
        let state = Rc::new(RefCell::new(EdgeAutoScrollState {
            step_px_per_tick: 12.0,
            ticking: true,
        }));
        stop_drag_autoscroll(&state);
        assert_eq!(state.borrow().step_px_per_tick, 0.0);
    }

    #[test]
    fn row_offset_correction_applies_delta_and_clamps() {
        assert_eq!(
            apply_row_offset_correction(50.0, 400.0, 100.0, 220.0, 180.0),
            90.0
        );
        assert_eq!(
            apply_row_offset_correction(5.0, 120.0, 100.0, 20.0, 80.0),
            0.0
        );
    }

    #[test]
    fn target_index_mapping_uses_row_midpoints() {
        let tops = [0.0, 30.0, 60.0];
        let heights = [30.0, 30.0, 30.0];
        assert_eq!(target_index_from_pointer_y(2.0, &tops, &heights), Some(0));
        assert_eq!(target_index_from_pointer_y(20.0, &tops, &heights), Some(1));
        assert_eq!(target_index_from_pointer_y(55.0, &tops, &heights), Some(2));
        assert_eq!(target_index_from_pointer_y(200.0, &tops, &heights), Some(2));
    }
}
