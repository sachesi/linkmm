use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::Game;
use crate::core::installer::{
    DependencyOperator, FlagDependency, FomodConfig, FomodFile, FomodGroupType, FomodInstallStep,
    FomodPlugin, FomodPluginType, InstallStrategy, PluginDependencies,
};

/// Selected plugin indices by `[step_index][group_index][plugin_indices]`.
type FomodSelections = Vec<Vec<Vec<usize>>>;

fn collect_active_flags(
    fomod: &FomodConfig,
    selections: &FomodSelections,
    up_to_step_inclusive: usize,
) -> HashMap<String, HashSet<String>> {
    let mut flags: HashMap<String, HashSet<String>> = HashMap::new();
    for (si, step) in fomod.steps.iter().enumerate() {
        if si > up_to_step_inclusive {
            break;
        }
        if !step_is_visible_with_flags(step, &flags) {
            continue;
        }
        for (gi, group) in step.groups.iter().enumerate() {
            let Some(selected) = selections.get(si).and_then(|s| s.get(gi)) else {
                continue;
            };
            for &pi in selected {
                let Some(plugin) = group.plugins.get(pi) else {
                    continue;
                };
                for flag in &plugin.condition_flags {
                    flags
                        .entry(flag.name.clone())
                        .or_default()
                        .insert(flag.value.clone());
                }
            }
        }
    }
    flags
}

fn flag_dependency_matches(
    dep: &FlagDependency,
    active_flags: &HashMap<String, HashSet<String>>,
) -> bool {
    if let Some(values) = active_flags.get(&dep.flag) {
        values.contains(&dep.value)
    } else {
        false
    }
}

fn dependencies_match(
    dependencies: &PluginDependencies,
    active_flags: &HashMap<String, HashSet<String>>,
) -> bool {
    if dependencies.flags.is_empty() {
        return true;
    }
    match dependencies.operator {
        DependencyOperator::And => dependencies
            .flags
            .iter()
            .all(|dep| flag_dependency_matches(dep, active_flags)),
        DependencyOperator::Or => dependencies
            .flags
            .iter()
            .any(|dep| flag_dependency_matches(dep, active_flags)),
    }
}

fn step_is_visible_with_flags(
    step: &FomodInstallStep,
    active_flags: &HashMap<String, HashSet<String>>,
) -> bool {
    let Some(visible) = &step.visible else {
        return true;
    };
    dependencies_match(visible, active_flags)
}

fn plugin_is_visible(
    plugin: &FomodPlugin,
    active_flags: &HashMap<String, HashSet<String>>,
) -> bool {
    let Some(deps) = &plugin.dependencies else {
        return true;
    };
    dependencies_match(deps, active_flags)
}

fn sanitize_step_selection(
    fomod: &FomodConfig,
    selections: &mut FomodSelections,
    step_index: usize,
) {
    let Some(step) = fomod.steps.get(step_index) else {
        return;
    };
    let active_flags = collect_active_flags(fomod, selections, step_index);
    for (gi, group) in step.groups.iter().enumerate() {
        let Some(selected) = selections.get_mut(step_index).and_then(|s| s.get_mut(gi)) else {
            continue;
        };
        let visible: Vec<usize> = group
            .plugins
            .iter()
            .enumerate()
            .filter_map(|(pi, plugin)| {
                if plugin_is_visible(plugin, &active_flags) {
                    Some(pi)
                } else {
                    None
                }
            })
            .collect();
        selected.retain(|pi| visible.contains(pi));
        match group.group_type {
            FomodGroupType::SelectAll => {
                *selected = visible;
            }
            FomodGroupType::SelectExactlyOne => {
                if selected.len() > 1 {
                    selected.truncate(1);
                }
                if selected.is_empty()
                    && let Some(first) = visible.first()
                {
                    selected.push(*first);
                }
            }
            FomodGroupType::SelectAtLeastOne => {
                if selected.is_empty()
                    && let Some(first) = visible.first()
                {
                    selected.push(*first);
                }
            }
            FomodGroupType::SelectAtMostOne => {
                if selected.len() > 1 {
                    selected.truncate(1);
                }
            }
            FomodGroupType::SelectAny => {}
        }
        selected.sort();
        selected.dedup();
    }
}

