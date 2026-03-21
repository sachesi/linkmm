use std::cell::RefCell;
use std::rc::Rc;

use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use log::Level;

use crate::core::config::AppConfig;
use crate::core::logger;

/// Polling interval for the log viewer auto-refresh.
const LOG_REFRESH_INTERVAL_MS: u64 = 500;

/// Open a read-only log viewer window.
///
/// The window auto-refreshes every 500 ms and filters entries according to
/// the three logging toggles stored in `config`.
pub fn show_log_window(parent: &gtk4::Window, config: Rc<RefCell<AppConfig>>) {
    let win = adw::Window::builder()
        .title("Application Logs")
        .modal(false)
        .transient_for(parent)
        .default_width(860)
        .default_height(560)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let title_widget = adw::WindowTitle::new("Application Logs", "read-only");
    header.set_title_widget(Some(&title_widget));

    // Clear button
    let clear_btn = gtk4::Button::new();
    clear_btn.set_icon_name("edit-clear-all-symbolic");
    clear_btn.set_tooltip_text(Some("Clear log view"));
    clear_btn.add_css_class("flat");
    header.pack_end(&clear_btn);

    toolbar_view.add_top_bar(&header);

    // ── Text view (read-only) ─────────────────────────────────────────────
    let text_buffer = gtk4::TextBuffer::new(None);

    // Colour tags for each log level
    let tag_error = gtk4::TextTag::builder()
        .name("error")
        .foreground("#e01b24")
        .build();
    let tag_warn = gtk4::TextTag::builder()
        .name("warn")
        .foreground("#e5a50a")
        .build();
    let tag_info = gtk4::TextTag::builder()
        .name("info")
        .foreground("#3584e4")
        .build();
    let tag_time = gtk4::TextTag::builder()
        .name("time")
        .foreground("#888888")
        .build();

    {
        let table = text_buffer.tag_table();
        table.add(&tag_error);
        table.add(&tag_warn);
        table.add(&tag_info);
        table.add(&tag_time);
    }

    let text_view = gtk4::TextView::builder()
        .buffer(&text_buffer)
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .left_margin(8)
        .right_margin(8)
        .top_margin(8)
        .bottom_margin(8)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .build();

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hscrollbar_policy(gtk4::PolicyType::Automatic);
    scrolled.set_child(Some(&text_view));

    toolbar_view.set_content(Some(&scrolled));
    win.set_content(Some(&toolbar_view));

    // ── State shared between the refresh closure and clear button ─────────
    // Track the number of entries that have already been appended so that
    // incremental updates don't re-render the whole buffer on every tick.
    let rendered_count: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
    let cleared_at: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));

    // ── Populate / refresh helper ─────────────────────────────────────────
    let refresh = {
        let text_buffer_c = text_buffer.clone();
        let config_c = Rc::clone(&config);
        let rendered_count_c = Rc::clone(&rendered_count);
        let cleared_at_c = Rc::clone(&cleared_at);
        let tag_error_c = tag_error.clone();
        let tag_warn_c = tag_warn.clone();
        let tag_info_c = tag_info.clone();
        let tag_time_c = tag_time.clone();
        let scrolled_c = scrolled.clone();

        move || {
            let entries = logger::get_logs();
            let skip = *cleared_at_c.borrow();
            let already_rendered = *rendered_count_c.borrow();

            // Determine the slice of new entries to append.
            let new_start = already_rendered.max(skip);
            if new_start >= entries.len() {
                return;
            }
            let new_entries = &entries[new_start..];

            let (show_errors, show_warnings, show_activity) = {
                let cfg = config_c.borrow();
                (cfg.log_errors, cfg.log_warnings, cfg.log_activity)
            };

            let mut end_iter = text_buffer_c.end_iter();

            for entry in new_entries {
                let show = match entry.level {
                    Level::Error => show_errors,
                    Level::Warn => show_warnings,
                    Level::Info => show_activity,
                    // Debug / Trace — treat as activity
                    _ => show_activity,
                };
                if !show {
                    continue;
                }

                // Timestamp in dim grey
                let time_str = format!("[{}] ", entry.time_str());
                text_buffer_c.insert_with_tags(
                    &mut end_iter,
                    &time_str,
                    &[&tag_time_c],
                );

                // Level tag
                let level_str = format!("{} ", entry.level_str());
                let level_tag = match entry.level {
                    Level::Error => &tag_error_c,
                    Level::Warn => &tag_warn_c,
                    _ => &tag_info_c,
                };
                text_buffer_c.insert_with_tags(
                    &mut end_iter,
                    &level_str,
                    &[level_tag],
                );

                // Message and newline
                text_buffer_c.insert(
                    &mut end_iter,
                    &format!("{}\n", entry.message),
                );
            }

            *rendered_count_c.borrow_mut() = entries.len();

            // Auto-scroll to bottom
            let vadj = scrolled_c.vadjustment();
            vadj.set_value(vadj.upper() - vadj.page_size());
        }
    };

    // Initial population
    refresh();

    // ── Periodic refresh every 500 ms ─────────────────────────────────────
    let refresh_rc: Rc<dyn Fn()> = Rc::new(refresh);
    let refresh_timeout = Rc::clone(&refresh_rc);
    let win_weak = win.downgrade();

    glib::timeout_add_local(std::time::Duration::from_millis(LOG_REFRESH_INTERVAL_MS), move || {
        // Stop the timer once the window is closed.
        if win_weak.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        refresh_timeout();
        glib::ControlFlow::Continue
    });

    // ── Clear button handler ───────────────────────────────────────────────
    {
        let text_buffer_c = text_buffer.clone();
        let rendered_count_c = Rc::clone(&rendered_count);
        let cleared_at_c = Rc::clone(&cleared_at);

        clear_btn.connect_clicked(move |_| {
            text_buffer_c.set_text("");
            // Treat all entries up to now as "already seen but cleared"
            let total = logger::get_logs().len();
            *cleared_at_c.borrow_mut() = total;
            *rendered_count_c.borrow_mut() = total;
        });
    }

    win.present();
}
