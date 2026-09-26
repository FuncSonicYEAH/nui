//! Model protocol: host-owned tabular data that `For` nodes iterate
//! (plan §5 宿主互操作). The engine stores models by index; a `For` iterable
//! evaluates to [`Value::Model`] and [`Engine::sync_for_nodes`] reconciles
//! the row elements against the model each frame (pull-based, like every
//! other reactive read).
//!
//! Mutations go through the engine so dependents stay coherent: structural
//! changes ([`Engine::model_push`]/[`Engine::model_remove`]) flag a row
//! sync; field writes ([`Engine::model_set_field`]) invalidate exactly the
//! bindings in the affected row.

use nui_core::Value;

use crate::binding::Engine;
use crate::element::{ElementId, ElementTree, FOR_VALUE_PROPERTY, ForBinding};

/// Handle to a model registered with an [`Engine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelId(pub u32);

/// One model row: `(field, value)` pairs; lookups are by field name.
pub type ModelRow = Vec<(String, Value)>;

/// Read-only view of the tabular data a `For` node can iterate. Custom
/// models implement this and register with the engine; mutation of built-in
/// models flows through the engine's model methods so dependents are
/// invalidated.
pub trait Model: std::fmt::Debug {
    /// Number of rows.
    fn row_count(&self) -> usize;
    /// Reads one field of one row; `None` when the row or field is absent.
    fn field(&self, row: usize, name: &str) -> Option<Value>;
    /// Downcast hook: lets the engine reach the concrete model's mutable
    /// API. Implement as `self`.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// The built-in `Vec`-backed model.
///
/// Mutate through [`Engine::model_push`], [`Engine::model_remove`], and
/// [`Engine::model_set_field`]; direct `&mut` access is deliberately not
/// exposed so no write bypasses dependency invalidation.
#[derive(Debug, Default)]
pub struct VecModel {
    rows: Vec<ModelRow>,
}

impl VecModel {
    /// Creates an empty model.
    pub fn new() -> VecModel {
        return VecModel::default();
    }

    /// Creates a model from initial rows.
    pub fn from_rows(rows: Vec<ModelRow>) -> VecModel {
        return VecModel { rows };
    }

    /// Reads one row; `None` when out of bounds.
    pub fn row(&self, row: usize) -> Option<&ModelRow> {
        return self.rows.get(row);
    }
}

impl Model for VecModel {
    fn row_count(&self) -> usize {
        return self.rows.len();
    }

    fn field(&self, row: usize, name: &str) -> Option<Value> {
        let row_data = self.rows.get(row)?;
        return row_data
            .iter()
            .find(|(field_name, _)| return field_name == name)
            .map(|(_, value)| return value.clone());
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        return self;
    }
}

/// The mutable `VecModel` API, resolved through [`Model::as_any_mut`] so
/// the engine can operate on built-in models without enum dispatch.
pub(crate) fn with_vec_model_mut<R>(
    engine: &mut Engine,
    model: ModelId,
    operate: impl FnOnce(&mut VecModel) -> R,
) -> Option<R> {
    let slot = engine.models.get_mut(model.0 as usize)?;
    let model = slot.as_mut()?;
    let vec_model = model.as_any_mut().downcast_mut::<VecModel>()?;
    return Some(operate(vec_model));
}

impl Engine {
    /// Registers a model and returns its handle. The engine owns the model;
    /// mutate built-ins through the engine's model methods.
    pub fn add_model(&mut self, model: Box<dyn Model>) -> ModelId {
        let index = self.models.len() as u32;
        self.models.push(Some(model));
        return ModelId(index);
    }

    /// Number of rows of a registered model; `0` for unknown handles.
    pub fn model_row_count(&self, model: ModelId) -> usize {
        return self
            .models
            .get(model.0 as usize)
            .and_then(|slot| return slot.as_ref())
            .map(|model| return model.row_count())
            .unwrap_or(0);
    }

    /// Reads one field of one row; `None` for unknown models/rows/fields.
    pub fn model_field(&self, model: ModelId, row: usize, name: &str) -> Option<Value> {
        let model = self.models.get(model.0 as usize)?.as_ref()?;
        return model.field(row, name);
    }

    /// Appends a row (structural change: the next
    /// [`Engine::sync_for_nodes`] rebuilds the `For` rows).
    pub fn model_push(&mut self, model: ModelId, row: ModelRow) -> bool {
        let pushed = with_vec_model_mut(self, model, |vec_model| {
            vec_model.rows.push(row);
        })
        .is_some();
        if pushed {
            self.models_dirty = true;
        }
        return pushed;
    }

    /// Removes one row by index (structural change; see
    /// [`Engine::model_push`]). Returns whether the row existed.
    pub fn model_remove(&mut self, model: ModelId, row: usize) -> bool {
        let removed = with_vec_model_mut(self, model, |vec_model| {
            if row >= vec_model.rows.len() {
                return false;
            }
            vec_model.rows.remove(row);
            return true;
        })
        .unwrap_or(false);
        if removed {
            self.models_dirty = true;
        }
        return removed;
    }

