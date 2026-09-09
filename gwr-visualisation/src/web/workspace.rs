// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, HashMap, HashSet};

use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    Document, Element, HtmlButtonElement, HtmlElement, HtmlInputElement, HtmlSelectElement,
};

use super::workspace_model::{PanelSnapshot, Snapshot, VERSION, migrate_name, retained_focus};

const DEFAULT_FULL_WIDTH: &[&str] = &["layer-summary", "relationships", "tensor-memory"];

pub(crate) struct Workspace {
    storage_key: String,
    initial_order: Vec<String>,
    focused: Option<String>,
    ready: bool,
    pub(crate) restored: bool,
}

impl Workspace {
    pub(crate) fn initialize(document: &Document, timetable: &str) -> Result<Self, JsValue> {
        let panels = panels(document)?;
        let initial_order = panels.iter().filter_map(view_name).collect::<Vec<_>>();
        for panel in &panels {
            decorate_panel(document, panel)?;
        }
        let mut workspace = Self {
            storage_key: format!("gwr-visualisation-workspace-v{VERSION}:{timetable}"),
            initial_order,
            focused: None,
            ready: false,
            restored: false,
        };
        workspace.restored = workspace
            .read()
            .is_some_and(|snapshot| workspace.restore(document, snapshot).unwrap_or(false));
        workspace.ready = true;
        workspace.update_add_options(document)?;
        Ok(workspace)
    }

    pub(crate) fn save(&self, document: &Document) {
        if !self.ready {
            return;
        }
        let Ok(snapshot) = self.snapshot(document) else {
            return;
        };
        let Ok(serialized) = serde_json::to_string(&snapshot) else {
            return;
        };
        if let Some(storage) = storage() {
            let _ = storage.set_item(&self.storage_key, &serialized);
        }
        let _ = self.update_add_options(document);
    }

    pub(crate) fn reset(&mut self, document: &Document) -> Result<(), JsValue> {
        if let Some(storage) = storage() {
            let _ = storage.remove_item(&self.storage_key);
        }
        self.focused = None;
        let views = element(document, "views")?;
        views.class_list().remove_1("workspace-has-focus")?;
        let by_name = panels(document)?
            .into_iter()
            .filter_map(|panel| view_name(&panel).map(|name| (name, panel)))
            .collect::<HashMap<_, _>>();
        for name in &self.initial_order {
            if let Some(panel) = by_name.get(name) {
                views.append_child(panel)?;
            }
        }
        for panel in by_name.values() {
            html_element(panel)?.style().set_property("height", "")?;
            panel.class_list().remove_1("workspace-focused")?;
            set_width(panel, default_width(view_name(panel).as_deref()))?;
            set_collapsed(panel, false)?;
        }
        select(document, "view-layout")?.set_value("one");
        self.update_focus_buttons(document)?;
        Ok(())
    }

    pub(crate) fn add_selected(&self, document: &Document) -> Result<(), JsValue> {
        let name = select(document, "workspace-add-view")?.value();
        if name.is_empty() {
            return Ok(());
        }
        set_toggle(document, &name, true)
    }

    pub(crate) fn move_panel(
        &self,
        document: &Document,
        panel_name: &str,
        direction: isize,
    ) -> Result<(), JsValue> {
        let visible = visible_panels(document)?;
        let Some(index) = visible
            .iter()
            .position(|panel| view_name(panel).as_deref() == Some(panel_name))
        else {
            return Ok(());
        };
        let target_index = index as isize + direction;
        if target_index < 0 || target_index >= visible.len() as isize {
            return Ok(());
        }
        let views = element(document, "views")?;
        let panel = &visible[index];
        let target = &visible[target_index as usize];
        if direction < 0 {
            views.insert_before(panel, Some(target))?;
        } else {
            views.insert_before(panel, target.next_sibling().as_ref())?;
        }
        self.save(document);
        Ok(())
    }

