// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    Document, DragEvent, Element, Event, HtmlElement, HtmlInputElement, HtmlSelectElement,
    KeyboardEvent,
};

use super::logic::AppModel;
use super::render;
use super::state::EntityKind;
use super::workspace::{Workspace, panel_name};
use crate::model::TensorSummary;

const PRESETS: &[(&str, &[&str])] = &[
    (
        "summary",
        &["timetable-summary", "compute-summary", "memory-summary"],
    ),
    (
        "layers",
        &[
            "timetable-summary",
            "layer-summary",
            "layer-details",
            "relationships",
        ],
    ),
    (
        "compute",
        &["compute-summary", "pe-grid", "relationships", "selected-pe"],
    ),
    (
        "memory",
        &[
            "memory-summary",
            "memories-overview",
            "relationships",
            "pe-grid",
            "memory-details",
            "tensor-memory",
            "selected-tensor",
        ],
    ),
    ("tensor", &["tensor-memory", "selected-tensor", "pe-grid"]),
];

thread_local! {
    static APPLICATION: RefCell<Option<Application>> = const { RefCell::new(None) };
}

pub(crate) fn start(model: AppModel, document: Document) -> Result<(), JsValue> {
    let workspace = Workspace::initialize(&document, &model.data.summary.timetable)?;
    let restored = workspace.restored;
    APPLICATION.with(|application| {
        *application.borrow_mut() = Some(Application {
            model,
            document: document.clone(),
            workspace,
            initialized_filters: HashSet::new(),
            dirty_panels: render::ALL_PANELS.iter().copied().collect(),
            global_stats_dirty: true,
            warnings_dirty: true,
            selection_timer: None,
            dragged_panel: None,
        });
    });
    bind_events(&document)?;
    with_application(|application| application.initialize(restored))?;
    Ok(())
}

pub(crate) fn attach_tensors(tensors: Vec<TensorSummary>) -> Result<(), JsValue> {
    with_application(|application| application.attach_tensors(tensors))
}

pub(crate) fn benchmark_kernel(name: &str, iterations: u32) -> Result<f64, JsValue> {
    with_application(|application| {
        let mut checksum = 0.0;
        for _ in 0..iterations {
            application.model.clear_cache();
            checksum += match name {
                "filtering" => {
                    let context = application.model.context(None, None);
                    (context.edges
                        + u64::try_from(context.tensor_indices.len())
                            .expect("Wasm collection lengths fit in u64"))
                        as f64
                }
                "aggregation" => {
                    let summary = application.model.filtered_summary();
                    summary.compute_nodes as f64 + summary.machine_ops.total as f64
                }
                "geometry" => application.model.all_memory_regions().len() as f64,
                _ => {
                    return Err(JsValue::from_str(&format!(
                        "Unknown benchmark kernel '{name}'"
                    )));
                }
            };
        }
        Ok(std::hint::black_box(checksum))
    })
}

struct Application {
    model: AppModel,
    document: Document,
    workspace: Workspace,
    initialized_filters: HashSet<EntityKind>,
    dirty_panels: HashSet<&'static str>,
    global_stats_dirty: bool,
    warnings_dirty: bool,
    selection_timer: Option<i32>,
    dragged_panel: Option<String>,
}

impl Application {
    fn initialize(&mut self, restored: bool) -> Result<(), JsValue> {
        render::initialize_controls(&self.model, &self.document)?;
        self.initialize_filter_statuses()?;
        if restored {
            self.apply_view_config(None)
        } else {
            self.set_preset("summary")
        }
    }

    fn initialize_filter_statuses(&self) -> Result<(), JsValue> {
        for kind in [
            EntityKind::Layer,
            EntityKind::Pe,
            EntityKind::Memory,
            EntityKind::Tensor,
        ] {
            let status = format!("{}-filter-pattern-status", kind.name());
            if let Some(element) = self.document.get_element_by_id(&status) {
                element.set_text_content(Some(&format!(
                    "{} shown",
                    self.model.filter_value_count(kind)
                )));
            }
        }
        Ok(())
    }