    /// Writes one field of one row and invalidates the bindings of that row
    /// that read the field. Returns whether anything changed.
    pub fn model_set_field(
        &mut self,
        tree: &mut ElementTree,
        model: ModelId,
        row: usize,
        name: &str,
        value: Value,
    ) -> bool {
        let changed = with_vec_model_mut(self, model, |vec_model| {
            let Some(row_data) = vec_model.rows.get_mut(row) else {
                return false;
            };
            let Some(entry) = row_data
                .iter_mut()
                .find(|(field_name, _)| return field_name == name)
            else {
                return false;
            };
            if entry.1 == value {
                return false;
            }
            entry.1 = value;
            return true;
        })
        .unwrap_or(false);
        if !changed {
            return false;
        }
        // Every binding in the row recorded the `(row root, "@field.name")`
        // dependency key; invalidate that key across this model's `For`
        // sites (populated by the last [`Engine::sync_for_nodes`] pass).
        let field_key = format!("@field.{name}");
        let sites = self.model_sites.get(&model.0).cloned().unwrap_or_default();
        return self.invalidate_row_field(tree, &sites, row, &field_key);
    }

    /// Whether model changes await a [`Engine::sync_for_nodes`] pass
    /// (frame-scheduling input alongside dirty bindings).
    pub fn has_pending_model_sync(&self) -> bool {
        return self.models_dirty;
    }

    /// Reconciles every `For` node's rows against its model: removes all row
    /// subtrees and re-instantiates from the prototype when the row count
    /// differs (v1 rebuild semantics, matching the hot-reload approach).
    /// Runs after propagation each frame; returns the number of rebuilt
    /// rows (fresh bindings need a second propagation pass).
    pub fn sync_for_nodes(&mut self, tree: &mut ElementTree) -> usize {
        let mut rebuilt = 0;
        let mut for_elements = Vec::new();
        tree.visit_pre_order(|id, element| {
            if element.for_binding.is_some() {
                for_elements.push(id);
            }
        });
        self.model_sites.clear();
        for id in for_elements {
            let Some(binding) = tree.arena[id].for_binding.clone() else {
                continue;
            };
            let Some(&Value::Model(model_index)) = tree.arena[id].get(FOR_VALUE_PROPERTY) else {
                continue;
            };
            if model_index == Value::UNSET_MODEL {
                continue;
            }
            let model = ModelId(model_index);
            self.model_sites.entry(model.0).or_default().push(id);
            if tree.arena[id].ty == "ListView" {
                rebuilt += self.sync_list_view(tree, id, &binding, model);
                continue;
            }
            let desired = self.model_row_count(model);
            let children = tree.arena[id].children.clone();
            if children.len() == desired {
                continue;
            }
            for child in children {
                let mut removed = Vec::new();
                collect_subtree(tree, child, &mut removed);
                self.retire_bindings(&removed);
                tree.remove_subtree(child);
            }
            for row in 0..desired {
                let _ = instantiate_row(
                    tree,
                    self,
                    id,
                    &binding.variable,
                    model,
                    row,
                    &binding.prototype,
                );
                rebuilt += 1;
            }
        }
        // Drop window bookkeeping of elements gone from the tree.
        self.list_windows.retain(|id, _| {
            return tree.arena.contains_key(*id) && tree.arena[*id].for_binding.is_some();
        });
        self.models_dirty = false;
        return rebuilt;
    }

    /// Virtualized sync for a `ListView`: only the rows inside the visible
    /// window exist as elements (M10). Rows are uniform `row_height` (dp);
    /// a leading spacer element holds the pre-window offset so taffy lays
    /// window rows at their absolute positions.
    fn sync_list_view(
        &mut self,
        tree: &mut ElementTree,
        id: ElementId,
        binding: &ForBinding,
        model: ModelId,
    ) -> usize {
        let element = &tree.arena[id];
        let row_height = element
            .get("row_height")
            .and_then(|value| return dp_value(value))
            .unwrap_or(40.0)
            .max(1.0);
        let viewport_height = element
            .get("height")
            .and_then(|value| return dp_value(value))
            .unwrap_or(0.0);
        let scroll_y = element
            .get("scroll_y")
            .and_then(|value| return dp_value(value))
            .unwrap_or(0.0);
        let total = self.model_row_count(model);

        let first = if row_height > 0.0 {
            // Keep at least one row in the window at max scroll.
            ((scroll_y / row_height).floor() as usize).min(total.saturating_sub(1))
        } else {
            0
        };
        let count = if row_height > 0.0 && viewport_height > 0.0 {
            ((viewport_height / row_height).ceil() as usize + 1).min(total.saturating_sub(first))
        } else {
            total.saturating_sub(first)
        };

        let window_moved = match self.list_windows.get(&id) {
            Some((last_first, last_count)) => *last_first != first || *last_count != count,
            None => true,
        };
        self.list_windows.insert(id, (first, count));
        if !window_moved {
            return 0;
        }

        // Children: [spacer?, row...]. Keep the spacer, rebuild the rows.
        let children = tree.arena[id].children.clone();
        let has_spacer = children
            .first()
            .is_some_and(|child| return tree.arena[*child].ty == "Spacer");
        for child in children.iter().skip(if has_spacer { 1 } else { 0 }) {
            let mut removed = Vec::new();
            collect_subtree(tree, *child, &mut removed);
            self.retire_bindings(&removed);
            tree.remove_subtree(*child);
        }
        let spacer = if has_spacer {
            children[0]
        } else {
            let mut spacer_element = crate::element::Element::new("Spacer", None);
            // A `Spacer` eats free space by default (FUTURE 批次 5), but
            // this one is a fixed pre-window offset: growing it would push
            // the visible rows down whenever the list is taller than its
            // content.
            spacer_element.set("flex_grow", nui_core::Value::Float(0.0));
            let spacer = tree.insert(spacer_element);
            tree.append_child(id, spacer);
            // Move the spacer to the front (it was appended last).
            let list = &mut tree.arena[id].children;
            let position = list.iter().position(|child| return *child == spacer);
            if let Some(position) = position {
                list.remove(position);
                list.insert(0, spacer);
            }
            spacer
        };
        // Pre-window offset: rows laid out after the spacer sit at their
        // absolute positions (row r at y = r * row_height).
        tree.arena[spacer].set(
            "height",
            nui_core::Value::Length(nui_core::Length::Dp(first as f32 * row_height)),
        );

        let mut rebuilt = 0;
        for offset in 0..count {
            let row = first + offset;
            if row >= total {
                break;
            }
            let Some(row_root) = instantiate_row(
                tree,
                self,
                id,
                &binding.variable,
                model,
                row,
                &binding.prototype,
            ) else {
                continue;
            };
            fit_row_to_slot(tree, row_root, row_height);
            rebuilt += 1;
        }
        return rebuilt;
    }

