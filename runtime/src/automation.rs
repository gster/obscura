//! Version two automation. A locator is resolved again at every checkpoint.
use super::*;
use obscura_dom::{DomTree, NodeId};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    kind: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    exact: bool,
    #[serde(default)]
    index: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Action {
    operation: String,
    #[serde(default)]
    locator: Vec<Step>,
    #[serde(default)]
    value: String,
    #[serde(default)]
    count: usize,
}
fn normalized(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn matches(actual: &str, wanted: &str, exact: bool) -> bool {
    let a = normalized(actual);
    let b = normalized(wanted);
    if exact {
        a == b
    } else {
        a.to_lowercase().contains(&b.to_lowercase())
    }
}
fn tag(dom: &DomTree, id: NodeId) -> String {
    dom.get_node(id)
        .and_then(|n| n.as_element().map(|n| n.local.to_string()))
        .unwrap_or_default()
}
fn attribute(dom: &DomTree, id: NodeId, name: &str) -> Option<String> {
    dom.get_node(id)
        .and_then(|n| n.get_attribute(name).map(str::to_owned))
}
fn labels(dom: &DomTree, id: NodeId) -> String {
    if !matches!(
        tag(dom, id).as_str(),
        "button" | "input" | "select" | "textarea" | "meter" | "output" | "progress"
    ) {
        return String::new();
    }
    let ident = attribute(dom, id, "id");
    dom.descendants(dom.document())
        .into_iter()
        .filter(|n| {
            tag(dom, *n) == "label"
                && (ident.is_some() && attribute(dom, *n, "for") == ident
                    || dom.ancestors(id).contains(n))
        })
        .map(|n| dom.text_content(n))
        .collect::<Vec<_>>()
        .join(" ")
}
fn name(dom: &DomTree, id: NodeId) -> String {
    if let Some(ids) = attribute(dom, id, "aria-labelledby").as_deref() {
        let values: Vec<_> = ids
            .split_whitespace()
            .filter_map(|s| dom.get_element_by_id(s))
            .map(|n| dom.text_content(n))
            .collect();
        if !values.is_empty() {
            return values.join(" ");
        }
    }
    if let Some(s) = attribute(dom, id, "aria-label").as_deref() {
        if !s.trim().is_empty() {
            return s.into();
        }
    }
    if tag(dom, id) == "fieldset" {
        if let Some(legend) = dom
            .children(id)
            .into_iter()
            .find(|n| tag(dom, *n) == "legend")
        {
            return dom.text_content(legend);
        }
    }
    let label = labels(dom, id);
    if !label.is_empty() {
        return label;
    }
    if tag(dom, id) == "img" {
        return attribute(dom, id, "alt").as_deref().unwrap_or("").into();
    }
    if tag(dom, id) == "input"
        && matches!(
            attribute(dom, id, "type").as_deref(),
            Some("submit" | "button" | "reset")
        )
    {
        return attribute(dom, id, "value").as_deref().unwrap_or("").into();
    }
    let text = dom.text_content(id);
    if !text.trim().is_empty() {
        text
    } else {
        attribute(dom, id, "title").as_deref().unwrap_or("").into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_locator_ignores_empty_aria_label() {
        let dom = obscura_dom::parse_html(r#"<button id="empty" aria-label="">Search flights</button>
            <button id="space" aria-label="   ">Whitespace label</button>
            <button id="named" aria-label="Explicit name">Other text</button>"#);
        for (id, expected) in [("empty", "Search flights"), ("space", "Whitespace label"), ("named", "Explicit name")] {
            let locator = Step { kind: "role".into(), value: "button".into(), name: Some(expected.into()), exact: true, index: 0 };
            assert_eq!(resolve(&dom, &[locator], None).unwrap(), vec![dom.get_element_by_id(id).unwrap()]);
        }
    }
}
fn role(dom: &DomTree, id: NodeId) -> String {
    if let Some(r) = attribute(dom, id, "role").as_deref() {
        return r.split_whitespace().next().unwrap_or("").into();
    }
    match tag(dom, id).as_str() {
        "button" => "button",
        "a" if attribute(dom, id, "href").as_deref().is_some() => "link",
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "heading",
        "img" => "img",
        "fieldset" => "group",
        "option" => "option",
        "select" => "combobox",
        "textarea" => "textbox",
        "input" => match attribute(dom, id, "type").as_deref().unwrap_or("text") {
            "checkbox" => "checkbox",
            "radio" => "radio",
            "button" | "submit" | "reset" => "button",
            "number" => "spinbutton",
            "search" => "searchbox",
            "password" | "hidden" => "",
            _ => "textbox",
        },
        _ => "",
    }
    .into()
}
fn resolve(
    dom: &DomTree,
    steps: &[Step],
    visible: Option<&std::collections::HashSet<NodeId>>,
) -> Result<Vec<NodeId>, Error> {
    if steps.is_empty() || steps.len() > 32 {
        return Err(invalid("INVALID_LOCATOR"));
    }
    let mut roots = vec![dom.document()];
    for step in steps {
        if step.value.len() > 1024 {
            return Err(invalid("INVALID_LOCATOR"));
        }
        if step.kind == "nth" {
            let idx = if step.index < 0 {
                roots.len() as i64 + step.index
            } else {
                step.index
            };
            roots = if idx >= 0 {
                roots.get(idx as usize).copied().into_iter().collect()
            } else {
                vec![]
            };
            continue;
        }
        let mut result = vec![];
        for root in roots {
            let nodes = if step.kind == "css" {
                dom.query_selector_all_from(root, &step.value)
                    .map_err(|_| invalid("INVALID_SELECTOR"))?
            } else {
                dom.descendants(root)
                    .into_iter()
                    .filter(|id| dom.get_node(*id).is_some_and(|n| n.is_element()))
                    .collect()
            };
            for node in nodes {
                let yes = match step.kind.as_str() {
                    "css" => true,
                    "role" => {
                        visible.map_or(true, |nodes| nodes.contains(&node))
                            && role(dom, node) == step.value
                            && step
                                .name
                                .as_ref()
                                .map_or(true, |n| matches(&name(dom, node), n, step.exact))
                            && !std::iter::once(node).chain(dom.ancestors(node)).any(|n| {
                                attribute(dom, n, "aria-hidden").as_deref() == Some("true")
                            })
                    }
                    "label" => {
                        matches(&labels(dom, node), &step.value, step.exact)
                            || attribute(dom, node, "aria-label")
                                .as_deref()
                                .is_some_and(|v| matches(v, &step.value, step.exact))
                            || attribute(dom, node, "aria-labelledby").as_deref().is_some()
                                && matches(&name(dom, node), &step.value, step.exact)
                    }
                    "alt" => attribute(dom, node, "alt")
                        .as_deref()
                        .is_some_and(|v| matches(v, &step.value, step.exact)),
                    "text" => {
                        !matches!(tag(dom, node).as_str(), "script" | "style" | "head")
                            && matches(&dom.text_content(node), &step.value, step.exact)
                            && !dom.children(node).iter().any(|n| {
                                dom.get_node(*n).is_some_and(|n| n.is_element())
                                    && matches(&dom.text_content(*n), &step.value, step.exact)
                            })
                    }
                    _ => return Err(invalid("INVALID_LOCATOR")),
                };
                if yes && !result.contains(&node) {
                    result.push(node);
                }
            }
        }
        roots = result;
    }
    Ok(roots)
}
fn selector(dom: &DomTree, node: NodeId) -> String {
    let mut path = vec![];
    let mut current = node;
    while let Some(parent) = dom.get_node(current).and_then(|n| n.parent) {
        let siblings: Vec<_> = dom
            .children(parent)
            .into_iter()
            .filter(|n| dom.get_node(*n).is_some_and(|n| n.is_element()))
            .collect();
        let index = siblings.iter().position(|n| *n == current).unwrap_or(0) + 1;
        path.push(format!("*:nth-child({index})"));
        if parent == dom.document() {
            break;
        }
        current = parent;
    }
    path.reverse();
    path.join(" > ")
}
fn retryable(code: &str) -> bool {
    matches!(
        code,
        "ELEMENT_NOT_FOUND"
            | "ELEMENT_NOT_VISIBLE"
            | "ELEMENT_DISABLED"
            | "ELEMENT_READONLY"
            | "ELEMENT_OCCLUDED"
            | "OPTION_NOT_FOUND"
    )
}
impl BrowserRuntime {
    pub(super) async fn automation(&mut self, request: &Request) -> Result<Value, Error> {
        let action: Action = params(request)?;
        if action.operation == "close" {
            self.page(request)?;
            self.pages.remove(request.page_id.as_deref().unwrap());
            self.networks.remove(request.page_id.as_deref().unwrap());
            return Ok(json!({}));
        }
        let origins = self.origins.clone();
        let entry = self.page(request)?;
        let deadline = Instant::now() + Duration::from_millis(request.timeout_ms);
        let mut stable = None;
        let mut dispatched = false;
        loop {
            if let Some(url) = entry.navigation_url() {
                let url = Url::parse(&url).map_err(|_| ("INVALID_URL", "SENT"))?;
                if !allowed(&url, &origins) {
                    return Err(("ORIGIN_NOT_ALLOWED", "SENT"));
                }
                entry.generation += 1;
                entry
                    .page
                    .process_pending_navigation()
                    .await
                    .map_err(|_| ("NAVIGATION_FAILED", "SENT"))?;
                entry.url = entry.page.url_string();
                if !allowed(
                    &Url::parse(&entry.url).map_err(|_| ("INVALID_URL", "SENT"))?,
                    &origins,
                ) {
                    return Err(("ORIGIN_NOT_ALLOWED", "SENT"));
                }
                stable = None;
            }
            if Instant::now() >= deadline {
                return Err(("WAIT_TIMEOUT", if dispatched { "SENT" } else { "NOT_SENT" }));
            }
            if action.operation == "url" {
                if entry.url == action.value {
                    return Ok(json!({"value":entry.url,"page_generation":entry.generation}));
                }
            } else {
                let js = entry.page.js.as_mut().ok_or(invalid("NO_DOCUMENT"))?;
                let visible = action
                    .locator
                    .iter()
                    .any(|s| s.kind == "role")
                    .then(|| js.automation_visible_nodes());
                let nodes = js
                    .with_dom(|d| resolve(d, &action.locator, visible.as_ref()))
                    .ok_or(invalid("NO_DOCUMENT"))??;
                if action.operation == "count" {
                    return Ok(json!({"value":nodes.len()}));
                }
                if action.operation == "count_is" && nodes.len() == action.count {
                    return Ok(json!({}));
                }
                if action.operation != "count_is" {
                    if nodes.len() > 1 {
                        return Err(invalid("ELEMENT_AMBIGUOUS"));
                    }
                    let node = nodes.first().copied();
                    let rect = node.and_then(|n| js.automation_box(n));
                    match action.operation.as_str() {
                        "visible_now" => return Ok(json!({"value":rect.is_some()})),
                        "hidden" if rect.is_none() => return Ok(json!({})),
                        "detached" if node.is_none() => return Ok(json!({})),
                        "visible" if rect.is_some() => return Ok(json!({})),
                        "attached" if node.is_some() => return Ok(json!({})),
                        "box" => {
                            return Ok(
                                json!({"value":rect.map(|(x,y,width,height)|json!({"x":x,"y":y,"width":width,"height":height}))}),
                            )
                        }
                        _ => {}
                    }
                    if let Some(node) = node {
                        let css = js.with_dom(|d| selector(d, node)).unwrap();
                        match action.operation.as_str() {
                            "text" | "text_is" | "text_contains" | "value" | "value_is"
                            | "attribute" => {
                                let value = js
                                    .with_dom(|d| match action.operation.as_str() {
                                        "attribute" => attribute(d, node, &action.value)
                                            .as_deref()
                                            .map(str::to_owned),
                                        "value" | "value_is" => {
                                            d.text_control(node).map(|v| v.value).or_else(|| {
                                                attribute(d, node, "value")
                                                    .as_deref()
                                                    .map(str::to_owned)
                                            })
                                        }
                                        _ => Some(d.text_content(node)),
                                    })
                                    .flatten();
                                let satisfied = match action.operation.as_str() {
                                    "text_is" | "value_is" => value.as_ref().is_some_and(|v| {
                                        normalized(v) == normalized(&action.value)
                                    }),
                                    "text_contains" => value.as_ref().is_some_and(|v| {
                                        normalized(v).contains(&normalized(&action.value))
                                    }),
                                    _ => true,
                                };
                                if satisfied {
                                    if matches!(action.operation.as_str(), "text_is" | "text_contains" | "value_is") {
                                        return Ok(json!({}));
                                    }
                                    let result = json!({"value":value});
                                    // Leave room for the RPC envelope and navigation generation.
                                    if serde_json::to_vec(&result).map_err(|_| invalid("VALUE_LIMIT"))?.len() > protocol::LIMIT - 1024 {
                                        return Err(invalid("VALUE_LIMIT"));
                                    }
                                    return Ok(result);
                                }
                            }
                            "click" | "fill" | "check" | "uncheck" | "select" | "scroll"
                                if rect.is_some() =>
                            {
                                let disabled = js
                                    .with_dom(|d| {
                                        d.is_disabled(node)
                                            || d.is_inert(node)
                                            || std::iter::once(node).chain(d.ancestors(node)).any(
                                                |n| {
                                                    attribute(d, n, "aria-disabled").as_deref()
                                                        == Some("true")
                                                },
                                            )
                                    })
                                    .unwrap_or(true);
                                let readonly = js
                                    .with_dom(|d| {
                                        attribute(d, node, "readonly").as_deref().is_some()
                                            || attribute(d, node, "aria-readonly").as_deref()
                                                == Some("true")
                                    })
                                    .unwrap_or(true);
                                if !disabled && !(action.operation == "fill" && readonly) {
                                    let watchdog = js.arm_watchdog(
                                        deadline
                                            .saturating_duration_since(Instant::now()),
                                    );
                                    let scroll = js.automation_scroll(&css);
                                    if js.disarm_watchdog(watchdog) {
                                        return Err(("INPUT_TIMEOUT", "UNKNOWN"));
                                    }
                                    match scroll {
                                        Err(("INPUT_GEOMETRY_UNSUPPORTED", "NOT_SENT")) => {
                                            // A scale/rotation transition may settle on a later frame.
                                            stable = None;
                                        }
                                        Err(error) => return Err(error),
                                        Ok(()) => {
                                            let current = js.automation_box(node);
                                            let settled = stable == Some((node, current));
                                            stable = Some((node, current));
                                            let hit = if matches!(
                                                action.operation.as_str(),
                                                "click" | "check" | "uncheck"
                                            ) {
                                                js.input_target(&css).map(|_| ())
                                            } else {
                                                Ok(())
                                            };
                                            if let Err(e) = hit {
                                                if !retryable(e) {
                                                    return Err(invalid(e));
                                                }
                                            } else if settled
                                                || matches!(action.operation.as_str(), "fill" | "select")
                                            {
                                                let watchdog = js.arm_watchdog(
                                                    deadline
                                                        .saturating_duration_since(Instant::now()),
                                                );
                                                let result = (|| match action.operation.as_str() {
                                                    "scroll" => Ok(()),
                                                    "fill" => {
                                                        js.native_fill(&css, &action.value).map(|_| ())
                                                    }
                                                    "select" => js.automation_select(&css, &action.value),
                                                    "check" | "uncheck" => {
                                                        let checked = js
                                                            .with_dom(|d| {
                                                                d.checked_state(node).map(|s| s.checked)
                                                            })
                                                            .flatten()
                                                            .ok_or(invalid("INPUT_ELEMENT_UNSUPPORTED"))?;
                                                        if checked == (action.operation == "check") {
                                                            Ok(())
                                                        } else {
                                                            js.native_click(&css).map(|_| ())
                                                        }
                                                    }
                                                    _ => js.native_click(&css).map(|_| ()),
                                                })();
                                                if js.disarm_watchdog(watchdog) {
                                                    return Err(("INPUT_TIMEOUT", "UNKNOWN"));
                                                }
                                                match result {
                                                    Ok(()) => {
                                                        dispatched = true;
                                                    }
                                                    Err((code, "NOT_SENT")) if retryable(code) => {}
                                                    Err(e) => return Err(e),
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            "hidden" | "detached" | "visible" | "attached" => {}
                            "click" | "fill" | "check" | "uncheck" | "select" | "scroll" => {}
                            _ => return Err(invalid("UNKNOWN_OPERATION")),
                        }
                    }
                }
            }
            entry.page.advance_automation(deadline.into_std()).await
                .map_err(|_| ("INPUT_TIMEOUT", "UNKNOWN"))?;
            if let Some(url) = entry.navigation_url() {
                let url = Url::parse(&url).map_err(|_| ("INVALID_URL", "SENT"))?;
                if !allowed(&url, &origins) {
                    return Err(("ORIGIN_NOT_ALLOWED", "SENT"));
                }
                entry.generation += 1;
                entry
                    .page
                    .process_pending_navigation()
                    .await
                    .map_err(|_| ("NAVIGATION_FAILED", "SENT"))?;
                entry.url = entry.page.url_string();
                if !allowed(
                    &Url::parse(&entry.url).map_err(|_| ("INVALID_URL", "SENT"))?,
                    &origins,
                ) {
                    return Err(("ORIGIN_NOT_ALLOWED", "SENT"));
                }
                stable = None;
            }
            if dispatched {
                if matches!(action.operation.as_str(), "check" | "uncheck") {
                    let state = entry
                        .page
                        .js
                        .as_ref()
                        .and_then(|js| {
                            let visible = js.automation_visible_nodes();
                            js.with_dom(|d| {
                                resolve(d, &action.locator, Some(&visible))
                                    .ok()
                                    .filter(|v| v.len() == 1)
                                    .and_then(|v| d.checked_state(v[0]).map(|s| s.checked))
                            })
                        })
                        .flatten();
                    if state != Some(action.operation == "check") {
                        return Err(("CHECK_STATE_MISMATCH", "SENT"));
                    }
                }
                return Ok(json!({"page_generation":entry.generation,"url":entry.url}));
            }
            tokio::time::sleep(Duration::from_millis(16)).await;
        }
    }
}