    pub(crate) fn set_width(
        &self,
        document: &Document,
        panel_name: &str,
        width: &str,
    ) -> Result<(), JsValue> {
        if let Some(panel) = panel(document, panel_name)? {
            set_width(&panel, width)?;
            self.save(document);
        }
        Ok(())
    }

    pub(crate) fn toggle_collapsed(
        &self,
        document: &Document,
        panel_name: &str,
    ) -> Result<(), JsValue> {
        if let Some(panel) = panel(document, panel_name)? {
            let collapsed = !panel.class_list().contains("workspace-collapsed");
            set_collapsed(&panel, collapsed)?;
            self.save(document);
        }
        Ok(())
    }

    pub(crate) fn toggle_focus(
        &mut self,
        document: &Document,
        panel_name: &str,
    ) -> Result<(), JsValue> {
        self.focused =
            (self.focused.as_deref() != Some(panel_name)).then(|| panel_name.to_string());
        element(document, "views")?
            .class_list()
            .toggle_with_force("workspace-has-focus", self.focused.is_some())?;
        for panel in panels(document)? {
            let focused = view_name(&panel).as_deref() == self.focused.as_deref();
            panel
                .class_list()
                .toggle_with_force("workspace-focused", focused)?;
        }
        self.update_focus_buttons(document)
    }

    pub(crate) fn hide(&mut self, document: &Document, panel_name: &str) -> Result<(), JsValue> {
        if self.focused.as_deref() == Some(panel_name) {
            self.toggle_focus(document, panel_name)?;
        }
        set_toggle(document, panel_name, false)
    }

    pub(crate) fn reconcile_focus(
        &mut self,
        document: &Document,
        visible: &HashSet<String>,
    ) -> Result<(), JsValue> {
        let hidden_focus = retained_focus(self.focused.as_deref(), visible)
            .is_none()
            .then(|| self.focused.clone())
            .flatten();
        if let Some(focused) = hidden_focus {
            self.toggle_focus(document, &focused)?;
        }
        Ok(())
    }

    pub(crate) fn update_add_options(&self, document: &Document) -> Result<(), JsValue> {
        let hidden = panels(document)?
            .into_iter()
            .filter(|panel| view_name(panel).is_some_and(|name| !toggle_checked(document, &name)))
            .collect::<Vec<_>>();
        let options: String = if hidden.is_empty() {
            "<option value=\"\">All panels visible</option>".into()
        } else {
            hidden
                .iter()
                .map(|panel| {
                    let name = view_name(panel).unwrap_or_default();
                    let label = panel
                        .get_attribute("data-workspace-label")
                        .unwrap_or_else(|| name.clone());
                    format!(
                        "<option value=\"{}\">{}</option>",
                        html_escape(&name),
                        html_escape(&label)
                    )
                })
                .collect::<String>()
        };
        element(document, "workspace-add-view")?.set_inner_html(&options);
        element(document, "workspace-add")?
            .dyn_into::<HtmlButtonElement>()?
            .set_disabled(hidden.is_empty());
        Ok(())
    }

    fn snapshot(&self, document: &Document) -> Result<Snapshot, JsValue> {
        let panel_values = panels(document)?;
        let order = panel_values.iter().filter_map(view_name).collect();
        let visible = toggles(document)?
            .into_iter()
            .filter(|toggle| toggle.checked())
            .filter_map(|toggle| toggle.get_attribute("data-view-toggle"))
            .collect();
        let mut panel_snapshots = BTreeMap::new();
        for panel in &panel_values {
            let Some(name) = view_name(panel) else {
                continue;
            };
            let width = panel
                .get_attribute("data-workspace-width")
                .unwrap_or_else(|| "1".into());
            let height = html_element(panel)?
                .style()
                .get_property_value("height")
                .ok()
                .filter(|value| !value.is_empty());
            panel_snapshots.insert(
                name,
                PanelSnapshot {
                    width,
                    height,
                    collapsed: panel.class_list().contains("workspace-collapsed"),
                },
            );
        }
        Ok(Snapshot {
            version: VERSION,
            layout: select(document, "view-layout")?.value(),
            visible,
            order,
            panels: panel_snapshots,
        })
    }