    /// Invalidates `(row root, field_key)` for one row across all `For` /
    /// `ListView` sites of a model. ListView children carry a leading
    /// spacer and hold only the visible window, so the child index is
    /// `row - first + 1` (skipped when the row is outside the window).
    fn invalidate_row_field(
        &mut self,
        tree: &mut ElementTree,
        sites: &[ElementId],
        row: usize,
        field_key: &str,
    ) -> bool {
        let mut any = false;
        for &for_element in sites {
            let virtualized = tree.arena[for_element].ty == "ListView";
            let index = if virtualized {
                match self.list_windows.get(&for_element) {
                    Some((first, count)) => {
                        if row < *first || row >= *first + *count {
                            continue;
                        }
                        row - first + 1
                    }
                    None => continue,
                }
            } else {
                row
            };
            let Some(row_root) = tree.arena[for_element].children.get(index).copied() else {
                continue;
            };
            self.invalidate(row_root, field_key);
            any = true;
        }
        return any;
    }
}

/// Extracts a dp f32 from a property value.
fn dp_value(value: &Value) -> Option<f32> {
    return match value {
        Value::Int(inner) => Some(*inner as f32),
        Value::Float(inner) => Some(*inner as f32),
        Value::Length(nui_core::Length::Dp(inner)) => Some(*inner),
        _ => None,
    };
}

/// Collects a subtree's handles (root included), pre-order.
fn collect_subtree(tree: &ElementTree, id: ElementId, out: &mut Vec<ElementId>) {
    out.push(id);
    for child in tree.arena[id].children.clone() {
        collect_subtree(tree, child, out);
    }
}

/// Clones one row's prototype children under `for_element`, tagging every
/// element of the row subtree with the row's scope.
///
/// Returns the row's root element (the first prototype node's instance),
/// which the `ListView` path sizes to its slot; `None` for a prototype
/// with no nodes.
pub(crate) fn instantiate_row(
    tree: &mut ElementTree,
    engine: &mut Engine,
    for_element: ElementId,
    variable: &str,
    model: ModelId,
    row: usize,
    prototype: &[nui_compiler::NodeIr],
) -> Option<ElementId> {
    let scope = crate::element::RowScope {
        variable: variable.to_string(),
        model,
        row,
    };
    let mut root = None;
    for node in prototype {
        let child =
            crate::instantiate::instantiate_scoped_node(tree, engine, node, Some(scope.clone()));
        tree.append_child(for_element, child);
        root.get_or_insert(child);
    }
    return root;
}

/// Sizes one `ListView` row to its slot, so taffy stacks the window's rows
/// at exactly the positions the virtualizer computed (`row * row_height`).
///
/// The row is a fresh clone of the prototype, so its `height` slot still
/// holds the document's declaration rather than a written-back
/// measurement — which is what makes "did the row declare a height?"
/// answerable here at all.
fn fit_row_to_slot(tree: &mut ElementTree, row_root: ElementId, row_height: f32) {
    let declared = tree.arena[row_root].get("height").and_then(dp_value);
    let slot = crate::widget::row_slot(row_height, declared);
    if let Some(height) = slot.height {
        tree.arena[row_root].set("height", Value::Length(nui_core::Length::Dp(height)));
    }
    if slot.margin_bottom > 0.0 {
        tree.arena[row_root].set(
            "margin_bottom",
            Value::Length(nui_core::Length::Dp(slot.margin_bottom)),
        );
    }
}
