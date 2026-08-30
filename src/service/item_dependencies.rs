use crate::domain::item::{Item, ItemKind};
use crate::service::error::ItemError;
use crate::storage::sqlite::{ItemDependencyRepo, ItemRepo};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// "Depends on" (docs/issues_and_features.md): validates and persists the full set of items
/// `item` depends on, replacing whatever was there before — called only from
/// `project_items::update_project_item`, after the item's own field update has already
/// persisted, mirroring `reminders::sync_item_reminders`'s post-update side-effect shape.
///
/// Scope is deliberately narrow (per the feature's own scoping discussion): only Task items
/// can participate, on either side, and only as a *sibling* — same project, same
/// `parent_item_id` (including both top-level, i.e. no parent at all). This keeps the
/// completion guard (`has_incomplete_dependencies` below) a same-project lookup with no
/// cross-project permission check of its own, and keeps the web UI's picker a plain list of
/// already-loaded siblings rather than a project-wide search.
pub async fn set_item_dependencies(
    dep_repo: &Arc<dyn ItemDependencyRepo>,
    items: &Arc<dyn ItemRepo>,
    project_id: &str,
    item: &Item,
    depends_on_item_ids: &[String],
) -> Result<(), ItemError> {
    // Clearing is always allowed, regardless of item kind — there's nothing to validate
    // about an empty set, and this is the only way a non-Task item's (accidentally
    // populated, e.g. via a since-changed itemType) dependency rows could ever be cleared.
    if depends_on_item_ids.is_empty() {
        dep_repo.set_dependencies(&item.id, &[]).await?;
        return Ok(());
    }
    if item.kind() != ItemKind::Task {
        return Err(ItemError::Invalid(
            "only Task items can depend on other items".to_string(),
        ));
    }

    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    for dep_id in depends_on_item_ids {
        if dep_id == &item.id {
            return Err(ItemError::Invalid(
                "an item cannot depend on itself".to_string(),
            ));
        }
        if !seen.insert(dep_id.clone()) {
            continue;
        }
        let dep = items.get_by_project(project_id, dep_id).await?;
        if dep.kind() != ItemKind::Task {
            return Err(ItemError::Invalid(format!(
                "{dep_id} is not a Task item, so it can't be a dependency"
            )));
        }
        if dep.parent_item_id != item.parent_item_id {
            return Err(ItemError::Invalid(format!(
                "{dep_id} is not a sibling of this item"
            )));
        }
        ids.push(dep_id.clone());
    }

    let graph = build_dependency_graph(dep_repo, &item.id, &ids).await?;
    validate_dependency_graph(&graph, &item.id)?;

    dep_repo.set_dependencies(&item.id, &ids).await?;
    Ok(())
}

/// Fetches the full `depends_on` subgraph reachable from `item_id`, treating `item_id`'s own
/// edges as `new_deps` (its not-yet-persisted replacement set) rather than whatever is
/// currently stored for it. Siblings-only scoping (see `set_item_dependencies`'s doc comment)
/// keeps this graph small — bounded by one sibling group's size — so fetching it in full before
/// analyzing it is cheap; the visited set also protects against looping forever if bad data
/// ever put an existing cycle in the table some other way.
async fn build_dependency_graph(
    dep_repo: &Arc<dyn ItemDependencyRepo>,
    item_id: &str,
    new_deps: &[String],
) -> Result<HashMap<String, Vec<String>>, ItemError> {
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    graph.insert(item_id.to_string(), new_deps.to_vec());
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(item_id.to_string());
    let mut stack: Vec<String> = new_deps.to_vec();
    while let Some(current) = stack.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }
        let deps = dep_repo.list_for_item(&current).await?;
        stack.extend(deps.clone());
        graph.insert(current, deps);
    }
    Ok(graph)
}

/// Maximum number of edges allowed in a single dependency chain — a safety cap (not a
/// meaningful business rule) to keep the sibling-scoped dependency graph (see
/// `set_item_dependencies`'s doc comment) from growing unboundedly deep.
const MAX_DEPENDENCY_CHAIN_LENGTH: usize = 50;