    fn read(&self) -> Option<Snapshot> {
        let value = storage()?.get_item(&self.storage_key).ok().flatten()?;
        Snapshot::parse(&value)
    }

    fn restore(&self, document: &Document, snapshot: Snapshot) -> Result<bool, JsValue> {
        let legacy_memory = !snapshot.panels.contains_key("memories-overview");
        let views = element(document, "views")?;
        let mut remaining = panels(document)?
            .into_iter()
            .filter_map(|panel| view_name(&panel).map(|name| (name, panel)))
            .collect::<BTreeMap<_, _>>();
        for saved in &snapshot.order {
            let name = migrate_name(saved);
            if let Some(panel) = remaining.remove(name) {
                views.append_child(&panel)?;
                if legacy_memory && name == "memory-summary" {
                    if let Some(overview) = remaining.remove("memories-overview") {
                        views.append_child(&overview)?;
                    }
                }
            }
        }
        for panel in remaining.values() {
            views.append_child(panel)?;
        }
        let visible = snapshot.migrated_visible();
        for toggle in toggles(document)? {
            let checked = toggle
                .get_attribute("data-view-toggle")
                .is_some_and(|name| visible.contains(&name));
            toggle.set_checked(checked);
        }
        if matches!(snapshot.layout.as_str(), "auto" | "one" | "two" | "three") {
            select(document, "view-layout")?.set_value(&snapshot.layout);
        }
        for panel in panels(document)? {
            let Some(name) = view_name(&panel) else {
                continue;
            };
            let config = snapshot.panel_config(&name);
            set_width(&panel, config.map_or("1", |value| value.width.as_str()))?;
            html_element(&panel)?.style().set_property(
                "height",
                config
                    .and_then(|value| value.height.as_deref())
                    .unwrap_or(""),
            )?;
            set_collapsed(&panel, config.is_some_and(|value| value.collapsed))?;
        }
        Ok(true)
    }

    fn update_focus_buttons(&self, document: &Document) -> Result<(), JsValue> {
        for panel in panels(document)? {
            let active = view_name(&panel).as_deref() == self.focused.as_deref();
            if let Some(button) = panel.query_selector(".workspace-focus")? {
                button.set_text_content(Some(if active { "Exit focus" } else { "Focus" }));
                button.set_attribute("aria-pressed", if active { "true" } else { "false" })?;
            }
        }
        Ok(())
    }
}

pub(crate) fn panel_name(element: &Element) -> Option<String> {
    element
        .closest("[data-view]")
        .ok()
        .flatten()
        .and_then(|panel| view_name(&panel))
}

fn decorate_panel(document: &Document, panel: &Element) -> Result<(), JsValue> {
    let heading = panel.query_selector(":scope > h2, :scope > .panel-title-row h2")?;
    let label = heading
        .as_ref()
        .and_then(|element| element.text_content())
        .map(|value| value.trim().to_string())
        .or_else(|| view_name(panel))
        .unwrap_or_default();
    panel.set_attribute("data-workspace-label", &label)?;
    let bar = document.create_element("div")?;
    bar.set_class_name("workspace-panel-bar");
    if let Some(heading) = heading {
        bar.append_child(&heading)?;
    } else {
        bar.set_inner_html(&format!("<h2>{}</h2>", html_escape(&label)));
    }
    let tools = document.create_element("div")?;
    tools.set_class_name("workspace-panel-tools");
    tools.set_inner_html(&format!(
        "<button type=\"button\" class=\"workspace-drag\" draggable=\"true\" data-workspace-action=\"drag\" aria-label=\"Drag {}\">Move</button><button type=\"button\" data-workspace-action=\"up\" aria-label=\"Move {} earlier\">Up</button><button type=\"button\" data-workspace-action=\"down\" aria-label=\"Move {} later\">Down</button><select class=\"workspace-panel-width\" data-workspace-action=\"width\" aria-label=\"{} width\"><option value=\"1\">1 column</option><option value=\"2\">2 columns</option><option value=\"full\">Full row</option></select><button type=\"button\" class=\"workspace-collapse\" data-workspace-action=\"collapse\"></button><button type=\"button\" class=\"workspace-focus\" data-workspace-action=\"focus\" aria-pressed=\"false\">Focus</button><button type=\"button\" data-workspace-action=\"hide\" aria-label=\"Hide {}\">Hide</button>",
        html_escape(&label), html_escape(&label), html_escape(&label), html_escape(&label), html_escape(&label),
    ));
    bar.append_child(&tools)?;
    panel.insert_before(&bar, panel.first_child().as_ref())?;
    set_width(panel, default_width(view_name(panel).as_deref()))?;
    set_collapsed(panel, false)
}

