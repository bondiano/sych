//! `module-cycle`: sibling modules that use each other in a circle, so there
//! is no order to read them in. A module uses a sibling when code written in
//! it or inside it names an item written in the sibling or inside it. What a
//! module and the modules inside it use of each other makes no circle, and
//! neither do crates, which Cargo keeps in order.

use std::collections::BTreeMap;

use super::{Check, Diagnostic, Tie, circle_report, counted, listed};
use crate::architecture::graph::circles;
use crate::architecture::model::{ItemKind, Model};
use crate::architecture::tree::{NodeId, NodeKind, Tree};

pub fn check(model: &Model) -> Vec<Diagnostic> {
    let tree = &model.tree;
    // By parent, the ties between its children.
    let mut ties: BTreeMap<NodeId, BTreeMap<(NodeId, NodeId), Tie>> = BTreeMap::new();
    for reference in model.references.iter().filter(|r| !r.test) {
        let item = &model.items[reference.item.index()];
        let test = tree.is_test(item.owner)
            || item.file.is_some_and(|file| model.files[file.index()].test);
        if item.kind == ItemKind::Module || test {
            continue;
        }

        if let Some((parent, user, used)) = siblings(tree, reference.from, item.owner) {
            ties.entry(parent)
                .or_default()
                .entry((user, used))
                .or_default()
                .add(model, reference.item, reference, user);
        }
    }
    let mut diagnostics = Vec::new();
    for (parent, ties) in ties {
        let mut children: Vec<NodeId> = ties.keys().flat_map(|&(a, b)| [a, b]).collect();
        children.sort();
        children.dedup();
        let place = |child: NodeId| children.binary_search(&child).expect("a tied child");
        let mut uses = vec![Vec::new(); children.len()];
        for &(user, used) in ties.keys() {
            uses[place(user)].push(place(used));
        }
        for circle in circles(&uses) {
            let members: Vec<NodeId> = circle.into_iter().map(|at| children[at]).collect();
            diagnostics.push(diagnostic(model, parent, &members, &ties));
        }
    }
    diagnostics
}

/// The module that holds both `a` and `b`, and its children that hold each,
/// unless one of the two holds the other or they are in different crates.
fn siblings(tree: &Tree, a: NodeId, b: NodeId) -> Option<(NodeId, NodeId, NodeId)> {
    let parent = tree.common_ancestor(a, b);
    if parent == a || parent == b || tree.node(parent).kind == NodeKind::Workspace {
        return None;
    }

    let child = |of: NodeId| {
        tree.ancestors(of)
            .find(|&node| tree.parent(node) == Some(parent))
    };
    Some((parent, child(a)?, child(b)?))
}