    fn attach_tensors(&mut self, tensors: Vec<TensorSummary>) -> Result<(), JsValue> {
        self.model.attach_tensors(tensors);
        self.sync_selections();
        self.update_tensor_filter()?;
        self.mark_all_panels_dirty();
        self.render_dirty()
    }

    fn update_tensor_filter(&self) -> Result<(), JsValue> {
        render::update_filter_summaries(&self.model, &self.document)?;
        if let Some(status) = self
            .document
            .get_element_by_id("tensor-filter-pattern-status")
        {
            status.set_text_content(Some(&format!(
                "{} shown",
                self.model.filter_value_count(EntityKind::Tensor)
            )));
        }
        if self.initialized_filters.contains(&EntityKind::Tensor) {
            render::render_filter_options(&self.model, &self.document, EntityKind::Tensor)?;
        }
        Ok(())
    }

    fn set_preset(&mut self, name: &str) -> Result<(), JsValue> {
        let preset = PRESETS
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .unwrap_or(&PRESETS[1]);
        self.apply_preset_modes(name)?;
        self.reorder_panels(preset.1)?;
        let visible = preset.1.iter().copied().collect::<HashSet<_>>();
        for toggle in elements(&self.document, "[data-view-toggle]")? {
            let checked = toggle
                .get_attribute("data-view-toggle")
                .is_some_and(|value| visible.contains(value.as_str()));
            toggle.dyn_into::<HtmlInputElement>()?.set_checked(checked);
        }
        self.apply_view_config(Some(name))
    }

    fn apply_preset_modes(&mut self, name: &str) -> Result<(), JsValue> {
        let previous_relationship = self.model.state.relationship_mode.clone();
        let previous_pe_measure = self.model.state.pe_measure.clone();
        let previous_pe_mode = self.model.state.pe_mode.clone();
        match name {
            "compute" => {
                self.model.state.relationship_mode = "compute".into();
                self.model.state.pe_measure = "compute:machine-ops".into();
                self.model.set_pe_mode("grid");
            }
            "memory" => {
                self.model.state.relationship_mode = "memory".into();
                self.model.state.pe_measure = "data:total".into();
                self.model.set_pe_mode("chart");
            }
            "tensor" => {
                self.model.state.pe_measure = "tensor:read".into();
                self.model.set_pe_mode("grid");
            }
            _ => return Ok(()),
        }
        select(&self.document, "relationship-mode")?.set_value(&self.model.state.relationship_mode);
        render::update_relationship_measure_options(&self.model, &self.document)?;
        self.model.state.relationship_measure =
            select(&self.document, "relationship-measure")?.value();
        select(&self.document, "pe-overview-measure")?.set_value(&self.model.state.pe_measure);
        if self.model.state.relationship_mode != previous_relationship {
            self.mark_panels_dirty(&["relationships"]);
        }
        if self.model.state.pe_measure != previous_pe_measure
            || self.model.state.pe_mode != previous_pe_mode
        {
            self.mark_panels_dirty(&["pe-grid"]);
        }
        Ok(())
    }

    fn reorder_panels(&self, preferred: &[&str]) -> Result<(), JsValue> {
        let views = element(&self.document, "views")?;
        let panels = elements(&self.document, "#views > [data-view]")?;
        let by_name = panels
            .iter()
            .filter_map(|panel| {
                panel
                    .get_attribute("data-view")
                    .map(|name| (name, panel.clone()))
            })
            .collect::<HashMap<_, _>>();
        for name in preferred {
            if let Some(panel) = by_name.get(*name) {
                views.append_child(panel)?;
            }
        }
        for panel in panels {
            if panel
                .get_attribute("data-view")
                .is_some_and(|name| !preferred.contains(&name.as_str()))
            {
                views.append_child(&panel)?;
            }
        }
        Ok(())
    }