fn resolve_fomod_files(fomod: &FomodConfig, selections: &FomodSelections) -> Vec<FomodFile> {
    let mut files: Vec<FomodFile> = fomod.required_files.clone();
    let mut normalized = selections.clone();
    for si in 0..fomod.steps.len() {
        sanitize_step_selection(fomod, &mut normalized, si);
    }
    for (si, step) in fomod.steps.iter().enumerate() {
        let step_flags = if si == 0 {
            HashMap::new()
        } else {
            collect_active_flags(fomod, &normalized, si - 1)
        };
        if !step_is_visible_with_flags(step, &step_flags) {
            continue;
        }
        for (gi, group) in step.groups.iter().enumerate() {
            if let Some(selected) = normalized.get(si).and_then(|s| s.get(gi)) {
                for &pi in selected {
                    if let Some(plugin) = group.plugins.get(pi) {
                        files.extend(plugin.files.iter().cloned());
                    }
                }
            }
        }
    }
    let active_flags = if fomod.steps.is_empty() {
        HashMap::new()
    } else {
        collect_active_flags(fomod, &normalized, fomod.steps.len() - 1)
    };
    for conditional in &fomod.conditional_file_installs {
        if dependencies_match(&conditional.dependencies, &active_flags) {
            files.extend(conditional.files.iter().cloned());
        }
    }
    files
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn show_fomod_wizard(
    parent: Option<&gtk4::Window>,
    archive_name: &str,
    fomod: &FomodConfig,
    images_data: HashMap<String, Vec<u8>>,
    on_install_fn: impl Fn(InstallStrategy) + 'static,
) {
    let mod_display_name = fomod
        .mod_name
        .clone()
        .unwrap_or_else(|| archive_name.to_string());

    // No interactive steps — install required files immediately.
    if fomod.steps.is_empty() {
        let strategy = InstallStrategy::Fomod(fomod.required_files.clone());
        on_install_fn(strategy);
        return;
    }

    // Initialise per-step, per-group selections with defaults.
    let selections: Rc<RefCell<FomodSelections>> = Rc::new(RefCell::new(Vec::new()));
    {
        let mut sel = selections.borrow_mut();
        for step in &fomod.steps {
            let mut step_sel = Vec::new();
            for group in &step.groups {
                let mut gs: Vec<usize> = Vec::new();
                for (idx, plugin) in group.plugins.iter().enumerate() {
                    if matches!(
                        plugin.type_descriptor,
                        FomodPluginType::Required | FomodPluginType::Recommended
                    ) || group.group_type == FomodGroupType::SelectAll
                    {
                        gs.push(idx);
                    }
                }
                if gs.is_empty()
                    && matches!(
                        group.group_type,
                        FomodGroupType::SelectExactlyOne | FomodGroupType::SelectAtLeastOne
                    )
                    && !group.plugins.is_empty()
                {
                    gs.push(0);
                }
                gs.sort();
                gs.dedup();
                step_sel.push(gs);
            }
            sel.push(step_sel);
        }
    }

    // Convert raw image bytes into GPU textures once, up-front.
    let image_cache: Rc<RefCell<HashMap<String, gtk4::gdk::Texture>>> =
        Rc::new(RefCell::new(HashMap::new()));
    {
        let mut cache = image_cache.borrow_mut();
        for (path, bytes) in images_data {
            if let Ok(texture) =
                gtk4::gdk::Texture::from_bytes(&gtk4::glib::Bytes::from_owned(bytes))
            {
                cache.insert(path, texture);
            }
        }
    }

    // ── adw::Dialog ──────────────────────────────────────────────────────────
    let dialog = adw::Dialog::builder()
        .title(format!("Install: {mod_display_name}"))
        .content_width(920)
        .content_height(640)
        .build();

    let nav_view = adw::NavigationView::new();
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&nav_view));
    dialog.set_child(Some(&toast_overlay));

    // ── Shared install callback ───────────────────────────────────────────────
    let dlg = dialog.clone();
    let fomod_rc = Rc::new(fomod.clone());
    let sc_install = Rc::clone(&selections);
    let fc_install = Rc::clone(&fomod_rc);
    let on_install_fn = Rc::new(on_install_fn);
    let on_install: Rc<dyn Fn()> = Rc::new(move || {
        let files = {
            let sel = sc_install.borrow();
            resolve_fomod_files(&fc_install, &sel)
        };
        dlg.close();
        on_install_fn(InstallStrategy::Fomod(files));
    });

    // Build and push the first visible step page.
    let initial_flags = HashMap::new();
    if let Some(first_idx) = (0..fomod_rc.steps.len())
        .find(|&i| step_is_visible_with_flags(&fomod_rc.steps[i], &initial_flags))
    {
        let page = build_fomod_nav_page(
            first_idx,
            Rc::clone(&fomod_rc),
            Rc::clone(&selections),
            Rc::clone(&image_cache),
            nav_view.clone(),
            Rc::clone(&on_install),
        );
        nav_view.push(&page);
    }

    dialog.present(parent);
}

/// State exposed by the left info pane so rows can update it on hover.
struct FomodInfoPane {
    /// The root widget to set as the paned start child.
    root: gtk4::Box,
    /// The image display — update `set_paintable` on hover.
    picture: gtk4::Picture,
    /// Plugin name label below the image — update `set_label` on hover.
    name_label: gtk4::Label,
    /// Description label — update `set_label` on hover.
    desc_label: gtk4::Label,
}