fn diagnostic(
    model: &Model,
    parent: NodeId,
    members: &[NodeId],
    ties: &BTreeMap<(NodeId, NodeId), Tie>,
) -> Diagnostic {
    let tree = &model.tree;
    let name = |module: NodeId| tree.node(module).name.clone();
    let mut names: Vec<String> = members.iter().map(|&member| name(member)).collect();
    names.sort();
    let mut keys: Vec<String> = members.iter().map(|&m| tree.display(m)).collect();
    keys.sort();
    let inside = ties
        .iter()
        .filter(|((user, used), _)| members.contains(user) && members.contains(used))
        .map(|(&(user, used), tie)| ((name(user), name(used)), tie))
        .collect();
    let (location, ties, picture) = circle_report(model, "uses", "module", members.len(), inside);
    let mut notes = vec![format!("members: {}", listed(&names))];
    notes.extend(ties);
    Diagnostic {
        check: Check::ModuleCycle,
        key: keys.join(","),
        size: members.len(),
        members: keys,
        message: format!(
            "{} in `{}` use each other in a circle",
            counted(members.len(), "module"),
            tree.display(parent)
        ),
        location,
        notes,
        picture,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::architecture::checks::{Location, Picture, Step};
    use crate::architecture::model::sample::Sample;

    fn keys(sample: &Sample) -> Vec<String> {
        check(&sample.model).into_iter().map(|d| d.key).collect()
    }

    #[test]
    fn siblings_that_use_each_other_are_a_circle_though_no_two_things_are() {
        let mut sample = Sample::new();
        let (socket, kinds) = (sample.module("socket"), sample.module("kinds"));
        let open = sample.item(socket, "open", ItemKind::Function);
        let limit = sample.item(socket, "LIMIT", ItemKind::Constant);
        let stream = sample.item(kinds, "Stream", ItemKind::Type);
        let check_limit = sample.item(kinds, "fits", ItemKind::Function);
        sample.refer(open, stream);
        assert_eq!(keys(&sample), Vec::<String>::new());
        sample.refer(check_limit, limit);
        let diagnostics = check(&sample.model);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].key, "p::kinds,p::socket");
        assert_eq!(
            diagnostics[0].message,
            "2 modules in `p` use each other in a circle"
        );
        assert_eq!(
            diagnostics[0].notes,
            [
                "members: kinds, socket",
                "src/kinds.rs:2:1: `kinds` uses `socket` by 1 item: LIMIT",
                "src/socket.rs:1:1: `socket` uses `kinds` by 1 item: Stream",
            ]
        );
        let at = |file: &str, line| Location {
            file: file.into(),
            line,
        };
        assert_eq!(
            diagnostics[0].picture,
            Picture::Circle {
                steps: vec![
                    Step {
                        member: "kinds".into(),
                        site: at("src/kinds.rs", 2),
                        uses: "fits uses LIMIT".into(),
                    },
                    Step {
                        member: "socket".into(),
                        site: at("src/socket.rs", 1),
                        uses: "open uses Stream".into(),
                    },
                ],
                rest: None,
            }
        );
    }

    #[test]
    fn a_circle_is_between_the_children_of_the_module_that_holds_it() {
        let mut sample = Sample::new();
        let (epoll, address) = (
            sample.module("kinds::epoll"),
            sample.module("socket::address"),
        );
        let wait = sample.item(epoll, "wait", ItemKind::Function);
        let parse = sample.item(address, "parse", ItemKind::Function);
        sample.refer(wait, parse);
        sample.refer(parse, wait);
        assert_eq!(keys(&sample), ["p::kinds,p::socket"]);
    }

    #[test]
    fn a_module_and_what_is_inside_it_make_no_circle() {
        let mut sample = Sample::new();
        let (kinds, epoll) = (sample.module("kinds"), sample.module("kinds::epoll"));
        let kind = sample.item(kinds, "Kind", ItemKind::Type);
        let wait = sample.item(epoll, "wait", ItemKind::Function);
        let poll = sample.item(kinds, "poll", ItemKind::Function);
        sample.refer(wait, kind);
        sample.refer(poll, wait);
        assert_eq!(keys(&sample), Vec::<String>::new());
    }

    #[test]
    fn a_method_counts_where_it_is_written() {
        let mut sample = Sample::new();
        let (tasks, timers) = (sample.module("tasks"), sample.module("timers"));
        let task = sample.item(tasks, "Tasks", ItemKind::Type);
        let timer = sample.item(timers, "Timers", ItemKind::Type);
        let fire = sample.member(timer, timers, "Timers::fire", ItemKind::Function);
        sample.refer(fire, task);
        // `impl Tasks { fn spawn(&mut self, timers: &mut Timers) }`, in `timers`.
        let spawn = sample.member(task, timers, "Tasks::spawn", ItemKind::Function);
        sample.refer(spawn, timer);
        assert_eq!(keys(&sample), Vec::<String>::new());
        // The same, in `tasks`.
        let spawn = sample.member(task, tasks, "Tasks::spawn", ItemKind::Function);
        sample.refer(spawn, timer);
        assert_eq!(keys(&sample), ["p::tasks,p::timers"]);
    }
}