    fn apply_view_config(&mut self, active_preset: Option<&str>) -> Result<(), JsValue> {
        let layout = select(&self.document, "view-layout")?.value();
        let views = element(&self.document, "views")?;
        for name in ["auto", "one", "two", "three"] {
            views.class_list().remove_1(&format!("layout-{name}"))?;
        }
        views.class_list().add_1(&format!("layout-{layout}"))?;
        let visible = elements(&self.document, "[data-view-toggle]")?
            .into_iter()
            .filter_map(|element| element.dyn_into::<HtmlInputElement>().ok())
            .filter(|toggle| toggle.checked())
            .filter_map(|toggle| toggle.get_attribute("data-view-toggle"))
            .collect::<HashSet<_>>();
        self.workspace.reconcile_focus(&self.document, &visible)?;
        for panel in elements(&self.document, "[data-view]")? {
            let hidden = panel
                .get_attribute("data-view")
                .is_none_or(|name| !visible.contains(&name));
            panel.dyn_into::<HtmlElement>()?.set_hidden(hidden);
        }
        for button in elements(&self.document, "[data-preset]")? {
            let pressed = button.get_attribute("data-preset").as_deref() == active_preset;
            button.set_attribute("aria-pressed", if pressed { "true" } else { "false" })?;
        }
        self.sync_selections();
        self.render_dirty()?;
        self.workspace.save(&self.document);
        Ok(())
    }

    fn sync_selections(&mut self) {
        for kind in [EntityKind::Layer, EntityKind::Pe, EntityKind::Memory] {
            let selected = self.model.state.selected(kind).map(str::to_string);
            if selected
                .as_deref()
                .is_none_or(|value| !self.model.state.filter(kind).is_selected(value))
            {
                let replacement = self
                    .model
                    .state
                    .filter(kind)
                    .first_selected()
                    .map(str::to_string);
                match kind {
                    EntityKind::Layer => self.model.state.selected_layer = replacement,
                    EntityKind::Pe => self.model.state.selected_pe = replacement,
                    EntityKind::Memory => self.model.state.selected_memory = replacement,
                    EntityKind::Tensor => unreachable!(),
                }
            }
        }
        let selected_tensor = self.model.state.selected_tensor.clone();
        let visible_tensor = selected_tensor.as_deref().is_some_and(|id| {
            self.model
                .filtered_tensors()
                .iter()
                .any(|tensor| tensor.id == id)
        });
        if !visible_tensor {
            self.model.state.selected_tensor = self
                .model
                .filtered_tensors()
                .first()
                .map(|tensor| tensor.id.clone());
        }
    }

    fn filters_changed(&mut self) -> Result<(), JsValue> {
        self.model.state.filters_changed();
        self.sync_selections();
        self.mark_all_panels_dirty();
        render::update_filter_summaries(&self.model, &self.document)?;
        let initialized = self.initialized_filters.iter().copied().collect::<Vec<_>>();
        for kind in initialized {
            render::render_filter_options(&self.model, &self.document, kind)?;
        }
        self.render_dirty()
    }

    fn selection_changed(&mut self, kind: EntityKind, id: String) -> Result<(), JsValue> {
        self.model.state.select(kind, id);
        self.update_selection_outlines(kind)?;
        self.mark_panels_dirty(selection_dependencies(kind));
        self.render_dirty()
    }

    fn mark_all_panels_dirty(&mut self) {
        self.dirty_panels.extend(render::ALL_PANELS.iter().copied());
        self.global_stats_dirty = true;
    }