/// Rejects `graph` (as fetched by `build_dependency_graph`) if it contains a cycle back to
/// `item_id`, or if the longest chain starting at `item_id` would exceed
/// `MAX_DEPENDENCY_CHAIN_LENGTH` edges. Cycle detection and chain-length measurement are the
/// same graph walk — a standard DAG longest-path DFS with a "currently visiting" set doubles as
/// cycle detection, since re-entering a node still on the current path is exactly what a cycle
/// looks like.
fn validate_dependency_graph(
    graph: &HashMap<String, Vec<String>>,
    item_id: &str,
) -> Result<(), ItemError> {
    fn longest_chain(
        node: &str,
        graph: &HashMap<String, Vec<String>>,
        memo: &mut HashMap<String, usize>,
        visiting: &mut HashSet<String>,
    ) -> Result<usize, ItemError> {
        if let Some(&len) = memo.get(node) {
            return Ok(len);
        }
        if !visiting.insert(node.to_string()) {
            return Err(ItemError::Invalid(
                "this dependency would create a cycle".to_string(),
            ));
        }
        let mut max_len = 0;
        for child in graph.get(node).map(|v| v.as_slice()).unwrap_or(&[]) {
            max_len = max_len.max(1 + longest_chain(child, graph, memo, visiting)?);
        }
        visiting.remove(node);
        memo.insert(node.to_string(), max_len);
        Ok(max_len)
    }

    let mut memo = HashMap::new();
    let mut visiting = HashSet::new();
    let chain_length = longest_chain(item_id, graph, &mut memo, &mut visiting)?;
    if chain_length > MAX_DEPENDENCY_CHAIN_LENGTH {
        return Err(ItemError::Invalid(format!(
            "dependency chain would exceed the maximum length of {MAX_DEPENDENCY_CHAIN_LENGTH}"
        )));
    }
    Ok(())
}

