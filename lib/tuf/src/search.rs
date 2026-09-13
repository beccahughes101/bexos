//! Incremental target lookup: authenticate each delegation before requesting its children.
use super::*;
use alloc::rc::Rc;

const MAX_SEARCH_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SEARCH_ROLES: usize = 256;

struct Role {
    name: String,
    parent: Option<Rc<TargetsSigned>>,
    depth: usize,
    terminating: bool,
}

pub struct TargetSearch {
    root: RootMetadata,
    root_hash: [u8; 32],
    snapshot_hash: [u8; 32],
    references: BTreeMap<String, MetadataReference>,
    path: String,
    now: u64,
    stack: Vec<Role>,
    pending: Option<Role>,
    visited: BTreeSet<String>,
    downloaded: u64,
    target: Option<VerifiedTarget>,
}

impl TargetSearch {
    /// The snapshot must already have been accepted and durably committed by the caller.
    pub fn new(
        state: &ClientState,
        snapshot: &[u8],
        path: &str,
        now: u64,
    ) -> Result<Self, TufError> {
        ensure_safe_target_path(path)?;
        if snapshot.len() > MAX_METADATA_BYTES {
            return Err(TufError::BadLength);
        }
        let root = signed_root(&state.root.bytes)?.signed;
        expire(root.expires.as_deref(), now)?;
        verify_role(snapshot, &root, "snapshot")?;
        let snapshot_hash = Sha256::digest(canonical_signed(snapshot)?).into();
        if state.metadata_hashes.get("snapshot") != Some(&snapshot_hash) {
            return Err(TufError::Rollback);
        }
        let metadata = signed_snapshot(snapshot)?.signed;
        expire(metadata.expires.as_deref(), now)?;
        if metadata.version != state.snapshot_version || metadata.meta.len() > MAX_SEARCH_ROLES {
            return Err(TufError::Rollback);
        }
        let references = metadata
            .meta
            .iter()
            .map(|(name, meta)| Ok((name.clone(), reference(name, meta)?)))
            .collect::<Result<_, TufError>>()?;
        Ok(Self {
            root,
            root_hash: Sha256::digest(&state.root.bytes).into(),
            snapshot_hash,
            references,
            path: path.into(),
            now,
            stack: alloc::vec![Role {
                name: "targets".into(),
                parent: None,
                depth: 0,
                terminating: false,
            }],
            pending: None,
            visited: BTreeSet::new(),
            downloaded: 0,
            target: None,
        })
    }

    /// Return only the next reference needed by the ordered depth-first search.
    pub fn next_reference(&mut self) -> Result<Option<MetadataReference>, TufError> {
        if self.target.is_some() {
            return Ok(None);
        }
        if self.pending.is_none() {
            while let Some(role) = self.stack.pop() {
                if self.visited.contains(&role.name) {
                    continue;
                }
                if role.depth > MAX_DELEGATION_DEPTH || self.visited.len() >= MAX_SEARCH_ROLES {
                    return Err(TufError::DelegationLimit);
                }
                if role.terminating {
                    self.stack.clear();
                }
                self.visited.insert(role.name.clone());
                self.pending = Some(role);
                break;
            }
        }
        let Some(role) = &self.pending else {
            return Ok(None);
        };
        let reference = self
            .references
            .get(&format!("{}.json", role.name))
            .ok_or(TufError::MissingMetadata)?;
        if self.downloaded.saturating_add(reference.length) > MAX_SEARCH_BYTES {
            return Err(TufError::DelegationLimit);
        }
        Ok(Some(reference.clone()))
    }

    /// Accept one role and advance rollback state only after every check succeeds.
    /// The caller persists the resulting state before requesting another role or payload.
    pub fn accept(&mut self, state: &mut ClientState, bytes: &[u8]) -> Result<(), TufError> {
        if Sha256::digest(&state.root.bytes).as_slice() != self.root_hash
            || state.metadata_hashes.get("snapshot") != Some(&self.snapshot_hash)
        {
            return Err(TufError::Rollback);
        }
        let reference = self.next_reference()?.ok_or(TufError::WrongRole)?;
        reference.verify(bytes)?;
        let pending = self.pending.as_ref().ok_or(TufError::WrongRole)?;
        verify_targets_role(bytes, &self.root, &pending.name, pending.parent.as_deref())?;
        let role = signed_targets(bytes)?.signed;
        expire(role.expires.as_deref(), self.now)?;
        let previous = *state.targets_versions.get(&pending.name).unwrap_or(&0);
        if role.version != reference.version || role.version < previous {
            return Err(TufError::Rollback);
        }
        for path in role.targets.keys() {
            ensure_safe_target_path(path)?;
        }
        let mut names = BTreeSet::new();
        for child in role.delegations.roles.iter().flatten() {
            ensure_safe_target_path(&child.name)?;
            if child.name == "targets"
                || !names.insert(&child.name)
                || child.paths.is_some() == child.path_hash_prefixes.is_some()
            {
                return Err(TufError::UnsafeTargetPath);
            }
        }
        state.remember_metadata(
            &format!("targets:{}", pending.name),
            previous,
            role.version,
            bytes,
        )?;
        state
            .targets_versions
            .insert(pending.name.clone(), role.version);
        self.downloaded += reference.length;
        if let Some(target) = role.targets.get(&self.path) {
            self.target = Some(VerifiedTarget {
                role: pending.name.clone(),
                path: self.path.clone(),
                length: target.length,
                hashes: target.hashes.clone(),
                url: target.custom.bexos.url.clone(),
                generation: target.custom.bexos.generation.unwrap_or(0),
                kind: target.custom.bexos.kind.clone(),
                target_id: target.custom.bexos.target_id.clone(),
            });
            self.stack.clear();
        } else {
            let depth = pending.depth + 1;
            let parent = Rc::new(role);
            for child in parent.delegations.roles.iter().flatten().rev() {
                if delegation_matches(child, &self.path) {
                    self.stack.push(Role {
                        name: child.name.clone(),
                        parent: Some(parent.clone()),
                        depth,
                        terminating: child.terminating.unwrap_or(false),
                    });
                }
            }
        }
        self.pending = None;
        Ok(())
    }

    pub fn target(&self) -> Option<&VerifiedTarget> {
        self.target.as_ref()
    }
}