    fn mark_panels_dirty(&mut self, panels: &[&'static str]) {
        self.dirty_panels.extend(panels.iter().copied());
    }

    fn render_dirty(&mut self) -> Result<(), JsValue> {
        let panels = render::ALL_PANELS
            .iter()
            .copied()
            .filter(|panel| self.dirty_panels.contains(panel))
            .collect::<Vec<_>>();
        let rendered = render::render_dirty(
            &self.model,
            &self.document,
            &panels,
            self.global_stats_dirty,
            self.warnings_dirty,
        )?;
        for panel in rendered {
            self.dirty_panels.remove(panel);
        }
        self.global_stats_dirty = false;
        self.warnings_dirty = false;
        Ok(())
    }

    fn update_selection_outlines(&self, kind: EntityKind) -> Result<(), JsValue> {
        let selected = self.model.state.selected(kind);
        let selector = format!("[data-selection-kind=\"{}\"]", kind.name());
        for item in elements(&self.document, &selector)? {
            let is_selected = item.get_attribute("data-selection-id").as_deref() == selected;
            item.class_list()
                .toggle_with_force("selected", is_selected)?;
            if item.has_attribute("aria-pressed") {
                item.set_attribute("aria-pressed", if is_selected { "true" } else { "false" })?;
            }
        }
        Ok(())
    }

    fn schedule_selection(&mut self, kind: EntityKind, id: String) -> Result<(), JsValue> {
        self.cancel_selection_timer();
        let callback = Closure::once_into_js(move || {
            if let Err(error) = with_application(|application| {
                application.selection_timer = None;
                application.selection_changed(kind, id)
            }) {
                web_sys::console::error_1(&error);
            }
        });
        let timer = web_sys::window()
            .ok_or_else(|| JsValue::from_str("Missing browser window"))?
            .set_timeout_with_callback_and_timeout_and_arguments_0(callback.unchecked_ref(), 220)?;
        self.selection_timer = Some(timer);
        Ok(())
    }

    fn cancel_selection_timer(&mut self) {
        if let Some(timer) = self.selection_timer.take() {
            if let Some(window) = web_sys::window() {
                window.clear_timeout_with_handle(timer);
            }
        }
    }

    fn select_only(&mut self, kind: EntityKind, id: &str) -> Result<(), JsValue> {
        self.cancel_selection_timer();
        self.model.state.filter_mut(kind).select_only(id);
        self.filters_changed()
    }

    fn initialize_filter(&mut self, kind: EntityKind) -> Result<(), JsValue> {
        self.initialized_filters.insert(kind);
        render::render_filter_options(&self.model, &self.document, kind)
    }

    fn select_pattern_matches(&mut self, kind: EntityKind) -> Result<(), JsValue> {
        if self
            .model
            .state
            .filter_mut(kind)
            .select_pattern_matches()
            .is_ok()
        {
            self.filters_changed()?;
        }
        Ok(())
    }

    fn clear_pattern(&mut self, kind: EntityKind) -> Result<(), JsValue> {
        self.model.state.filter_mut(kind).clear_pattern();
        let pattern = element(&self.document, &format!("{}-filter-pattern", kind.name()))?
            .dyn_into::<HtmlInputElement>()?;
        pattern.set_value("");
        pattern.focus()?;
        self.initialize_filter(kind)
    }

    fn handle_click(&mut self, event: Event) -> Result<(), JsValue> {
        let Some(target) = event_element(&event) else {
            return Ok(());
        };
        if let Some(button) = target.closest("[data-preset]")? {
            return self.set_preset(&button.get_attribute("data-preset").unwrap_or_default());
        }
        if let Some(button) = target.closest("[data-pe-overview-mode]")? {
            let requested = button
                .get_attribute("data-pe-overview-mode")
                .unwrap_or_else(|| "grid".into());
            self.model.set_pe_mode(&requested);
            self.mark_panels_dirty(&["pe-grid"]);
            return self.render_dirty();
        }
        if let Some(button) = target.closest("[data-workspace-action]")? {
            if button.get_attribute("data-workspace-action").as_deref() != Some("drag") {
                return self.workspace_action(&button);
            }
        }
        if target.closest("#workspace-add")?.is_some() {
            self.workspace.add_selected(&self.document)?;
            return self.apply_view_config(None);
        }
        if target.closest("#workspace-reset")?.is_some() {
            self.workspace.reset(&self.document)?;
            return self.set_preset("summary");
        }
        if let Some((kind, action)) = filter_button(&target) {
            match action.as_str() {
                "all" => {
                    self.model.state.filter_mut(kind).select_all(true);
                    return self.filters_changed();
                }
                "none" => {
                    self.model.state.filter_mut(kind).select_all(false);
                    return self.filters_changed();
                }
                "select-matches" => return self.select_pattern_matches(kind),
                "clear-pattern" => return self.clear_pattern(kind),
                _ => {}
            }
        }
        if let Some(entity) = target.closest("[data-select-kind]")? {
            if let Some((kind, id)) = selection(&entity) {
                return self.schedule_selection(kind, id);
            }
        }
        if let Some(entity) = target.closest("[data-relationship-kind]")? {
            if let Some((kind, id)) = relationship_selection(&entity) {
                return self.schedule_selection(kind, id);
            }
        }
        Ok(())
    }

    fn handle_double_click(&mut self, event: Event) -> Result<(), JsValue> {
        let Some(target) = event_element(&event) else {
            return Ok(());
        };
        let entity = target
            .closest("[data-select-kind]")?
            .or(target.closest("[data-relationship-kind]")?);
        if let Some(entity) = entity {
            event.prevent_default();
            if let Some((kind, id)) = selection(&entity).or_else(|| relationship_selection(&entity))
            {
                self.select_only(kind, &id)?;
            }
        }
        Ok(())
    }

    fn handle_change(&mut self, event: Event) -> Result<(), JsValue> {
        let Some(target) = event_element(&event) else {
            return Ok(());
        };
        if let Some(kind) = target
            .get_attribute("data-filter-kind")
            .and_then(|value| EntityKind::parse(&value))
        {
            let input = target.dyn_into::<HtmlInputElement>()?;
            self.model
                .state
                .filter_mut(kind)
                .set_selected(&input.value(), input.checked());
            return self.filters_changed();
        }
        match target.id().as_str() {
            "relationship-mode" => {
                self.model.state.relationship_mode =
                    target.dyn_into::<HtmlSelectElement>()?.value();
                render::update_relationship_measure_options(&self.model, &self.document)?;
                self.model.state.relationship_measure =
                    select(&self.document, "relationship-measure")?.value();
                self.mark_panels_dirty(&["relationships"]);
                self.render_dirty()
            }
            "relationship-measure" => {
                self.model.state.relationship_measure =
                    target.dyn_into::<HtmlSelectElement>()?.value();
                self.mark_panels_dirty(&["relationships"]);
                self.render_dirty()
            }
            "pe-overview-measure" => {
                self.model.state.pe_measure = target.dyn_into::<HtmlSelectElement>()?.value();
                self.mark_panels_dirty(&["pe-grid"]);
                self.render_dirty()
            }
            "skip-memory-gaps" => {
                self.model.state.skip_memory_gaps =
                    target.dyn_into::<HtmlInputElement>()?.checked();
                self.mark_panels_dirty(&["tensor-memory", "memory-details"]);
                self.render_dirty()
            }
            "view-layout" => self.apply_view_config(None),
            _ if target.has_attribute("data-view-toggle") => self.apply_view_config(None),
            _ if target.get_attribute("data-workspace-action").as_deref() == Some("width") => {
                let name = panel_name(&target).unwrap_or_default();
                let width = target.dyn_into::<HtmlSelectElement>()?.value();
                self.workspace.set_width(&self.document, &name, &width)
            }
            _ => Ok(()),
        }
    }

    fn handle_input(&mut self, event: Event) -> Result<(), JsValue> {
        let Some(target) = event_element(&event) else {
            return Ok(());
        };
        if let Some(kind) = pattern_kind(&target) {
            let value = target.dyn_into::<HtmlInputElement>()?.value();
            self.model.state.filter_mut(kind).set_pattern(value);
            return self.initialize_filter(kind);
        }
        if target.id() == "relationship-strength" {
            let input = target.dyn_into::<HtmlInputElement>()?;
            self.model.state.relationship_strength =
                input.value_as_number().round().clamp(0.0, 100.0) as u32;
            self.mark_panels_dirty(&["relationships"]);
            return self.render_dirty();
        }
        Ok(())
    }

    fn handle_keydown(&mut self, event: Event) -> Result<(), JsValue> {
        let keyboard = event.dyn_into::<KeyboardEvent>()?;
        let Some(target) = event_element(keyboard.as_ref()) else {
            return Ok(());
        };
        if keyboard.key() == "Enter" {
            if let Some(kind) = pattern_kind(&target) {
                keyboard.prevent_default();
                return self.select_pattern_matches(kind);
            }
        }
        if matches!(keyboard.key().as_str(), "Enter" | " ") {
            if let Some(entity) = target.closest("[data-relationship-kind]")? {
                keyboard.prevent_default();
                if let Some((kind, id)) = relationship_selection(&entity) {
                    return self.selection_changed(kind, id);
                }
            }
        }
        Ok(())
    }

    fn handle_toggle(&mut self, event: Event) -> Result<(), JsValue> {
        let Some(target) = event_element(&event) else {
            return Ok(());
        };
        if target.tag_name() != "DETAILS" || !target.has_attribute("open") {
            return Ok(());
        }
        if let Some(container) = target.query_selector(".filter-options")? {
            if let Some(kind) = container
                .id()
                .strip_suffix("-filter")
                .and_then(EntityKind::parse)
            {
                return self.initialize_filter(kind);
            }
        }
        Ok(())
    }

    fn workspace_action(&mut self, button: &Element) -> Result<(), JsValue> {
        let action = button
            .get_attribute("data-workspace-action")
            .unwrap_or_default();
        let name = panel_name(button).unwrap_or_default();
        match action.as_str() {
            "up" => self.workspace.move_panel(&self.document, &name, -1),
            "down" => self.workspace.move_panel(&self.document, &name, 1),
            "collapse" => self.workspace.toggle_collapsed(&self.document, &name),
            "focus" => self.workspace.toggle_focus(&self.document, &name),
            "hide" => {
                self.workspace.hide(&self.document, &name)?;
                self.apply_view_config(None)
            }
            _ => Ok(()),
        }
    }

    fn handle_drag_start(&mut self, event: Event) -> Result<(), JsValue> {
        let drag = event.dyn_into::<DragEvent>()?;
        let Some(target) = event_element(drag.as_ref()).and_then(|target| {
            target
                .closest("[data-workspace-action=\"drag\"]")
                .ok()
                .flatten()
        }) else {
            return Ok(());
        };
        let Some(name) = panel_name(&target) else {
            return Ok(());
        };
        self.dragged_panel = Some(name.clone());
        if let Some(panel) = target.closest("[data-view]")? {
            panel.class_list().add_1("workspace-dragging")?;
        }
        if let Some(transfer) = drag.data_transfer() {
            transfer.set_effect_allowed("move");
            transfer.set_data("text/plain", &name)?;
        }
        Ok(())
    }

    fn handle_drag_over(&mut self, event: Event) -> Result<(), JsValue> {
        let Some(dragged_name) = self.dragged_panel.clone() else {
            return Ok(());
        };
        let Some(target) =
            event_element(&event).and_then(|target| target.closest("[data-view]").ok().flatten())
        else {
            return Ok(());
        };
        let Some(target_name) = target.get_attribute("data-view") else {
            return Ok(());
        };
        if target_name == dragged_name {
            return Ok(());
        }
        event.prevent_default();
        let Some(dragged) = self.document.query_selector(&format!(
            "[data-view=\"{}\"]",
            selector_escape(&dragged_name)
        ))?
        else {
            return Ok(());
        };
        let views = element(&self.document, "views")?;
        let children = elements(&self.document, "#views > [data-view]")?;
        let dragged_index = children
            .iter()
            .position(|panel| panel == &dragged)
            .unwrap_or(0);
        let target_index = children
            .iter()
            .position(|panel| panel == &target)
            .unwrap_or(0);
        if dragged_index < target_index {
            views.insert_before(&dragged, target.next_sibling().as_ref())?;
        } else {
            views.insert_before(&dragged, Some(&target))?;
        }
        Ok(())
    }

    fn handle_drag_end(&mut self, event: Event) -> Result<(), JsValue> {
        if let Some(target) =
            event_element(&event).and_then(|target| target.closest("[data-view]").ok().flatten())
        {
            target.class_list().remove_1("workspace-dragging")?;
        }
        self.dragged_panel = None;
        self.workspace.save(&self.document);
        Ok(())
    }
}

fn bind_events(document: &Document) -> Result<(), JsValue> {
    listen(document, "click", false, Application::handle_click)?;
    listen(
        document,
        "dblclick",
        false,
        Application::handle_double_click,
    )?;
    listen(document, "change", false, Application::handle_change)?;
    listen(document, "input", false, Application::handle_input)?;
    listen(document, "keydown", false, Application::handle_keydown)?;
    listen(document, "toggle", true, Application::handle_toggle)?;
    listen(document, "dragstart", false, Application::handle_drag_start)?;
    listen(document, "dragover", false, Application::handle_drag_over)?;
    listen(document, "dragend", false, Application::handle_drag_end)
}

fn listen(
    document: &Document,
    name: &str,
    capture: bool,
    handler: fn(&mut Application, Event) -> Result<(), JsValue>,
) -> Result<(), JsValue> {
    let callback = Closure::<dyn FnMut(Event)>::new(move |event| {
        if let Err(error) = with_application(|application| handler(application, event)) {
            web_sys::console::error_1(&error);
        }
    });
    document.add_event_listener_with_callback_and_bool(
        name,
        callback.as_ref().unchecked_ref(),
        capture,
    )?;
    callback.forget();
    Ok(())
}

fn with_application<T>(
    action: impl FnOnce(&mut Application) -> Result<T, JsValue>,
) -> Result<T, JsValue> {
    APPLICATION.with(|application| {
        let mut application = application.borrow_mut();
        let application = application
            .as_mut()
            .ok_or_else(|| JsValue::from_str("Report application is not initialized"))?;
        action(application)
    })
}

fn filter_button(target: &Element) -> Option<(EntityKind, String)> {
    for kind in [
        EntityKind::Layer,
        EntityKind::Pe,
        EntityKind::Memory,
        EntityKind::Tensor,
    ] {
        for action in ["all", "none", "select-matches", "clear-pattern"] {
            let id = format!("{}-filter-{action}", kind.name());
            if target.closest(&format!("#{id}")).ok().flatten().is_some() {
                return Some((kind, action.into()));
            }
        }
    }
    None
}

fn pattern_kind(target: &Element) -> Option<EntityKind> {
    target
        .id()
        .strip_suffix("-filter-pattern")
        .and_then(EntityKind::parse)
}

fn selection(element: &Element) -> Option<(EntityKind, String)> {
    let kind = element
        .get_attribute("data-select-kind")
        .and_then(|value| EntityKind::parse(&value))?;
    let id = element.get_attribute("data-select-id")?;
    Some((kind, id))
}

fn relationship_selection(element: &Element) -> Option<(EntityKind, String)> {
    let kind = element
        .get_attribute("data-relationship-kind")
        .and_then(|value| EntityKind::parse(&value))?;
    let id = element.get_attribute("data-relationship-id")?;
    Some((kind, id))
}

fn event_element(event: &Event) -> Option<Element> {
    event.target()?.dyn_into().ok()
}

fn elements(document: &Document, selector: &str) -> Result<Vec<Element>, JsValue> {
    let nodes = document.query_selector_all(selector)?;
    Ok((0..nodes.length())
        .filter_map(|index| nodes.item(index))
        .filter_map(|node| node.dyn_into().ok())
        .collect())
}

fn select(document: &Document, id: &str) -> Result<HtmlSelectElement, JsValue> {
    element(document, id)?
        .dyn_into()
        .map_err(|_| JsValue::from_str(&format!("#{id} is not a select")))
}

fn element(document: &Document, id: &str) -> Result<Element, JsValue> {
    document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("Missing report element #{id}")))
}

fn selector_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn selection_dependencies(kind: EntityKind) -> &'static [&'static str] {
    match kind {
        EntityKind::Layer => &["layer-details", "relationships"],
        EntityKind::Pe => &["selected-pe", "relationships"],
        EntityKind::Memory => &["memory-details", "relationships"],
        EntityKind::Tensor => &["selected-tensor", "relationships", "pe-grid"],
    }
}