/// True if `item_id` has at least one dependency that isn't complete — the "depends on"
/// counterpart to `service::items::has_incomplete_children`, used the same way: checked only
/// on a fresh incomplete->complete transition, by the caller (`project_items::
/// update_project_item`), not on every update.
pub(crate) async fn has_incomplete_dependencies(
    dep_repo: &Arc<dyn ItemDependencyRepo>,
    items: &Arc<dyn ItemRepo>,
    project_id: &str,
    item_id: &str,
) -> Result<bool, ItemError> {
    for dep_id in dep_repo.list_for_item(item_id).await? {
        if !items.get_by_project(project_id, &dep_id).await?.complete() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Resolves `item_id`'s dependency ids into full `Item`s — for display (name, completion
/// state, a link to each) on the read-only detail view and the edit form's picker.
pub async fn list_item_dependencies(
    dep_repo: &Arc<dyn ItemDependencyRepo>,
    items: &Arc<dyn ItemRepo>,
    project_id: &str,
    item_id: &str,
) -> Result<Vec<Item>, ItemError> {
    let mut resolved = Vec::new();
    for dep_id in dep_repo.list_for_item(item_id).await? {
        resolved.push(items.get_by_project(project_id, &dep_id).await?);
    }
    Ok(resolved)
}

/// Rejects moving `item_id` (reparenting it via the Tasks screen's Move dialog) while it
/// participates in a dependency edge, in either direction. A dependency requires both sides to
/// be siblings (see `set_item_dependencies`'s doc comment) — a move changes `parent_item_id`,
/// which would silently strand the edge outside that invariant. Rather than auto-clear the
/// edge, the move itself is rejected; the user removes the dependency first if they still want
/// to move the item.
pub async fn assert_movable(
    dep_repo: &Arc<dyn ItemDependencyRepo>,
    item_id: &str,
) -> Result<(), ItemError> {
    if !dep_repo.list_for_item(item_id).await?.is_empty() {
        return Err(ItemError::Invalid(
            "cannot move an item that depends on another item — remove the dependency first"
                .to_string(),
        ));
    }
    if !dep_repo.list_dependents(item_id).await?.is_empty() {
        return Err(ItemError::Invalid(
            "cannot move an item that another item depends on — remove that dependency first"
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::item::{ItemType, SimpleItem};
    use crate::storage::sqlite::{MockItemDependencyRepo, MockItemRepo};

    fn task(id: &str, project_id: &str, parent_item_id: Option<&str>) -> Item {
        let mut item = Item::new_user_item("user1", "task");
        item.id = id.to_string();
        item.project_id = Some(project_id.to_string());
        item.parent_item_id = parent_item_id.map(|s| s.to_string());
        item
    }

    #[tokio::test]
    async fn rejects_self_dependency() {
        let dep_repo = Arc::new(MockItemDependencyRepo::new()) as Arc<dyn ItemDependencyRepo>;
        let items = Arc::new(MockItemRepo::new()) as Arc<dyn ItemRepo>;
        let item = task("i1", "p1", None);

        let err = set_item_dependencies(&dep_repo, &items, "p1", &item, &["i1".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn rejects_non_task_dependent() {
        let dep_repo = Arc::new(MockItemDependencyRepo::new()) as Arc<dyn ItemDependencyRepo>;
        let items = Arc::new(MockItemRepo::new()) as Arc<dyn ItemRepo>;
        let mut item = task("i1", "p1", None);
        item.item_type = ItemType::Simple(SimpleItem);

        let err = set_item_dependencies(&dep_repo, &items, "p1", &item, &["i2".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn rejects_non_sibling_dependency() {
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .withf(|_, id| id == "i2")
            .returning(|_, _| Ok(task("i2", "p1", Some("other-parent"))));
        let dep_repo = Arc::new(MockItemDependencyRepo::new()) as Arc<dyn ItemDependencyRepo>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;
        let item = task("i1", "p1", None);

        let err = set_item_dependencies(&dep_repo, &items, "p1", &item, &["i2".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn accepts_a_valid_sibling_dependency() {
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .withf(|_, id| id == "i2")
            .returning(|_, _| Ok(task("i2", "p1", None)));
        let mut dep_repo_mock = MockItemDependencyRepo::new();
        dep_repo_mock
            .expect_list_for_item()
            .returning(|_| Ok(vec![]));
        dep_repo_mock
            .expect_set_dependencies()
            .withf(|id, deps| id == "i1" && deps == ["i2".to_string()])
            .returning(|_, _| Ok(()));
        let dep_repo = Arc::new(dep_repo_mock) as Arc<dyn ItemDependencyRepo>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;
        let item = task("i1", "p1", None);

        set_item_dependencies(&dep_repo, &items, "p1", &item, &["i2".to_string()])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn rejects_a_dependency_cycle() {
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .withf(|_, id| id == "i2")
            .returning(|_, _| Ok(task("i2", "p1", None)));
        let mut dep_repo_mock = MockItemDependencyRepo::new();
        // i2 already depends on i1 — so i1 depending on i2 would close a 2-cycle.
        dep_repo_mock
            .expect_list_for_item()
            .withf(|id| id == "i2")
            .returning(|_| Ok(vec!["i1".to_string()]));
        let dep_repo = Arc::new(dep_repo_mock) as Arc<dyn ItemDependencyRepo>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;
        let item = task("i1", "p1", None);

        let err = set_item_dependencies(&dep_repo, &items, "p1", &item, &["i2".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    /// Wires a mock `ItemDependencyRepo` where `iN` depends on `i(N+1)` for every `N` in
    /// `2..=chain_end`, and `i(chain_end)` has no further dependencies — a straight-line chain
    /// of `chain_end - 1` edges hanging off `i2`.
    fn linear_chain_dep_repo(chain_end: usize) -> MockItemDependencyRepo {
        let mut dep_repo_mock = MockItemDependencyRepo::new();
        dep_repo_mock.expect_list_for_item().returning(move |id| {
            let n: usize = id[1..].parse().unwrap();
            if n < chain_end {
                Ok(vec![format!("i{}", n + 1)])
            } else {
                Ok(vec![])
            }
        });
        dep_repo_mock
    }

    #[tokio::test]
    async fn rejects_a_dependency_chain_longer_than_the_max() {
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, id| Ok(task(id, "p1", None)));
        // i1 -> i2 -> i3 -> ... -> i52 is 51 edges, one more than MAX_DEPENDENCY_CHAIN_LENGTH.
        let dep_repo = Arc::new(linear_chain_dep_repo(52)) as Arc<dyn ItemDependencyRepo>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;
        let item = task("i1", "p1", None);

        let err = set_item_dependencies(&dep_repo, &items, "p1", &item, &["i2".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn accepts_a_dependency_chain_at_exactly_the_max() {
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, id| Ok(task(id, "p1", None)));
        // i1 -> i2 -> i3 -> ... -> i51 is exactly 50 edges, the max allowed.
        let mut dep_repo_mock = linear_chain_dep_repo(51);
        dep_repo_mock
            .expect_set_dependencies()
            .returning(|_, _| Ok(()));
        let dep_repo = Arc::new(dep_repo_mock) as Arc<dyn ItemDependencyRepo>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;
        let item = task("i1", "p1", None);

        set_item_dependencies(&dep_repo, &items, "p1", &item, &["i2".to_string()])
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn has_incomplete_dependencies_true_when_any_dependency_is_incomplete() {
        let mut dep_repo_mock = MockItemDependencyRepo::new();
        dep_repo_mock
            .expect_list_for_item()
            .returning(|_| Ok(vec!["i2".to_string()]));
        let mut items_mock = MockItemRepo::new();
        items_mock
            .expect_get_by_project()
            .returning(|_, _| Ok(task("i2", "p1", None)));
        let dep_repo = Arc::new(dep_repo_mock) as Arc<dyn ItemDependencyRepo>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;

        assert!(
            has_incomplete_dependencies(&dep_repo, &items, "p1", "i1")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn has_incomplete_dependencies_false_when_every_dependency_is_complete() {
        let mut dep_repo_mock = MockItemDependencyRepo::new();
        dep_repo_mock
            .expect_list_for_item()
            .returning(|_| Ok(vec!["i2".to_string()]));
        let mut items_mock = MockItemRepo::new();
        items_mock.expect_get_by_project().returning(|_, _| {
            let mut dep = task("i2", "p1", None);
            if let ItemType::Task(t) = &mut dep.item_type {
                t.complete = true;
            }
            Ok(dep)
        });
        let dep_repo = Arc::new(dep_repo_mock) as Arc<dyn ItemDependencyRepo>;
        let items = Arc::new(items_mock) as Arc<dyn ItemRepo>;

        assert!(
            !has_incomplete_dependencies(&dep_repo, &items, "p1", "i1")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn assert_movable_rejects_an_item_with_an_outgoing_dependency() {
        let mut dep_repo_mock = MockItemDependencyRepo::new();
        dep_repo_mock
            .expect_list_for_item()
            .returning(|_| Ok(vec!["i2".to_string()]));
        let dep_repo = Arc::new(dep_repo_mock) as Arc<dyn ItemDependencyRepo>;

        let err = assert_movable(&dep_repo, "i1").await.unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn assert_movable_rejects_an_item_with_a_dependent() {
        let mut dep_repo_mock = MockItemDependencyRepo::new();
        dep_repo_mock
            .expect_list_for_item()
            .returning(|_| Ok(vec![]));
        dep_repo_mock
            .expect_list_dependents()
            .returning(|_| Ok(vec!["i3".to_string()]));
        let dep_repo = Arc::new(dep_repo_mock) as Arc<dyn ItemDependencyRepo>;

        let err = assert_movable(&dep_repo, "i1").await.unwrap_err();
        assert!(matches!(err, ItemError::Invalid(_)));
    }

    #[tokio::test]
    async fn assert_movable_allows_an_item_with_no_dependency_edges() {
        let mut dep_repo_mock = MockItemDependencyRepo::new();
        dep_repo_mock
            .expect_list_for_item()
            .returning(|_| Ok(vec![]));
        dep_repo_mock
            .expect_list_dependents()
            .returning(|_| Ok(vec![]));
        let dep_repo = Arc::new(dep_repo_mock) as Arc<dyn ItemDependencyRepo>;

        assert_movable(&dep_repo, "i1").await.unwrap();
    }
}
