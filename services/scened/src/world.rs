//! Inter-view admission and reusable committed-world compilation.
use crate::state::Scene;
use bexos_graphics::{
    Error,
    scene::{Graph, Session},
    world::{GraphSource, World},
};
use std::collections::{BTreeMap, BTreeSet};
fn roots(
    graphs: &impl GraphSource,
    stacking: &[u64],
    hidden: &BTreeSet<u64>,
) -> ([u64; 16], usize) {
    let mut children = [0; 16];
    let mut child_count = 0;
    for (_, graph) in graphs.graphs() {
        for node in graph.nodes.values() {
            if let Some(child) = node.embedded {
                if graphs.graph(child).is_some()
                    && !children[..child_count].contains(&child)
                    && child_count < 16
                {
                    children[child_count] = child;
                    child_count += 1;
                }
            }
        }
    }
    let mut roots = [0; 16];
    let mut count = 0;
    for owner in graphs
        .graphs()
        .map(|(id, _)| id)
        .filter(|id| !stacking.contains(id))
        .chain(stacking.iter().copied())
    {
        if graphs.graph(owner).is_some()
            && !hidden.contains(&owner)
            && !children[..child_count].contains(&owner)
            && count < 16
        {
            roots[count] = owner;
            count += 1;
        }
    }
    (roots, count)
}
struct Candidate<'a> {
    sessions: &'a BTreeMap<u64, Session>,
    owner: u64,
    graph: &'a Graph,
}
impl GraphSource for Candidate<'_> {
    fn graph(&self, id: u64) -> Option<&Graph> {
        if id == self.owner {
            Some(self.graph)
        } else {
            self.sessions.get(&id).map(|s| &s.committed)
        }
    }
    fn graphs(&self) -> impl Iterator<Item = (u64, &Graph)> {
        self.sessions.iter().map(|(id, s)| {
            (
                *id,
                if *id == self.owner {
                    self.graph
                } else {
                    &s.committed
                },
            )
        })
    }
}
pub fn validate(s: &Scene, owner: u64, graph: &Graph) -> Result<(), Error> {
    let graphs = Candidate {
        sessions: &s.sessions,
        owner,
        graph,
    };
    World::validate(&graphs)?;
    let (roots, count) = roots(&graphs, &s.controls.stacking, &s.controls.embedded);
    World::validate_geometry(&graphs, &roots[..count])
}
pub fn compile(s: &mut Scene, target: bexos_graphics::Surface) -> Result<(), Error> {
    let graphs = s
        .sessions
        .iter()
        .filter(|(id, _)| s.shell.visible(**id))
        .map(|(id, session)| (*id, &session.committed))
        .collect::<BTreeMap<_, _>>();
    let (mut roots, mut count) = roots(&graphs, &s.controls.stacking, &s.controls.embedded);
    if s.shell.enabled {
        count = 0;
        if let Some(root) = s.shell.root().filter(|id| graphs.contains_key(id)) {
            roots[0] = root;
            count = 1;
        }
    }
    s.world
        .compile(&graphs, &roots[..count], target, &mut s.snapshots)?;
    let damage = s.damage_tracker.update(
        s.world
            .items(&s.snapshots)
            .map(|(owner, item)| (owner, *item)),
        |owner| {
            s.snapshot_changes.contains(&owner)
                && s.queues
                    .get(&owner)
                    .is_none_or(|q| !s.fences.frames.contains_key(&(owner, q.latched_sequence)))
        },
    );
    if let Some(damage) = damage {
        s.damage = Some(s.damage.map_or(damage, |d| d.union(damage)));
    }
    s.snapshot_changes.clear();
    s.world_generation = s.controls.generation;
    Ok(())
}
pub fn latch(s: &mut Scene, ticks: u64) -> bool {
    let mut ids = [0; 16];
    let count = s.queues.len();
    for (out, id) in ids.iter_mut().zip(s.queues.keys()) {
        *out = *id;
    }
    let mut latched = false;
    let mut retired = false;
    for owner in ids.into_iter().take(count) {
        let Some(frame) = s.queues[&owner]
            .frames
            .front()
            .filter(|f| f.time <= ticks && s.fences.ready(owner, f.sequence))
        else {
            continue;
        };
        // Validate against earlier sessions already latched at this boundary.
        // Two queued reciprocal embeddings cannot both become committed.
        if validate(s, owner, &frame.graph).is_err() {
            let frame = s.queues.get_mut(&owner).unwrap().reject().unwrap();
            crate::fences::cancel(&mut s.fences, owner, frame.sequence);
            retired = true;
            continue;
        }
        let frame = s.queues.get_mut(&owner).unwrap().latch(ticks).unwrap();
        for node in frame.graph.nodes.values() {
            if let Some(child) = node.embedded.filter(|child| s.sessions.contains_key(child)) {
                s.controls.embedded.insert(child);
            }
        }
        let session = s.sessions.get_mut(&owner).unwrap();
        s.gpu.invalidate_transaction(
            &session.committed,
            &frame.graph,
            s.fences.frames.contains_key(&(owner, frame.sequence)),
        );
        session.committed = frame.graph;
        session.presentation = frame.time;
        s.controls.committed(owner, frame.sequence);
        crate::fences::commit(s, owner, frame.sequence);
        s.snapshot_changes.insert(owner);
        s.dirty = true;
        latched = true;
    }
    if latched {
        s.controls.changed();
    }
    if latched || retired {
        crate::session::collect_buffers(s);
    }
    latched
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reciprocal_presentations_cannot_latch_together_and_detached_children_stay_hidden() {
        let mut s = Scene::default();
        for id in 1..=2 {
            let mut session = Session::default();
            session.pending.create(1).unwrap();
            session.pending.root = Some(1);
            session.committed = session.pending.clone();
            session.pending.node(1).unwrap().embedded = Some(3 - id);
            s.queues
                .entry(id)
                .or_default()
                .submit(&session.pending, 0)
                .unwrap();
            s.sessions.insert(id, session);
            s.controls.register(id);
        }
        assert!(latch(&mut s, 0));
        assert_eq!(s.queues[&1].latched_sequence, 1);
        assert_eq!(s.queues[&2].rejected_sequence, 1);
        assert_eq!(s.sessions[&2].committed.nodes[&1].embedded, None);
        assert!(s.controls.embedded.contains(&2));
        s.sessions
            .get_mut(&1)
            .unwrap()
            .pending
            .node(1)
            .unwrap()
            .embedded = None;
        s.queues
            .get_mut(&1)
            .unwrap()
            .submit(&s.sessions[&1].pending, 1)
            .unwrap();
        assert!(latch(&mut s, 1));
        let (ids, count) = roots(&s.sessions, &s.controls.stacking, &s.controls.embedded);
        assert_eq!(&ids[..count], &[1]);
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;
    use crate::shell::{Composition, Owner};
    #[test]
    fn only_attached_same_user_content_reaches_the_display() {
        let target = bexos_graphics::Surface {
            width: 32,
            height: 32,
            stride: 128,
            format: bexos_graphics::Format::Bgra,
        };
        let mut s = Scene {
            shell: Composition {
                enabled: true,
                sysui: "sys".into(),
                userui: "user".into(),
                uid: 1000,
                ..Default::default()
            },
            ..Default::default()
        };
        for (id, package, uid, role) in [
            (1, "sys", 0, 1),
            (2, "user", 1000, 2),
            (3, "app", 1000, 0),
            (4, "other", 1001, 0),
            (5, "unattached", 1000, 0),
        ] {
            s.shell.owners.insert(
                id,
                Owner {
                    package: package.into(),
                    uid,
                    role,
                    epoch: 0,
                },
            );
            let mut v = Session::default();
            v.committed.create(1).unwrap();
            v.committed.root = Some(1);
            v.committed.node(1).unwrap().content = Some((id, target));
            s.sessions.insert(id, v);
            s.controls.register(id);
        }
        s.sessions
            .get_mut(&1)
            .unwrap()
            .committed
            .node(1)
            .unwrap()
            .embedded = Some(2);
        s.sessions
            .get_mut(&2)
            .unwrap()
            .committed
            .node(1)
            .unwrap()
            .embedded = Some(3);
        compile(&mut s, target).unwrap();
        assert_eq!(
            s.world.paint.iter().map(|p| p.view).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        s.shell.locked = true;
        compile(&mut s, target).unwrap();
        assert_eq!(
            s.world.paint.iter().map(|p| p.view).collect::<Vec<_>>(),
            vec![1]
        );
        s.shell.locked = false;
        // Even an existing embedding must not revive a retained shell grant
        // when the same UID begins a different login session.
        s.shell.epoch = 1;
        compile(&mut s, target).unwrap();
        assert_eq!(
            s.world.paint.iter().map(|p| p.view).collect::<Vec<_>>(),
            vec![1]
        );
        s.shell.owners.get_mut(&2).unwrap().epoch = 1;
        compile(&mut s, target).unwrap();
        assert_eq!(
            s.world.paint.iter().map(|p| p.view).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        s.shell.owners.remove(&1);
        compile(&mut s, target).unwrap();
        assert!(s.world.paint.is_empty());
    }
}