fn build_fomod_info_pane(
    fomod: &FomodConfig,
    step_idx: usize,
    image_cache: &Rc<RefCell<HashMap<String, gtk4::gdk::Texture>>>,
) -> FomodInfoPane {
    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    root.set_width_request(300);
    root.add_css_class("sidebar");

    // ── Image panel ──────────────────────────────────────────────────────────
    let picture = gtk4::Picture::new();
    picture.set_content_fit(gtk4::ContentFit::Contain);
    picture.set_can_shrink(true);
    picture.set_size_request(300, 220);
    picture.set_halign(gtk4::Align::Fill);
    picture.set_valign(gtk4::Align::Start);

    // Pre-fill with the first image found in this step, if any.
    if let Some(first_image) = fomod.steps.get(step_idx).and_then(|s| {
        s.groups
            .iter()
            .flat_map(|g| &g.plugins)
            .find_map(|p| p.image_path.as_ref())
    }) {
        if let Some(tex) = image_cache.borrow().get(first_image).cloned() {
            picture.set_paintable(Some(&tex));
        }
    }

    // ── Click-to-fullscreen ───────────────────────────────────────────────────
    let img_btn = gtk4::Button::new();
    img_btn.set_child(Some(&picture));
    img_btn.add_css_class("flat");
    img_btn.add_css_class("fomod-preview-btn");
    img_btn.set_margin_top(12);
    img_btn.set_margin_start(12);
    img_btn.set_margin_end(12);
    img_btn.set_cursor(gtk4::gdk::Cursor::from_name("zoom-in", None).as_ref());

    // Rounded corners on the image: inject CSS once when the widget gets a
    // display (i.e. after it is realized inside a window).
    let css_provider = gtk4::CssProvider::new();
    css_provider.load_from_string(".fomod-preview-btn picture { border-radius: 12px; }");
    picture.connect_realize(move |widget| {
        gtk4::style_context_add_provider_for_display(
            &widget.display(),
            &css_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });

    let picture_clone = picture.clone();
    img_btn.connect_clicked(move |btn| {
        if let Some(paintable) = picture_clone.paintable() {
            let parent_widget: Option<gtk4::Widget> =
                btn.root().map(|r| r.upcast::<gtk4::Widget>());
            show_fullscreen_image_dialog(&paintable, parent_widget.as_ref());
        }
    });

    root.append(&img_btn);

    // ── Plugin name below the image ───────────────────────────────────────────
    let name_label = gtk4::Label::new(None);
    name_label.add_css_class("dim-label");
    name_label.add_css_class("caption");
    name_label.set_halign(gtk4::Align::Center);
    name_label.set_margin_top(4);
    name_label.set_margin_bottom(8);
    name_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    root.append(&name_label);

    // ── Separator ─────────────────────────────────────────────────────────────
    root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    // ── Description text (scrollable, read-only) ─────────────────────────────
    let desc_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .build();

    let initial_desc = fomod
        .steps
        .get(step_idx)
        .and_then(|s| {
            s.groups
                .iter()
                .flat_map(|g| &g.plugins)
                .find_map(|p| p.description.as_ref())
        })
        .map(|d| normalize_line_endings(d))
        .unwrap_or_default();

    let desc_label = gtk4::Label::new(Some(initial_desc.trim()));
    desc_label.set_wrap(true);
    desc_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    desc_label.set_xalign(0.0);
    desc_label.set_valign(gtk4::Align::Start);
    desc_label.set_selectable(true);
    desc_label.set_margin_top(12);
    desc_label.set_margin_bottom(12);
    desc_label.set_margin_start(12);
    desc_label.set_margin_end(12);

    desc_scroll.set_child(Some(&desc_label));
    root.append(&desc_scroll);

    FomodInfoPane {
        root,
        picture,
        name_label,
        desc_label,
    }
}

/// Normalize CRLF / CR line endings to LF.
fn normalize_line_endings(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Attach an `EventControllerMotion` to `row` that updates the left info pane
/// on hover. If `texture` is `Some`, the panel image is updated too; otherwise
/// the last displayed image is kept (MO2 behaviour).
fn attach_hover_controller(
    row: &adw::ActionRow,
    info_pane: &FomodInfoPane,
    texture: Option<&gtk4::gdk::Texture>,
    plugin_name: &str,
    description: Option<&str>,
) {
    let panel_pic = info_pane.picture.clone();
    let panel_name = info_pane.name_label.clone();
    let panel_desc = info_pane.desc_label.clone();
    let hover_tex = texture.cloned();
    let name_owned = plugin_name.to_string();
    let desc_owned = description.unwrap_or_default().to_string();

    let motion = gtk4::EventControllerMotion::new();
    motion.connect_enter(move |_, _, _| {
        if let Some(tex) = &hover_tex {
            panel_pic.set_paintable(Some(tex));
        }
        panel_name.set_label(&name_owned);
        let cleaned = normalize_line_endings(&desc_owned);
        panel_desc.set_label(cleaned.trim());
    });
    row.add_controller(motion);
}

/// Open a full-size image in a modal adw::Dialog.
fn show_fullscreen_image_dialog(
    paintable: &impl gtk4::gdk::prelude::IsA<gtk4::gdk::Paintable>,
    parent: Option<&gtk4::Widget>,
) {
    let dialog = adw::Dialog::builder()
        .title("Preview")
        .content_width(900)
        .content_height(700)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&adw::HeaderBar::new());

    let picture = gtk4::Picture::new();
    picture.set_paintable(Some(paintable));
    picture.set_content_fit(gtk4::ContentFit::Contain);
    picture.set_can_shrink(true);
    picture.set_vexpand(true);
    picture.set_hexpand(true);
    picture.set_margin_top(12);
    picture.set_margin_bottom(12);
    picture.set_margin_start(12);
    picture.set_margin_end(12);

    toolbar_view.set_content(Some(&picture));
    dialog.set_child(Some(&toolbar_view));
    dialog.present(parent);
}

/// Build one `adw::NavigationPage` for `step_idx` in `fomod`, following the
/// GNOME HIG with an MO2-style two-pane layout:
///
/// * `adw::ToolbarView` with `adw::HeaderBar` (provides the back button).
/// * Left pane (300 px): image preview, plugin name, and description.
/// * Right pane: scrollable options list with plugin groups as
///   `adw::PreferencesGroup` + `gtk4::ListBox` with `boxed-list` class and
///   `adw::ActionRow` per plugin.
/// * Hovering a row updates the left-pane preview; no inline row thumbnails.
/// * Next / Install pill button in the `ToolbarView` bottom bar.
/// * Next is insensitive until every required group has a valid selection.
fn build_fomod_nav_page(
    step_idx: usize,
    fomod: Rc<FomodConfig>,
    selections: Rc<RefCell<FomodSelections>>,
    image_cache: Rc<RefCell<HashMap<String, gtk4::gdk::Texture>>>,
    nav_view: adw::NavigationView,
    on_install: Rc<dyn Fn()>,
) -> adw::NavigationPage {
    // Ensure selections for this step are consistent with its constraints.
    {
        let mut sel = selections.borrow_mut();
        sanitize_step_selection(&fomod, &mut sel, step_idx);
    }

    let step = &fomod.steps[step_idx];

    // Flags set by all steps that precede this one (used for plugin-level
    // visibility within the current step).
    let prev_flags = {
        let sel = selections.borrow();
        if step_idx > 0 {
            collect_active_flags(&fomod, &sel, step_idx - 1)
        } else {
            HashMap::new()
        }
    };

    // ── NavigationPage ───────────────────────────────────────────────────────
    let page = adw::NavigationPage::builder().title(&step.name).build();

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&adw::HeaderBar::new());

    // ── Forward button (Next / Install) ──────────────────────────────────────
    let is_last_initially = {
        let sel = selections.borrow();
        let flags = collect_active_flags(&fomod, &sel, step_idx);
        (step_idx + 1..fomod.steps.len())
            .find(|&i| step_is_visible_with_flags(&fomod.steps[i], &flags))
            .is_none()
    };

    let fwd_btn = gtk4::Button::with_label(if is_last_initially { "Install" } else { "Next" });
    fwd_btn.add_css_class("suggested-action");
    fwd_btn.add_css_class("pill");
    fwd_btn.set_halign(gtk4::Align::Center);
    fwd_btn.set_margin_top(12);
    fwd_btn.set_margin_bottom(12);

    fwd_btn.set_sensitive(step_selection_is_valid(
        &fomod,
        step_idx,
        &selections.borrow(),
    ));

    // ── Forward-navigation handler ───────────────────────────────────────────
    {
        let fc = Rc::clone(&fomod);
        let sc = Rc::clone(&selections);
        let ic = Rc::clone(&image_cache);
        let nv = nav_view.clone();
        let oi = Rc::clone(&on_install);
        fwd_btn.connect_clicked(move |_| {
            let next_flags = {
                let sel = sc.borrow();
                collect_active_flags(&fc, &sel, step_idx)
            };
            let next_idx = (step_idx + 1..fc.steps.len())
                .find(|&i| step_is_visible_with_flags(&fc.steps[i], &next_flags));
            if let Some(next_idx) = next_idx {
                let next_page = build_fomod_nav_page(
                    next_idx,
                    Rc::clone(&fc),
                    Rc::clone(&sc),
                    Rc::clone(&ic),
                    nv.clone(),
                    Rc::clone(&oi),
                );
                nv.push(&next_page);
            } else {
                oi();
            }
        });
    }

    // ── Empty step guard (§4f) ────────────────────────────────────────────────
    let all_groups_empty = step
        .groups
        .iter()
        .all(|g| g.plugins.iter().all(|p| !plugin_is_visible(p, &prev_flags)));

    if all_groups_empty {
        let empty = adw::StatusPage::builder()
            .icon_name("dialog-information-symbolic")
            .title("No Options Available")
            .description("All options in this step are hidden based on your earlier selections.")
            .vexpand(true)
            .build();
        toolbar_view.set_content(Some(&empty));
        fwd_btn.set_sensitive(true);
        let action_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        action_bar.set_halign(gtk4::Align::Center);
        action_bar.append(&fwd_btn);
        toolbar_view.add_bottom_bar(&action_bar);
        page.set_child(Some(&toolbar_view));
        return page;
    }

    // ── Horizontal paned: left=info, right=options ────────────────────────────
    let paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
    paned.set_vexpand(true);
    paned.set_hexpand(true);
    paned.set_shrink_start_child(false);
    paned.set_shrink_end_child(false);
    paned.set_resize_start_child(false);
    paned.set_resize_end_child(true);
    paned.set_position(300);

    // ── LEFT PANE ─────────────────────────────────────────────────────────────
    let info_pane = build_fomod_info_pane(&fomod, step_idx, &image_cache);
    paned.set_start_child(Some(&info_pane.root));

    // ── RIGHT PANE ────────────────────────────────────────────────────────────
    let right_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .build();

    let right_box = gtk4::Box::new(gtk4::Orientation::Vertical, 18);
    right_box.set_margin_top(18);
    right_box.set_margin_bottom(18);
    right_box.set_margin_start(12);
    right_box.set_margin_end(18);

    // ── Validation banner (§4e) ───────────────────────────────────────────────
    let validation_banner = adw::Banner::builder()
        .title("Please make a selection in all required groups to continue")
        .revealed(!step_selection_is_valid(
            &fomod,
            step_idx,
            &selections.borrow(),
        ))
        .build();
    right_box.append(&validation_banner);

    let fwd_btn_ref = fwd_btn.clone();
    let validation_banner_ref = validation_banner.clone();

    // ── Plugin groups ────────────────────────────────────────────────────────
    for (gi, group) in step.groups.iter().enumerate() {
        // Only include plugins that pass dependency-flag visibility check.
        let visible_plugins: Vec<usize> = group
            .plugins
            .iter()
            .enumerate()
            .filter_map(|(pi, plugin)| {
                if plugin_is_visible(plugin, &prev_flags) {
                    Some(pi)
                } else {
                    None
                }
            })
            .collect();

        if visible_plugins.is_empty() {
            continue;
        }

        // §4a: group description instead of title suffix.
        let hint = match group.group_type {
            FomodGroupType::SelectExactlyOne => "Select one option",
            FomodGroupType::SelectAtMostOne => "Select at most one option",
            FomodGroupType::SelectAtLeastOne => "Select at least one option",
            FomodGroupType::SelectAll => "All options are included",
            FomodGroupType::SelectAny => "Select any number of options",
        };

        let pref_group = adw::PreferencesGroup::builder()
            .title(&group.name)
            .description(hint)
            .build();

        let list = gtk4::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk4::SelectionMode::None);
        list.set_hexpand(true);

        let use_radio = matches!(
            group.group_type,
            FomodGroupType::SelectExactlyOne | FomodGroupType::SelectAtMostOne
        );
        let mut radio_leader: Option<gtk4::CheckButton> = None;

        for &pi in &visible_plugins {
            let plugin = &group.plugins[pi];
            let row = adw::ActionRow::builder().title(&plugin.name).build();

            // §4c: CRLF cleanup + 3-line truncation in subtitle.
            // `.lines()` handles \r\n, \r, and \n uniformly.
            if let Some(ref d) = plugin.description {
                let truncated: String = d.lines().take(3).collect::<Vec<_>>().join("\n");
                let trimmed = truncated.trim();
                if !trimmed.is_empty() {
                    row.set_subtitle(trimmed);
                }
            }

            // Hover → update left panel image, name, and description.
            // Thumbnails are no longer shown inline; the left pane handles preview.
            let hover_texture = plugin
                .image_path
                .as_ref()
                .and_then(|ip| image_cache.borrow().get(ip).cloned());
            attach_hover_controller(
                &row,
                &info_pane,
                hover_texture.as_ref(),
                &plugin.name,
                plugin.description.as_deref(),
            );

            match group.group_type {
                FomodGroupType::SelectAll => {
                    // All plugins are auto-selected; show a read-only badge
                    // instead of an interactive checkbox.
                    row.set_sensitive(false);
                    row.set_activatable(false);
                    let badge = gtk4::Label::new(Some("Included"));
                    badge.add_css_class("dim-label");
                    badge.add_css_class("caption");
                    badge.set_valign(gtk4::Align::Center);
                    row.add_suffix(&badge);
                }
                _ => {
                    let check = gtk4::CheckButton::new();
                    check.set_valign(gtk4::Align::Center);

                    // Use GTK radio-group semantics — no manual uncheck loops.
                    if use_radio {
                        if let Some(ref leader) = radio_leader {
                            check.set_group(Some(leader));
                        } else {
                            radio_leader = Some(check.clone());
                        }
                    }

                    // Restore persisted selection state.
                    {
                        let sel = selections.borrow();
                        if let Some(gs) = sel.get(step_idx).and_then(|s| s.get(gi)) {
                            check.set_active(gs.contains(&pi));
                        }
                    }

                    // Required plugins: pre-checked and locked.
                    if plugin.type_descriptor == FomodPluginType::Required {
                        check.set_active(true);
                        check.set_sensitive(false);
                    }

                    // NotUsable plugins: greyed out with an explanatory tooltip.
                    if plugin.type_descriptor == FomodPluginType::NotUsable {
                        row.set_sensitive(false);
                        row.set_tooltip_text(Some("Not available for your current setup"));
                    }

                    // Update selections and button/banner state on every toggle.
                    let sel_c = Rc::clone(&selections);
                    let fomod_c = Rc::clone(&fomod);
                    let nb = fwd_btn_ref.clone();
                    let vb = validation_banner_ref.clone();
                    let is_radio = use_radio;
                    check.connect_toggled(move |btn| {
                        let mut sel = sel_c.borrow_mut();
                        if let Some(gs) = sel.get_mut(step_idx).and_then(|s| s.get_mut(gi)) {
                            if btn.is_active() {
                                if is_radio {
                                    gs.clear();
                                }
                                if !gs.contains(&pi) {
                                    gs.push(pi);
                                }
                            } else {
                                gs.retain(|&x| x != pi);
                            }
                        }
                        // Sensitivity: re-validate required groups.
                        let valid = step_selection_is_valid(&fomod_c, step_idx, &sel);
                        nb.set_sensitive(valid);
                        vb.set_revealed(!valid);
                        // Label: re-evaluate step visibility so Install/Next
                        // stays accurate when selections affect later steps.
                        let cur_flags = collect_active_flags(&fomod_c, &sel, step_idx);
                        let still_last = (step_idx + 1..fomod_c.steps.len())
                            .find(|&i| step_is_visible_with_flags(&fomod_c.steps[i], &cur_flags))
                            .is_none();
                        nb.set_label(if still_last { "Install" } else { "Next" });
                    });

                    // Type badge (Required / Recommended / Not Usable).
                    let tl = match plugin.type_descriptor {
                        FomodPluginType::Required => Some("Required"),
                        FomodPluginType::Recommended => Some("Recommended"),
                        FomodPluginType::NotUsable => Some("Not Usable"),
                        FomodPluginType::Optional => None,
                    };
                    if let Some(lt) = tl {
                        let badge = gtk4::Label::new(Some(lt));
                        badge.add_css_class("dim-label");
                        badge.add_css_class("caption");
                        badge.set_valign(gtk4::Align::Center);
                        row.add_suffix(&badge);
                    }

                    row.add_suffix(&check);
                    row.set_activatable_widget(Some(&check));
                }
            }

            list.append(&row);
        }

        pref_group.add(&list);
        right_box.append(&pref_group);
    }

    // ── Assemble paned layout ───────────────────────────────────────────────
    right_scroll.set_child(Some(&right_box));
    paned.set_end_child(Some(&right_scroll));
    toolbar_view.set_content(Some(&paned));

    // ── Bottom action bar — Next / Install pill button ────────────────────────
    let action_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    action_bar.set_halign(gtk4::Align::Center);
    action_bar.append(&fwd_btn);
    toolbar_view.add_bottom_bar(&action_bar);

    page.set_child(Some(&toolbar_view));

    page
}

