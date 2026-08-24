//! Dependency resolution. Services start as soon as their own dependencies are
//! ready, so a graph boots in the time of its longest chain, not its sum.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::error::{Error, Result};
use crate::id::InstanceId;

pub(crate) type Edges = BTreeMap<InstanceId, Vec<InstanceId>>;

/// Expands the requested ids with everything they depend on, dependencies
/// first. Rejects unknown ids and cycles before anything is started.
pub(crate) fn plan(edges: &Edges, requested: &[InstanceId]) -> Result<Vec<InstanceId>> {
    let mut order = Vec::new();
    let mut settled = BTreeSet::new();
    let mut path = Vec::new();
    for id in requested {
        visit(edges, id, &mut order, &mut settled, &mut path)?;
    }
    Ok(order)
}

fn visit(
    edges: &Edges,
    id: &InstanceId,
    order: &mut Vec<InstanceId>,
    settled: &mut BTreeSet<InstanceId>,
    path: &mut Vec<InstanceId>,
) -> Result<()> {
    if settled.contains(id) {
        return Ok(());
    }
    if path.contains(id) {
        path.push(id.clone());
        let trail = path.iter().map(ToString::to_string).collect::<Vec<_>>();
        return Err(Error::DependencyCycle(trail.join(" -> ")));
    }
    let dependencies = edges
        .get(id)
        .ok_or_else(|| Error::UnknownInstance(id.clone()))?;

    path.push(id.clone());
    for dependency in dependencies {
        if !edges.contains_key(dependency) {
            return Err(Error::UnknownDependency {
                instance: id.clone(),
                missing: dependency.clone(),
            });
        }
        visit(edges, dependency, order, settled, path)?;
    }
    path.pop();

    settled.insert(id.clone());
    order.push(id.clone());
    Ok(())
}

/// What each planned service must wait for on the way up.
pub(crate) fn upward(order: &[InstanceId], edges: &Edges) -> HashMap<InstanceId, Vec<InstanceId>> {
    order
        .iter()
        .map(|id| (id.clone(), edges.get(id).cloned().unwrap_or_default()))
        .collect()
}

/// What each planned service must wait for on the way down: its dependents,
/// so nothing is pulled out from under something still using it.
pub(crate) fn downward(
    order: &[InstanceId],
    edges: &Edges,
) -> HashMap<InstanceId, Vec<InstanceId>> {
    let mut waits: HashMap<_, Vec<_>> = order.iter().map(|id| (id.clone(), Vec::new())).collect();
    for dependent in order {
        for dependency in edges.get(dependent).into_iter().flatten() {
            if let Some(list) = waits.get_mut(dependency) {
                list.push(dependent.clone());
            }
        }
    }
    waits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(text: &str) -> InstanceId {
        text.parse().unwrap()
    }

    fn edges(rows: &[(&str, &[&str])]) -> Edges {
        rows.iter()
            .map(|(name, deps)| (id(name), deps.iter().map(|d| id(d)).collect()))
            .collect()
    }

    #[test]
    fn dependencies_come_before_the_things_that_need_them() {
        let edges = edges(&[
            ("app@1", &["postgres@16", "valkey@8"]),
            ("postgres@16", &["disk@1"]),
            ("valkey@8", &["disk@1"]),
            ("disk@1", &[]),
        ]);

        let order = plan(&edges, &[id("app@1")]).unwrap();

        assert_eq!(order.first(), Some(&id("disk@1")));
        assert_eq!(order.last(), Some(&id("app@1")));
        assert_eq!(order.len(), 4, "a diamond visits each node once");
    }

    #[test]
    fn only_what_was_asked_for_is_planned() {
        let edges = edges(&[
            ("app@1", &["postgres@16"]),
            ("postgres@16", &[]),
            ("mailpit@1", &[]),
        ]);

        let order = plan(&edges, &[id("app@1")]).unwrap();

        assert_eq!(order, [id("postgres@16"), id("app@1")]);
    }

    #[test]
    fn a_cycle_is_named_rather_than_hung_on() {
        let edges = edges(&[("a@1", &["b@1"]), ("b@1", &["c@1"]), ("c@1", &["a@1"])]);

        let error = plan(&edges, &[id("a@1")]).unwrap_err();

        assert_eq!(
            error.to_string(),
            "dependency cycle: a@1 -> b@1 -> c@1 -> a@1"
        );
    }

    #[test]
    fn a_dependency_nobody_registered_is_caught_up_front() {
        let edges = edges(&[("app@1", &["postgres@16"])]);

        let error = plan(&edges, &[id("app@1")]).unwrap_err();

        assert!(matches!(error, Error::UnknownDependency { .. }));
    }

    #[test]
    fn shutdown_waits_on_dependents_instead() {
        let edges = edges(&[("app@1", &["postgres@16"]), ("postgres@16", &[])]);
        let order = plan(&edges, &[id("app@1")]).unwrap();

        assert_eq!(downward(&order, &edges)[&id("postgres@16")], [id("app@1")]);
        assert!(downward(&order, &edges)[&id("app@1")].is_empty());
        assert_eq!(upward(&order, &edges)[&id("app@1")], [id("postgres@16")]);
    }
}