fn set_width(panel: &Element, width: &str) -> Result<(), JsValue> {
    let width = if matches!(width, "1" | "2" | "full") {
        width
    } else {
        "1"
    };
    panel.set_attribute("data-workspace-width", width)?;
    html_element(panel)?.style().set_property(
        "grid-column",
        if width == "full" {
            "1 / -1"
        } else if width == "2" {
            "span 2"
        } else {
            "span 1"
        },
    )?;
    if let Some(select) = panel.query_selector(".workspace-panel-width")? {
        select.dyn_into::<HtmlSelectElement>()?.set_value(width);
    }
    Ok(())
}

fn set_collapsed(panel: &Element, collapsed: bool) -> Result<(), JsValue> {
    panel
        .class_list()
        .toggle_with_force("workspace-collapsed", collapsed)?;
    if let Some(button) = panel.query_selector(".workspace-collapse")? {
        button.set_text_content(Some(if collapsed { "Expand" } else { "Collapse" }));
        button.set_attribute("aria-expanded", if collapsed { "false" } else { "true" })?;
    }
    Ok(())
}

fn default_width(name: Option<&str>) -> &'static str {
    if name.is_some_and(|name| DEFAULT_FULL_WIDTH.contains(&name)) {
        "full"
    } else {
        "1"
    }
}

fn panel(document: &Document, name: &str) -> Result<Option<Element>, JsValue> {
    document.query_selector(&format!("[data-view=\"{}\"]", selector_escape(name)))
}

fn panels(document: &Document) -> Result<Vec<Element>, JsValue> {
    elements(document, "[data-view]")
}

fn visible_panels(document: &Document) -> Result<Vec<Element>, JsValue> {
    let mut visible = Vec::new();
    for panel in panels(document)? {
        if !html_element(&panel)?.hidden() {
            visible.push(panel);
        }
    }
    Ok(visible)
}

fn html_element(element: &Element) -> Result<&HtmlElement, JsValue> {
    element
        .dyn_ref()
        .ok_or_else(|| JsValue::from_str("Workspace panel is not an HTML element"))
}

fn toggles(document: &Document) -> Result<Vec<HtmlInputElement>, JsValue> {
    elements(document, "[data-view-toggle]")?
        .into_iter()
        .map(|element| {
            element
                .dyn_into()
                .map_err(|_| JsValue::from_str("View toggle is not an input"))
        })
        .collect()
}

fn elements(document: &Document, selector: &str) -> Result<Vec<Element>, JsValue> {
    let nodes = document.query_selector_all(selector)?;
    Ok((0..nodes.length())
        .filter_map(|index| nodes.item(index))
        .filter_map(|node| node.dyn_into().ok())
        .collect())
}

fn view_name(panel: &Element) -> Option<String> {
    panel.get_attribute("data-view")
}

fn set_toggle(document: &Document, name: &str, checked: bool) -> Result<(), JsValue> {
    for toggle in toggles(document)? {
        if toggle.get_attribute("data-view-toggle").as_deref() == Some(name) {
            toggle.set_checked(checked);
        }
    }
    Ok(())
}

fn toggle_checked(document: &Document, name: &str) -> bool {
    toggles(document).ok().into_iter().flatten().any(|toggle| {
        toggle.get_attribute("data-view-toggle").as_deref() == Some(name) && toggle.checked()
    })
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

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn selector_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