/// Returns `true` when every group in `step_idx` that mandates a selection
/// (SelectExactlyOne, SelectAtLeastOne) has at least one plugin chosen.
fn step_selection_is_valid(
    fomod: &FomodConfig,
    step_idx: usize,
    selections: &FomodSelections,
) -> bool {
    let Some(step) = fomod.steps.get(step_idx) else {
        return true;
    };
    for (gi, group) in step.groups.iter().enumerate() {
        let count = selections
            .get(step_idx)
            .and_then(|s| s.get(gi))
            .map(|gs| gs.len())
            .unwrap_or(0);
        match group.group_type {
            FomodGroupType::SelectExactlyOne | FomodGroupType::SelectAtLeastOne => {
                if count == 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
pub fn show_fomod_wizard_from_library(
    parent: Option<&gtk4::Window>,
    archive_path: &Path,
    archive_name: &str,
    game: &Game,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    fomod: &FomodConfig,
    game_rc: &Rc<Option<Game>>,
) {
    let ap = archive_path.to_path_buf();
    let an = archive_name.to_string();
    let gc = game.clone();
    let cc = Rc::clone(config);
    let cont = container.clone();
    let grc = Rc::clone(game_rc);
    show_fomod_wizard(
        parent,
        archive_name,
        fomod,
        HashMap::new(),
        move |strategy| {
            crate::ui::downloads::do_install(
                &ap, &an, &gc, &cc, &cont, false, "", &strategy, &grc, None, None,
            );
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::installer::{ConditionFlag, ConditionalFileInstall, FomodPluginGroup};

    fn test_plugin(
        name: &str,
        file_source: &str,
        condition_flags: Vec<ConditionFlag>,
        dependencies: Option<PluginDependencies>,
    ) -> FomodPlugin {
        FomodPlugin {
            name: name.to_string(),
            description: None,
            image_path: None,
            files: vec![FomodFile {
                source: file_source.to_string(),
                destination: "Data".to_string(),
                priority: 0,
            }],
            type_descriptor: FomodPluginType::Optional,
            condition_flags,
            dependencies,
        }
    }

    #[test]
    fn resolve_fomod_files_filters_plus_minus_variants_by_dependency_flags() {
        let mut config = FomodConfig {
            mod_name: Some("Test".to_string()),
            required_files: Vec::new(),
            steps: Vec::new(),
            conditional_file_installs: Vec::new(),
        };
        config.steps.push(FomodInstallStep {
            name: "Flags".to_string(),
            visible: None,
            groups: vec![FomodPluginGroup {
                name: "Feature".to_string(),
                group_type: FomodGroupType::SelectExactlyOne,
                plugins: vec![
                    test_plugin(
                        "Enable +",
                        "flags-plus.txt",
                        vec![ConditionFlag {
                            name: "FeaturePack".to_string(),
                            value: "+".to_string(),
                        }],
                        None,
                    ),
                    test_plugin(
                        "Enable -",
                        "flags-minus.txt",
                        vec![ConditionFlag {
                            name: "FeaturePack".to_string(),
                            value: "-".to_string(),
                        }],
                        None,
                    ),
                ],
            }],
        });
        config.steps.push(FomodInstallStep {
            name: "Variant".to_string(),
            visible: None,
            groups: vec![FomodPluginGroup {
                name: "Pick".to_string(),
                group_type: FomodGroupType::SelectAny,
                plugins: vec![
                    test_plugin(
                        "Plus variant",
                        "plus-variant.txt",
                        Vec::new(),
                        Some(PluginDependencies {
                            operator: DependencyOperator::And,
                            flags: vec![FlagDependency {
                                flag: "FeaturePack".to_string(),
                                value: "+".to_string(),
                            }],
                        }),
                    ),
                    test_plugin(
                        "Minus variant",
                        "minus-variant.txt",
                        Vec::new(),
                        Some(PluginDependencies {
                            operator: DependencyOperator::And,
                            flags: vec![FlagDependency {
                                flag: "FeaturePack".to_string(),
                                value: "-".to_string(),
                            }],
                        }),
                    ),
                ],
            }],
        });

        // Simulate stale UI state selecting both variants in step 2 while step 1
        // has "+" selected.
        let selections = vec![vec![vec![0]], vec![vec![0, 1]]];
        let files = resolve_fomod_files(&config, &selections);
        let sources: Vec<String> = files.into_iter().map(|f| f.source).collect();
        assert!(sources.contains(&"plus-variant.txt".to_string()));
        assert!(!sources.contains(&"minus-variant.txt".to_string()));
    }

    #[test]
    fn sanitize_step_selection_respects_at_most_and_exactly_one_rules() {
        let config = FomodConfig {
            mod_name: Some("Test".to_string()),
            required_files: Vec::new(),
            conditional_file_installs: Vec::new(),
            steps: vec![FomodInstallStep {
                name: "Main".to_string(),
                visible: None,
                groups: vec![
                    FomodPluginGroup {
                        name: "Exactly one".to_string(),
                        group_type: FomodGroupType::SelectExactlyOne,
                        plugins: vec![
                            test_plugin("A", "a.txt", Vec::new(), None),
                            test_plugin("B", "b.txt", Vec::new(), None),
                        ],
                    },
                    FomodPluginGroup {
                        name: "At most one".to_string(),
                        group_type: FomodGroupType::SelectAtMostOne,
                        plugins: vec![
                            test_plugin("C", "c.txt", Vec::new(), None),
                            test_plugin("D", "d.txt", Vec::new(), None),
                        ],
                    },
                ],
            }],
        };
        let mut selections = vec![vec![vec![0, 1], vec![0, 1]]];
        sanitize_step_selection(&config, &mut selections, 0);
        assert_eq!(selections[0][0], vec![0]);
        assert_eq!(selections[0][1], vec![0]);

        let mut empty_at_most_one = vec![vec![vec![1], vec![]]];
        sanitize_step_selection(&config, &mut empty_at_most_one, 0);
        assert_eq!(empty_at_most_one[0][1], Vec::<usize>::new());
    }

    #[test]
    fn resolve_fomod_files_applies_step_visibility_and_conditional_files() {
        let config = FomodConfig {
            mod_name: Some("Test".to_string()),
            required_files: Vec::new(),
            steps: vec![
                FomodInstallStep {
                    name: "Flags".to_string(),
                    visible: None,
                    groups: vec![FomodPluginGroup {
                        name: "Feature".to_string(),
                        group_type: FomodGroupType::SelectExactlyOne,
                        plugins: vec![
                            test_plugin(
                                "Enable Underwear",
                                "underwear-on.txt",
                                vec![ConditionFlag {
                                    name: "bUnderwear".to_string(),
                                    value: "On".to_string(),
                                }],
                                None,
                            ),
                            test_plugin("Disable Underwear", "underwear-off.txt", Vec::new(), None),
                        ],
                    }],
                },
                FomodInstallStep {
                    name: "Underwear Options".to_string(),
                    visible: Some(PluginDependencies {
                        operator: DependencyOperator::And,
                        flags: vec![FlagDependency {
                            flag: "bUnderwear".to_string(),
                            value: "On".to_string(),
                        }],
                    }),
                    groups: vec![FomodPluginGroup {
                        name: "Color".to_string(),
                        group_type: FomodGroupType::SelectExactlyOne,
                        plugins: vec![test_plugin(
                            "Black",
                            "underwear-black.txt",
                            Vec::new(),
                            None,
                        )],
                    }],
                },
            ],
            conditional_file_installs: vec![ConditionalFileInstall {
                dependencies: PluginDependencies {
                    operator: DependencyOperator::And,
                    flags: vec![FlagDependency {
                        flag: "bUnderwear".to_string(),
                        value: "On".to_string(),
                    }],
                },
                files: vec![FomodFile {
                    source: "conditional-underwear.txt".to_string(),
                    destination: "Data".to_string(),
                    priority: 0,
                }],
            }],
        };

        // Step 1 picks "Disable Underwear". Step 2 has stale selection but should
        // not contribute because the step is hidden.
        let hidden_step_selections = vec![vec![vec![1]], vec![vec![0]]];
        let hidden_files = resolve_fomod_files(&config, &hidden_step_selections);
        let hidden_sources: Vec<String> = hidden_files.into_iter().map(|f| f.source).collect();
        assert!(!hidden_sources.contains(&"underwear-black.txt".to_string()));
        assert!(!hidden_sources.contains(&"conditional-underwear.txt".to_string()));

        // Step 1 picks "Enable Underwear", so step 2 and conditional files apply.
        let visible_step_selections = vec![vec![vec![0]], vec![vec![0]]];
        let visible_files = resolve_fomod_files(&config, &visible_step_selections);
        let visible_sources: Vec<String> = visible_files.into_iter().map(|f| f.source).collect();
        assert!(visible_sources.contains(&"underwear-black.txt".to_string()));
        assert!(visible_sources.contains(&"conditional-underwear.txt".to_string()));
    }

    #[test]
    fn resolve_fomod_files_diamond_skin_pattern_no_direct_plugin_files() {
        // Simulates Diamond Skin: plugins have ONLY conditionFlags (no direct
        // <files> elements).  All actual files come from conditionalFileInstalls.
        let config = FomodConfig {
            mod_name: Some("Diamond Skin".to_string()),
            required_files: Vec::new(),
            steps: vec![FomodInstallStep {
                name: "Body Type".to_string(),
                visible: None,
                groups: vec![FomodPluginGroup {
                    name: "Body".to_string(),
                    group_type: FomodGroupType::SelectExactlyOne,
                    plugins: vec![
                        FomodPlugin {
                            name: "CBBE".to_string(),
                            description: None,
                            image_path: None,
                            files: Vec::new(), // No direct files
                            type_descriptor: FomodPluginType::Optional,
                            condition_flags: vec![ConditionFlag {
                                name: "isCBBE".to_string(),
                                value: "selected".to_string(),
                            }],
                            dependencies: None,
                        },
                        FomodPlugin {
                            name: "UNP".to_string(),
                            description: None,
                            image_path: None,
                            files: Vec::new(), // No direct files
                            type_descriptor: FomodPluginType::Optional,
                            condition_flags: vec![ConditionFlag {
                                name: "isUNP".to_string(),
                                value: "selected".to_string(),
                            }],
                            dependencies: None,
                        },
                    ],
                }],
            }],
            conditional_file_installs: vec![
                ConditionalFileInstall {
                    dependencies: PluginDependencies {
                        operator: DependencyOperator::And,
                        flags: vec![FlagDependency {
                            flag: "isCBBE".to_string(),
                            value: "selected".to_string(),
                        }],
                    },
                    files: vec![FomodFile {
                        source: "CBBE 4K".to_string(),
                        destination: String::new(),
                        priority: 0,
                    }],
                },
                ConditionalFileInstall {
                    dependencies: PluginDependencies {
                        operator: DependencyOperator::And,
                        flags: vec![FlagDependency {
                            flag: "isUNP".to_string(),
                            value: "selected".to_string(),
                        }],
                    },
                    files: vec![FomodFile {
                        source: "UNP 4K".to_string(),
                        destination: String::new(),
                        priority: 0,
                    }],
                },
            ],
        };

        // Select CBBE (index 0): should get CBBE 4K files, not UNP 4K.
        let cbbe_selected = vec![vec![vec![0usize]]];
        let cbbe_files = resolve_fomod_files(&config, &cbbe_selected);
        let cbbe_sources: Vec<String> = cbbe_files.into_iter().map(|f| f.source).collect();
        assert!(
            cbbe_sources.contains(&"CBBE 4K".to_string()),
            "CBBE 4K should be installed when CBBE is selected"
        );
        assert!(
            !cbbe_sources.contains(&"UNP 4K".to_string()),
            "UNP 4K should NOT be installed when CBBE is selected"
        );

        // Select UNP (index 1): should get UNP 4K files, not CBBE 4K.
        let unp_selected = vec![vec![vec![1usize]]];
        let unp_files = resolve_fomod_files(&config, &unp_selected);
        let unp_sources: Vec<String> = unp_files.into_iter().map(|f| f.source).collect();
        assert!(
            unp_sources.contains(&"UNP 4K".to_string()),
            "UNP 4K should be installed when UNP is selected"
        );
        assert!(
            !unp_sources.contains(&"CBBE 4K".to_string()),
            "CBBE 4K should NOT be installed when UNP is selected"
        );
    }
}
