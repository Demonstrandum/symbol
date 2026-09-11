// One slice of `store`: it shares the parent's imports and types rather
// than re-listing ~90 names. `allow`, not `expect`, because clippy
// reports an expectation on a `use` item as unfulfilled.
#[allow(clippy::wildcard_imports)]
use super::*;

pub(super) fn canonical_alias_target(alias_path: &str, target: &str) -> Result<String, StoreError> {
    if target.is_empty()
        || target.len() > MAX_ALIAS_TARGET_BYTES
        || target.starts_with(['/', '\\'])
        || target.contains('\\')
        || target.chars().any(char::is_control)
        || looks_like_external_alias_target(target)
    {
        return Err(StoreError::InvalidAliasTarget);
    }
    let mut parts = alias_path
        .rsplit_once('/')
        .map_or_else(Vec::new, |(parent, _)| {
            parent.split('/').collect::<Vec<_>>()
        });
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(StoreError::InvalidAliasTarget);
                }
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(StoreError::InvalidAliasTarget);
    }
    let canonical = parts.join("/");
    safe_rel_path(&canonical).map_err(|_| StoreError::InvalidAliasTarget)?;
    if is_reserved_path(&canonical) || is_noise_path(Path::new(&canonical)) {
        return Err(StoreError::InvalidAliasTarget);
    }
    if relative_alias_target(alias_path, &canonical).len() > MAX_ALIAS_TARGET_BYTES {
        return Err(StoreError::InvalidAliasTarget);
    }
    Ok(canonical)
}

pub(super) fn looks_like_external_alias_target(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.contains("://")
        || [
            "data:",
            "file:",
            "ftp:",
            "ftps:",
            "git:",
            "http:",
            "https:",
            "javascript:",
            "mailto:",
            "ssh:",
            "ws:",
            "wss:",
        ]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

pub(super) fn alias_mutation_fingerprint(
    name: &str,
    aliases: &BTreeMap<String, String>,
    expected_tree_hash: Option<&str>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-alias-mutation-v1\0");
    hasher.update(name.as_bytes());
    for (path, target) in aliases {
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(&(target.len() as u64).to_le_bytes());
        hasher.update(target.as_bytes());
    }
    hasher.update(expected_tree_hash.unwrap_or("").as_bytes());
    hasher.finalize().to_hex().to_string()
}

pub(super) fn load_requested_aliases_locked<'a>(
    tx: &mut SqliteConnection,
    site_id: i64,
    paths: impl Iterator<Item = &'a String>,
) -> Result<Vec<AliasEntry>, StoreError> {
    let paths = paths.cloned().collect::<BTreeSet<_>>();
    aliases::table
        .filter(aliases::site_id.eq(site_id))
        .select(AliasRow::as_select())
        .order(aliases::path)
        .load::<AliasRow>(tx)?
        .into_iter()
        .filter(|row| paths.contains(&row.path))
        .map(alias_entry)
        .collect()
}

pub(super) fn alias_mutation_replay(
    tx: &mut SqliteConnection,
    idempotency: &Idempotency,
    fingerprint: &str,
) -> Result<Option<AliasMutationResult>, StoreError> {
    validate_idempotency_key(&idempotency.key)?;
    let record = idempotency_records::table
        .find(idempotency_key_hash(&idempotency.key))
        .select((
            idempotency_records::fingerprint,
            idempotency_records::operation_kind,
            idempotency_records::result_metadata,
        ))
        .first::<(String, i64, String)>(tx)
        .optional()?;
    let Some((stored_fingerprint, kind, metadata)) = record else {
        return Ok(None);
    };
    if stored_fingerprint != fingerprint || kind != IdempotencyKind::AliasMutation as i64 {
        return Err(StoreError::IdempotencyConflict);
    }
    let mut result = serde_json::from_str::<AliasMutationResult>(&metadata)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    result.mutation.replayed = true;
    Ok(Some(result))
}

pub(super) fn store_alias_mutation(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    result: &AliasMutationResult,
    now: i64,
) -> Result<(), StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(());
    };
    let metadata = serde_json::to_string(result)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    diesel::insert_into(idempotency_records::table)
        .values((
            idempotency_records::key_hash.eq(idempotency_key_hash(&idempotency.key)),
            idempotency_records::fingerprint.eq(fingerprint),
            idempotency_records::operation_kind.eq(IdempotencyKind::AliasMutation as i64),
            idempotency_records::result_metadata.eq(metadata),
            idempotency_records::expires.eq(now + IDEMPOTENCY_RETENTION_MILLIS),
        ))
        .execute(tx)?;
    Ok(())
}

pub(super) fn alias_entry(row: AliasRow) -> Result<AliasEntry, StoreError> {
    let resolved_kind = row
        .resolved_kind
        .map(|kind| match kind {
            0 => Ok(AliasResolvedKind::File),
            1 => Ok(AliasResolvedKind::Directory),
            _ => Err(StoreError::InvalidAliasTarget),
        })
        .transpose()?;
    Ok(AliasEntry {
        path: row.path,
        canonical_target: row.canonical_target,
        resolved_kind,
        resolved_hash: row.resolved_hash.map(ContentHash::to_wire),
        resolved_size: row.resolved_size.map(i64::cast_unsigned),
        resolved_files: None,
    })
}

#[derive(Clone)]
pub(super) struct RealEntry {
    pub(super) hash: ContentHash,
    pub(super) size: i64,
}

pub(super) enum GraphResolution {
    Missing,
    Directory,
    File(RealEntry),
}

#[derive(Clone, Copy)]
pub(super) enum AliasChange<'a> {
    Entry(&'a str),
    Alias(&'a str),
    Subtree(&'a str),
}

pub(super) fn load_real_entries(
    db: &mut SqliteConnection,
    site_id: i64,
) -> Result<BTreeMap<String, RealEntry>, StoreError> {
    let mut entries = files::table
        .filter(files::site_id.eq(site_id))
        .filter(files::path.ne(MANIFEST_PATH))
        .select((files::path, files::hash, files::size))
        .load::<(String, ContentHash, i64)>(db)?
        .into_iter()
        .map(|(path, hash, size)| (path, RealEntry { hash, size }))
        .collect::<BTreeMap<_, _>>();
    entries.extend(
        allocated_entries::table
            .filter(allocated_entries::site_id.eq(site_id))
            .select((
                allocated_entries::path,
                allocated_entries::hash,
                allocated_entries::size,
            ))
            .load::<(String, ContentHash, i64)>(db)?
            .into_iter()
            .map(|(path, hash, size)| (path, RealEntry { hash, size })),
    );
    Ok(entries)
}

pub(super) fn path_has_descendant<T>(entries: &BTreeMap<String, T>, path: &str) -> bool {
    let (start, end) = descendant_bounds(path);
    entries.range(start..end).next().is_some()
}

pub(super) fn alias_substitution<'a>(
    aliases: &'a BTreeMap<String, String>,
    path: &str,
) -> Option<(&'a str, &'a str)> {
    if let Some((path, target)) = aliases.get_key_value(path) {
        return Some((path, target));
    }
    let mut end = path.len();
    while let Some(slash) = path[..end].rfind('/') {
        let prefix = &path[..slash];
        if let Some((prefix, target)) = aliases.get_key_value(prefix) {
            return Some((prefix, target));
        }
        end = slash;
    }
    None
}

pub(super) fn resolve_graph_path(
    real: &BTreeMap<String, RealEntry>,
    aliases: &BTreeMap<String, String>,
    initial: &str,
) -> Result<GraphResolution, StoreError> {
    resolve_graph_path_final(real, aliases, initial).map(|(resolution, _)| resolution)
}

pub(super) fn resolve_graph_path_final(
    real: &BTreeMap<String, RealEntry>,
    aliases: &BTreeMap<String, String>,
    initial: &str,
) -> Result<(GraphResolution, String), StoreError> {
    resolve_graph_path_trace(real, aliases, initial).map(|(resolution, path, _)| (resolution, path))
}

pub(super) fn resolve_graph_path_trace(
    real: &BTreeMap<String, RealEntry>,
    aliases: &BTreeMap<String, String>,
    initial: &str,
) -> Result<(GraphResolution, String, Vec<String>), StoreError> {
    let mut path = initial.to_string();
    let mut visited = HashSet::new();
    let mut dependencies = Vec::new();
    for _ in 0..MAX_ALIAS_HOPS {
        if !visited.insert(path.clone()) {
            return Err(StoreError::AliasCycle);
        }
        if let Some(entry) = real.get(&path) {
            return Ok((GraphResolution::File(entry.clone()), path, dependencies));
        }
        if let Some((prefix, target)) = alias_substitution(aliases, &path) {
            dependencies.push(prefix.to_string());
            let suffix = &path[prefix.len()..];
            path = format!("{target}{suffix}");
            continue;
        }
        if path_has_descendant(real, &path) || path_has_descendant(aliases, &path) {
            return Ok((GraphResolution::Directory, path, dependencies));
        }
        return Ok((GraphResolution::Missing, path, dependencies));
    }
    Err(StoreError::AliasHopLimit)
}

pub(super) fn load_alias_rows_at_paths(
    tx: &mut SqliteConnection,
    site_id: i64,
    paths: &BTreeSet<String>,
) -> Result<Vec<AliasRow>, StoreError> {
    let mut rows = Vec::new();
    for chunk in paths.iter().collect::<Vec<_>>().chunks(30_000) {
        rows.extend(
            aliases::table
                .filter(aliases::site_id.eq(site_id))
                .filter(aliases::path.eq_any(chunk))
                .select(AliasRow::as_select())
                .load::<AliasRow>(tx)?,
        );
    }
    Ok(rows)
}

pub(super) fn load_alias_rows_for_exact_targets(
    tx: &mut SqliteConnection,
    site_id: i64,
    targets: &BTreeSet<String>,
) -> Result<Vec<AliasRow>, StoreError> {
    let mut rows = Vec::new();
    for chunk in targets.iter().collect::<Vec<_>>().chunks(30_000) {
        rows.extend(
            aliases::table
                .filter(aliases::site_id.eq(site_id))
                .filter(aliases::canonical_target.eq_any(chunk))
                .select(AliasRow::as_select())
                .load::<AliasRow>(tx)?,
        );
    }
    Ok(rows)
}

pub(super) fn alias_dependency_targets(
    paths: impl IntoIterator<Item = impl AsRef<str>>,
) -> BTreeSet<String> {
    let mut targets = BTreeSet::new();
    for path in paths {
        let path = path.as_ref();
        if path.is_empty() {
            continue;
        }
        targets.insert(path.to_string());
        targets.extend(
            aggregate_paths(path)
                .into_iter()
                .filter(|ancestor| !ancestor.is_empty())
                .map(str::to_string),
        );
    }
    targets
}

pub(super) fn add_affected_alias_rows(
    affected: &mut BTreeMap<String, AliasRow>,
    pending: &mut Vec<String>,
    rows: Vec<AliasRow>,
) {
    record_alias_refresh_rows(0, rows.len());
    for row in rows {
        if affected.contains_key(&row.path) {
            continue;
        }
        pending.push(row.path.clone());
        affected.insert(row.path.clone(), row);
    }
}

pub(super) fn refresh_aliases_locked(
    tx: &mut SqliteConnection,
    site_id: i64,
    changes: &[AliasChange<'_>],
) -> Result<(), StoreError> {
    pub(super) const FULL_REFRESH_THRESHOLD: usize = 512;

    if changes.is_empty() {
        return Ok(());
    }
    let changed_paths = changes
        .iter()
        .map(|change| change.path().to_string())
        .collect::<BTreeSet<_>>();
    let mut affected = BTreeMap::new();
    let mut pending = changes
        .iter()
        .filter(|change| change.includes_target_descendants())
        .map(|change| change.path().to_string())
        .collect::<Vec<_>>();
    add_affected_alias_rows(
        &mut affected,
        &mut pending,
        load_alias_rows_at_paths(tx, site_id, &changed_paths)?,
    );
    let dependency_targets = alias_dependency_targets(&changed_paths);
    add_affected_alias_rows(
        &mut affected,
        &mut pending,
        load_alias_rows_for_exact_targets(tx, site_id, &dependency_targets)?,
    );
    if affected.len() > FULL_REFRESH_THRESHOLD {
        return refresh_all_aliases_locked(tx, site_id);
    }

    let mut expanded = BTreeSet::new();
    let mut cursor = 0;
    while cursor < pending.len() {
        let path = pending[cursor].clone();
        cursor += 1;
        if path.is_empty() || !expanded.insert(path.clone()) {
            continue;
        }
        let targets = alias_dependency_targets(std::iter::once(path.as_str()));
        add_affected_alias_rows(
            &mut affected,
            &mut pending,
            load_alias_rows_for_exact_targets(tx, site_id, &targets)?,
        );
        let (start, end) = descendant_bounds(&path);
        let descendants = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .filter(aliases::canonical_target.ge(start))
            .filter(aliases::canonical_target.lt(end))
            .select(AliasRow::as_select())
            .load::<AliasRow>(tx)?;
        add_affected_alias_rows(&mut affected, &mut pending, descendants);
        if affected.len() > FULL_REFRESH_THRESHOLD {
            return refresh_all_aliases_locked(tx, site_id);
        }
    }

    for row in affected.into_values() {
        let (resolution, final_target, _) =
            resolve_db_path_final(tx, site_id, &row.canonical_target)?;
        if matches!(resolution, GraphResolution::Directory)
            && row
                .path
                .strip_prefix(&final_target)
                .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(StoreError::AliasCycle);
        }
        let value = match resolution {
            GraphResolution::Missing => (None, None, None),
            GraphResolution::Directory => (Some(AliasResolvedKind::Directory as i64), None, None),
            GraphResolution::File(file) => (
                Some(AliasResolvedKind::File as i64),
                Some(file.hash),
                Some(file.size),
            ),
        };
        resolve_db_path_final(tx, site_id, &row.path)?;
        if (
            row.resolved_kind,
            row.resolved_hash.as_ref(),
            row.resolved_size,
        ) != (value.0, value.1.as_ref(), value.2)
        {
            diesel::update(aliases::table.find((site_id, row.path.as_str())))
                .set((
                    aliases::resolved_kind.eq(value.0),
                    aliases::resolved_hash.eq(value.1),
                    aliases::resolved_size.eq(value.2),
                ))
                .execute(tx)?;
        }
    }
    Ok(())
}

pub(super) fn refresh_all_aliases_locked(
    tx: &mut SqliteConnection,
    site_id: i64,
) -> Result<(), StoreError> {
    let real = load_real_entries(tx, site_id)?;
    let rows = aliases::table
        .filter(aliases::site_id.eq(site_id))
        .select(AliasRow::as_select())
        .order(aliases::path)
        .load::<AliasRow>(tx)?;
    record_alias_refresh_rows(real.len(), rows.len());
    let graph = rows
        .iter()
        .map(|row| (row.path.clone(), row.canonical_target.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut resolved = Vec::with_capacity(rows.len());
    for row in rows {
        let (resolution, final_target) =
            resolve_graph_path_final(&real, &graph, &row.canonical_target)?;
        if matches!(resolution, GraphResolution::Directory)
            && row
                .path
                .strip_prefix(&final_target)
                .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(StoreError::AliasCycle);
        }
        let value = match resolution {
            GraphResolution::Missing => (None, None, None),
            GraphResolution::Directory => (Some(AliasResolvedKind::Directory as i64), None, None),
            GraphResolution::File(file) => (
                Some(AliasResolvedKind::File as i64),
                Some(file.hash),
                Some(file.size),
            ),
        };
        // Resolving the alias path itself catches prefix-substitution cycles that
        // are not visible by following only its canonical target.
        resolve_graph_path(&real, &graph, &row.path)?;
        if (
            row.resolved_kind,
            row.resolved_hash.as_ref(),
            row.resolved_size,
        ) != (value.0, value.1.as_ref(), value.2)
        {
            resolved.push((row.path, value));
        }
    }
    for (path, (kind, hash, size)) in resolved {
        diesel::update(aliases::table.find((site_id, path.as_str())))
            .set((
                aliases::resolved_kind.eq(kind),
                aliases::resolved_hash.eq(hash),
                aliases::resolved_size.eq(size),
            ))
            .execute(tx)?;
    }
    Ok(())
}

pub(super) fn validate_alias_graph_with_staged(
    tx: &mut SqliteConnection,
    site_id: i64,
    staged: &[&StagedFile],
    archive_aliases: &[ArchiveAlias<'_>],
) -> Result<(), StoreError> {
    let mut real = load_real_entries(tx, site_id)?;
    for file in staged {
        real.insert(
            file.path.clone(),
            RealEntry {
                hash: file.hash,
                size: file.size,
            },
        );
    }
    let mut graph = aliases::table
        .filter(aliases::site_id.eq(site_id))
        .select((aliases::path, aliases::canonical_target))
        .load::<(String, String)>(tx)?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    graph.extend(
        archive_aliases
            .iter()
            .map(|alias| (alias.path.to_string(), alias.target.to_string())),
    );
    for (path, target) in &graph {
        let (resolution, final_target) = resolve_graph_path_final(&real, &graph, target)?;
        if matches!(resolution, GraphResolution::Directory)
            && path
                .strip_prefix(&final_target)
                .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(StoreError::AliasCycle);
        }
        resolve_graph_path(&real, &graph, path)?;
    }
    Ok(())
}

pub(super) fn validate_archive_alias_conflicts_locked(
    tx: &mut SqliteConnection,
    site_id: i64,
    archive_aliases: &[ArchiveAlias<'_>],
) -> Result<(), StoreError> {
    let entries = site_entries::table
        .filter(site_entries::site_id.eq(site_id))
        .select((site_entries::path, site_entries::kind))
        .load::<(String, i64)>(tx)?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    for alias in archive_aliases {
        if aggregate_paths(alias.path).into_iter().any(|ancestor| {
            !ancestor.is_empty()
                && entries.get(ancestor).is_some_and(|kind| {
                    *kind == database::schema::ALIAS_ENTRY_KIND || ancestor != alias.path
                })
        }) {
            return Err(StoreError::AliasWrite);
        }
        if entries
            .get(alias.path)
            .is_some_and(|kind| *kind != database::schema::ALIAS_ENTRY_KIND)
        {
            return Err(StoreError::AliasConflict);
        }
        let (start, end) = descendant_bounds(alias.path);
        if entries.range(start..end).next().is_some() {
            return Err(StoreError::AliasConflict);
        }
    }
    Ok(())
}

pub(super) fn reject_alias_write_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    path: &str,
    allow_exact: bool,
) -> Result<(), StoreError> {
    reject_alias_writes_locked(db, site_id, [path], allow_exact)
}

pub(super) fn reject_expiry_below_alias_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    path: &str,
) -> Result<(), StoreError> {
    let ancestors = aggregate_paths(path)
        .into_iter()
        .filter(|ancestor| !ancestor.is_empty())
        .collect::<Vec<_>>();
    if ancestors.is_empty() {
        return Ok(());
    }
    let found = aliases::table
        .filter(aliases::site_id.eq(site_id))
        .filter(aliases::path.eq_any(ancestors))
        .select(aliases::site_id)
        .first::<i64>(db)
        .optional()?
        .is_some();
    if found {
        Err(StoreError::AliasWrite)
    } else {
        Ok(())
    }
}

pub(super) fn reject_alias_writes_locked<'a>(
    db: &mut SqliteConnection,
    site_id: i64,
    paths: impl IntoIterator<Item = &'a str>,
    allow_exact: bool,
) -> Result<(), StoreError> {
    let paths = paths.into_iter().collect::<Vec<_>>();
    let mut exact_conflicts = paths
        .iter()
        .flat_map(|path| aggregate_paths(path))
        .filter(|ancestor| !ancestor.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    if !allow_exact {
        exact_conflicts.extend(paths.iter().map(|path| (*path).to_string()));
    }
    for chunk in exact_conflicts.iter().collect::<Vec<_>>().chunks(30_000) {
        let found = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .filter(aliases::path.eq_any(chunk))
            .select(aliases::site_id)
            .first::<i64>(db)
            .optional()?
            .is_some();
        if found {
            return Err(StoreError::AliasWrite);
        }
    }
    if allow_exact {
        return Ok(());
    }
    if paths.len() <= 512 {
        for path in paths {
            let (start, end) = descendant_bounds(path);
            let found = aliases::table
                .filter(aliases::site_id.eq(site_id))
                .filter(aliases::path.ge(start))
                .filter(aliases::path.lt(end))
                .select(aliases::site_id)
                .first::<i64>(db)
                .optional()?
                .is_some();
            if found {
                return Err(StoreError::AliasWrite);
            }
        }
        return Ok(());
    }

    let changed = paths
        .iter()
        .map(|path| (*path).to_string())
        .collect::<BTreeSet<_>>();
    let top_levels = paths
        .iter()
        .filter_map(|path| path.split('/').next())
        .collect::<BTreeSet<_>>();
    for top_level in top_levels {
        let (start, end) = descendant_bounds(top_level);
        let candidates = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .filter(aliases::path.ge(start))
            .filter(aliases::path.lt(end))
            .select(aliases::path)
            .load::<String>(db)?;
        if candidates.into_iter().any(|candidate| {
            aggregate_paths(&candidate)
                .into_iter()
                .filter(|ancestor| !ancestor.is_empty())
                .any(|ancestor| changed.contains(ancestor))
        }) {
            return Err(StoreError::AliasWrite);
        }
    }
    Ok(())
}

pub(super) fn alias_node_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    rel: &str,
) -> Result<Option<NodeKind>, StoreError> {
    let direct_alias = aliases::table
        .find((site_id, rel))
        .select((aliases::resolved_kind, aliases::resolved_hash))
        .first::<(Option<i64>, Option<ContentHash>)>(db)
        .optional()?;
    if let Some((kind, hash)) = direct_alias {
        return Ok(Some(match (kind, hash) {
            (Some(kind), Some(hash)) if kind == AliasResolvedKind::File as i64 => {
                NodeKind::File { hash }
            }
            (Some(kind), _) if kind == AliasResolvedKind::Directory as i64 => NodeKind::Dir,
            _ => NodeKind::Missing,
        }));
    }
    let ancestor_paths = aggregate_paths(rel)
        .into_iter()
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    let has_alias_ancestor = if ancestor_paths.is_empty() {
        false
    } else {
        aliases::table
            .filter(aliases::site_id.eq(site_id))
            .filter(aliases::path.eq_any(&ancestor_paths))
            .select(aliases::site_id)
            .first::<i64>(db)
            .optional()?
            .is_some()
    };
    if has_alias_ancestor {
        let candidate_aliases = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .select((aliases::path, aliases::canonical_target))
            .load::<(String, String)>(db)?;
        let graph = candidate_aliases.into_iter().collect::<BTreeMap<_, _>>();
        let real_entries = load_real_entries(db, site_id)?;
        return Ok(Some(
            match resolve_graph_path(&real_entries, &graph, rel)? {
                GraphResolution::Missing => NodeKind::Missing,
                GraphResolution::Directory => NodeKind::Dir,
                GraphResolution::File(file) => NodeKind::File { hash: file.hash },
            },
        ));
    }
    Ok(None)
}

pub(super) fn resolved_alias_directory_target_locked(
    db: &mut SqliteConnection,
    name: &str,
    rel: &str,
) -> Result<Option<String>, StoreError> {
    let site_id = site_id_locked(db, name)?;
    let (resolved, target, used_alias) = resolve_db_path_final(db, site_id, rel)?;
    match resolved {
        GraphResolution::Directory if used_alias => Ok(Some(target)),
        GraphResolution::Missing | GraphResolution::File(_) | GraphResolution::Directory => {
            Ok(None)
        }
    }
}

pub(super) fn resolve_db_path_final(
    db: &mut SqliteConnection,
    site_id: i64,
    initial: &str,
) -> Result<(GraphResolution, String, bool), StoreError> {
    resolve_db_path_trace(db, site_id, initial)
        .map(|(resolution, path, dependencies)| (resolution, path, !dependencies.is_empty()))
}

pub(super) fn resolve_db_path_trace(
    db: &mut SqliteConnection,
    site_id: i64,
    initial: &str,
) -> Result<(GraphResolution, String, Vec<String>), StoreError> {
    let mut path = initial.to_string();
    let mut visited = HashSet::new();
    let mut dependencies = Vec::new();
    for _ in 0..MAX_ALIAS_HOPS {
        if !visited.insert(path.clone()) {
            return Err(StoreError::AliasCycle);
        }
        let file = files::table
            .find((site_id, path.as_str()))
            .select((files::hash, files::size))
            .first::<(ContentHash, i64)>(db)
            .optional()?;
        record_alias_resolution_rows(usize::from(file.is_some()));
        if let Some((hash, size)) = file {
            return Ok((
                GraphResolution::File(RealEntry { hash, size }),
                path,
                dependencies,
            ));
        }
        let allocated = allocated_entries::table
            .find((site_id, path.as_str()))
            .select((allocated_entries::hash, allocated_entries::size))
            .first::<(ContentHash, i64)>(db)
            .optional()?;
        record_alias_resolution_rows(usize::from(allocated.is_some()));
        if let Some((hash, size)) = allocated {
            return Ok((
                GraphResolution::File(RealEntry { hash, size }),
                path,
                dependencies,
            ));
        }
        let mut candidates = aggregate_paths(&path)
            .into_iter()
            .filter(|candidate| !candidate.is_empty())
            .collect::<Vec<_>>();
        candidates.push(path.as_str());
        candidates.sort_unstable();
        candidates.dedup();
        let substitutions = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .filter(aliases::path.eq_any(candidates))
            .select((aliases::path, aliases::canonical_target))
            .load::<(String, String)>(db)?;
        record_alias_resolution_rows(substitutions.len());
        if let Some((prefix, target)) = substitutions
            .into_iter()
            .max_by_key(|(prefix, _)| prefix.len())
        {
            let suffix = &path[prefix.len()..];
            dependencies.push(prefix.clone());
            path = format!("{target}{suffix}");
            continue;
        }
        let (start, end) = descendant_bounds(&path);
        let file_descendant = files::table
            .filter(files::site_id.eq(site_id))
            .filter(files::path.ge(&start))
            .filter(files::path.lt(&end))
            .select(files::site_id)
            .first::<i64>(db)
            .optional()?
            .is_some();
        record_alias_resolution_rows(usize::from(file_descendant));
        let allocated_descendant = if file_descendant {
            false
        } else {
            let found = allocated_entries::table
                .filter(allocated_entries::site_id.eq(site_id))
                .filter(allocated_entries::path.ge(&start))
                .filter(allocated_entries::path.lt(&end))
                .select(allocated_entries::site_id)
                .first::<i64>(db)
                .optional()?
                .is_some();
            record_alias_resolution_rows(usize::from(found));
            found
        };
        let alias_descendant = if file_descendant || allocated_descendant {
            false
        } else {
            let found = aliases::table
                .filter(aliases::site_id.eq(site_id))
                .filter(aliases::path.ge(&start))
                .filter(aliases::path.lt(&end))
                .select(aliases::site_id)
                .first::<i64>(db)
                .optional()?
                .is_some();
            record_alias_resolution_rows(usize::from(found));
            found
        };
        if file_descendant || allocated_descendant || alias_descendant {
            return Ok((GraphResolution::Directory, path, dependencies));
        }
        return Ok((GraphResolution::Missing, path, dependencies));
    }
    Err(StoreError::AliasHopLimit)
}

pub(super) fn load_alias_directory_files(
    db: &mut SqliteConnection,
    name: &str,
    logical: &str,
    target: &str,
) -> Result<Vec<(String, u64)>, StoreError> {
    let site_id = site_id_locked(db, name)?;
    let (start, end) = descendant_bounds(target);
    let mut rows = files::table
        .filter(files::site_id.eq(site_id))
        .filter(files::path.ge(&start))
        .filter(files::path.lt(&end))
        .select((files::path, files::size))
        .load::<(String, i64)>(db)?;
    record_alias_listed_file_rows(rows.len());
    let allocated_rows = allocated_entries::table
        .filter(allocated_entries::site_id.eq(site_id))
        .filter(allocated_entries::path.ge(&start))
        .filter(allocated_entries::path.lt(&end))
        .select((allocated_entries::path, allocated_entries::size))
        .load::<(String, i64)>(db)?;
    record_alias_listed_allocated_rows(allocated_rows.len());
    rows.extend(allocated_rows);
    rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok(rows
        .into_iter()
        .map(|(path, size)| {
            (
                format!("{logical}{}", &path[target.len()..]),
                size.cast_unsigned(),
            )
        })
        .collect())
}

pub(super) fn load_directory_aliases(
    db: &mut SqliteConnection,
    name: &str,
    logical: &str,
    physical: &str,
) -> Result<Vec<AliasEntry>, StoreError> {
    let site_id = site_id_locked(db, name)?;
    let mut query = aliases::table
        .filter(aliases::site_id.eq(site_id))
        .into_boxed();
    if !physical.is_empty() {
        let (start, end) = descendant_bounds(physical);
        query = query
            .filter(aliases::path.ge(start))
            .filter(aliases::path.lt(end));
    }
    let rows = query
        .select(AliasRow::as_select())
        .order(aliases::path)
        .load::<AliasRow>(db)?;
    record_alias_listed_rows(rows.len());
    let mut aggregate_targets = BTreeSet::new();
    let mut prepared = Vec::with_capacity(rows.len());
    for row in rows {
        let physical_alias = row.path.clone();
        let Some(suffix) = (if physical.is_empty() {
            Some(row.path.clone())
        } else {
            row.path
                .strip_prefix(physical)
                .and_then(|suffix| suffix.strip_prefix('/'))
                .map(str::to_string)
        }) else {
            continue;
        };
        let aggregate_target = if row.resolved_kind == Some(AliasResolvedKind::Directory as i64) {
            resolved_alias_directory_target_locked(db, name, &physical_alias)?.inspect(|target| {
                aggregate_targets.insert(target.clone());
            })
        } else {
            None
        };
        prepared.push((row, suffix, aggregate_target));
    }
    let mut aggregate_counts = BTreeMap::new();
    let aggregate_targets = aggregate_targets.into_iter().collect::<Vec<_>>();
    for targets in aggregate_targets.chunks(500) {
        let loaded = path_aggregates::table
            .filter(path_aggregates::site_id.eq(site_id))
            .filter(path_aggregates::path.eq_any(targets))
            .select((path_aggregates::path, path_aggregates::file_count))
            .load::<(String, i64)>(db)?;
        record_alias_aggregate_rows(loaded.len());
        aggregate_counts.extend(loaded);
    }
    let mut entries = Vec::with_capacity(prepared.len());
    for (mut row, suffix, aggregate_target) in prepared {
        let resolved_files = aggregate_target.map(|target| {
            aggregate_counts
                .get(&target)
                .copied()
                .unwrap_or(0)
                .cast_unsigned()
        });
        row.path = if logical.is_empty() {
            suffix
        } else {
            format!("{logical}/{suffix}")
        };
        let mut entry = alias_entry(row)?;
        entry.resolved_files = resolved_files;
        entries.push(entry);
    }
    Ok(entries)
}

pub(super) fn relative_alias_target(path: &str, target: &str) -> String {
    let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
    let from = if parent.is_empty() {
        Vec::new()
    } else {
        parent.split('/').collect::<Vec<_>>()
    };
    let to = target.split('/').collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = vec![".."; from.len() - common];
    parts.extend_from_slice(&to[common..]);
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

pub(super) fn zip_safe_relative_alias_target(path: &str, target: &str) -> io::Result<String> {
    let relative = relative_alias_target(path, target);
    if relative.len() > MAX_ALIAS_TARGET_BYTES {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "alias target exceeds ZIP-safe limit",
        ))
    } else {
        Ok(relative)
    }
}

impl Store {
    #[cfg(test)]
    pub fn put_alias(
        &self,
        name: &str,
        path: &str,
        target: &str,
        options: FileMutationOptions<'_>,
    ) -> Result<MutationResult, StoreError> {
        self.put_aliases(name, &[AliasSpec { path, target }], options)
    }

    #[cfg(test)]
    pub fn put_aliases(
        &self,
        name: &str,
        specs: &[AliasSpec<'_>],
        options: FileMutationOptions<'_>,
    ) -> Result<MutationResult, StoreError> {
        self.put_aliases_with_receipt(name, specs, options)
            .map(|result| result.mutation)
    }

    #[expect(clippy::significant_drop_tightening, clippy::too_many_lines)]
    pub fn put_aliases_with_receipt(
        &self,
        name: &str,
        specs: &[AliasSpec<'_>],
        options: FileMutationOptions<'_>,
    ) -> Result<AliasMutationResult, StoreError> {
        let name = parse_site_name(name)?;
        if specs.is_empty() {
            return Err(StoreError::InvalidAliasTarget);
        }
        let mut requested = BTreeMap::new();
        for spec in specs {
            if spec.path.starts_with(['/', '\\']) || spec.path.contains('\\') {
                return Err(StoreError::InvalidAliasTarget);
            }
            let path = normalize_rel(spec.path)?;
            if path.is_empty()
                || path.chars().any(char::is_control)
                || is_noise_path(Path::new(&path))
            {
                return Err(StoreError::InvalidAliasTarget);
            }
            reject_reserved_path(&path)?;
            let target = canonical_alias_target(&path, spec.target)?;
            match requested.entry(path) {
                std::collections::btree_map::Entry::Occupied(existing)
                    if existing.get() != &target =>
                {
                    return Err(StoreError::AliasConflict);
                }
                std::collections::btree_map::Entry::Occupied(_) => {}
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(target);
                }
            }
        }
        let fingerprint = alias_mutation_fingerprint(name, &requested, options.expected_tree_hash);
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, options.authorization)?;
        prune_idempotency_locked(&mut tx, now)?;
        if let Some(idempotency) = options.idempotency {
            validate_idempotency_key(&idempotency.key)?;
            if let Some(replay) = alias_mutation_replay(&mut tx, idempotency, &fingerprint)? {
                return Ok(replay);
            }
        }
        check_tree_precondition(&mut tx, name, options.expected_tree_hash)?;
        let site_id = site_id_locked(&mut tx, name)?;
        let entry_kinds = site_entries::table
            .filter(site_entries::site_id.eq(site_id))
            .select((site_entries::path, site_entries::kind))
            .load::<(String, i64)>(&mut *tx)?
            .into_iter()
            .collect::<HashMap<_, _>>();
        let entry_paths = entry_kinds.keys().cloned().collect::<BTreeSet<_>>();
        let existing_aliases = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .select((aliases::path, aliases::canonical_target))
            .load::<(String, String)>(&mut *tx)?
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let prospective_alias_paths = existing_aliases
            .keys()
            .chain(requested.keys())
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut changed_paths = Vec::new();
        let mut created_paths = 0_usize;
        for (path, target) in &requested {
            if path
                .strip_prefix(target)
                .is_some_and(|suffix| suffix.starts_with('/'))
            {
                return Err(StoreError::AliasCycle);
            }
            if aggregate_paths(path)
                .into_iter()
                .any(|ancestor| !ancestor.is_empty() && prospective_alias_paths.contains(ancestor))
            {
                return Err(StoreError::AliasWrite);
            }
            let (descendant_start, descendant_end) = descendant_bounds(path);
            if entry_paths
                .range(descendant_start..descendant_end)
                .next()
                .is_some()
            {
                return Err(StoreError::AliasConflict);
            }
            let entry_kind = entry_kinds.get(path).copied();
            match entry_kind {
                Some(kind) if kind != database::schema::ALIAS_ENTRY_KIND => {
                    return Err(StoreError::AliasConflict);
                }
                Some(_) => {
                    let current = existing_aliases
                        .get(path)
                        .expect("alias entry has alias metadata");
                    if current != target {
                        changed_paths.push(path.as_str());
                    }
                }
                None => {
                    changed_paths.push(path.as_str());
                    created_paths += 1;
                }
            }
        }
        if changed_paths.is_empty() {
            let (revision, tree_hash) = site_revision_locked(&mut tx, name)?;
            let result = MutationResult {
                created: false,
                changed: false,
                replayed: false,
                files: requested.len(),
                revision,
                tree_hash: tree_hash.to_wire(),
                undo: None,
                sanitized: TokenCounts::default(),
            };
            let result = AliasMutationResult {
                aliases: load_requested_aliases_locked(&mut tx, site_id, requested.keys())?,
                mutation: result,
            };
            store_alias_mutation(&mut tx, options.idempotency, &fingerprint, &result, now)?;
            tx.commit()?;
            return Ok(result);
        }
        let undo = snapshot_entry_deltas(
            &mut tx,
            name,
            UndoKind::Alias,
            &format!("restore previous aliases in {name}"),
            &changed_paths,
            now,
        )?;
        for path in &changed_paths {
            let target = &requested[*path];
            diesel::insert_into(site_entries::table)
                .values((
                    site_entries::site_id.eq(site_id),
                    site_entries::path.eq(*path),
                    site_entries::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                ))
                .on_conflict_do_nothing()
                .execute(&mut *tx)?;
            diesel::insert_into(aliases::table)
                .values((
                    aliases::site_id.eq(site_id),
                    aliases::path.eq(*path),
                    aliases::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                    aliases::canonical_target.eq(target),
                    aliases::resolved_kind.eq(Option::<i64>::None),
                    aliases::resolved_hash.eq(Option::<ContentHash>::None),
                    aliases::resolved_size.eq(Option::<i64>::None),
                ))
                .on_conflict((aliases::site_id, aliases::path))
                .do_update()
                .set((
                    aliases::canonical_target.eq(excluded(aliases::canonical_target)),
                    aliases::resolved_kind.eq(Option::<i64>::None),
                    aliases::resolved_hash.eq(Option::<ContentHash>::None),
                    aliases::resolved_size.eq(Option::<i64>::None),
                ))
                .execute(&mut *tx)?;
        }
        diesel::update(sites::table.find(site_id))
            .set((
                sites::updated.eq(now),
                sites::content_revision.eq(sites::content_revision + 1),
            ))
            .execute(&mut *tx)?;
        record_site_event(
            &mut tx,
            site_id,
            StoredSiteEventKind::Publish,
            changed_paths.len(),
            now,
        )?;
        let alias_changes = changed_paths
            .iter()
            .map(|path| AliasChange::Alias(path))
            .collect::<Vec<_>>();
        refresh_aliases_locked(&mut tx, site_id, &alias_changes)?;
        refresh_expiry_for_changes_locked(&mut tx, site_id, &changed_paths, now)?;
        let tree_hash = regenerate_site(&mut tx, &self.inner.blob_files, site_id, now)?;
        let (revision, _) = site_revision_locked(&mut tx, name)?;
        prune_undo_locked(&mut tx, now)?;
        let result = MutationResult {
            created: created_paths == changed_paths.len(),
            changed: true,
            replayed: false,
            files: changed_paths.len(),
            revision,
            tree_hash: tree_hash.to_wire(),
            undo: Some(undo),
            sanitized: TokenCounts::default(),
        };
        let result = AliasMutationResult {
            aliases: load_requested_aliases_locked(&mut tx, site_id, requested.keys())?,
            mutation: result,
        };
        store_alias_mutation(&mut tx, options.idempotency, &fingerprint, &result, now)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn alias(&self, name: &str, path: &str) -> Result<AliasEntry, StoreError> {
        let name = parse_site_name(name)?;
        let path = normalize_rel(path)?;
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        let row = aliases::table
            .find((site_id, path.as_str()))
            .select(AliasRow::as_select())
            .first::<AliasRow>(&mut *db)
            .map_err(map_sql)?;
        alias_entry(row)
    }

    #[cfg(test)]
    pub fn aliases(&self, name: &str) -> Result<Vec<AliasEntry>, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        aliases::table
            .filter(aliases::site_id.eq(site_id))
            .select(AliasRow::as_select())
            .order(aliases::path)
            .load::<AliasRow>(&mut *db)?
            .into_iter()
            .map(alias_entry)
            .collect()
    }

    #[cfg(test)]
    pub fn alias_inventory(&self, name: &str) -> Result<AliasInventory, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        let mut snapshot = DbTransaction::begin(&mut db)?;
        let (site_id, content_revision, tree_hash) = sites::table
            .filter(sites::name.eq(name))
            .select((sites::id, sites::content_revision, sites::tree_hash))
            .first::<(i64, i64, TreeHash)>(&mut *snapshot)
            .map_err(map_sql)?;
        let aliases = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .select(AliasRow::as_select())
            .order(aliases::path)
            .load::<AliasRow>(&mut *snapshot)?
            .into_iter()
            .map(alias_entry)
            .collect::<Result<Vec<_>, _>>()?;
        snapshot.commit()?;
        Ok(AliasInventory {
            site: name.to_string(),
            content_revision: content_revision.cast_unsigned(),
            tree_hash: tree_hash.to_wire(),
            aliases,
        })
    }

    #[cfg(test)]
    pub fn alias_stats(&self, name: &str) -> Result<AliasStats, StoreError> {
        let inventory = self.alias_inventory(name)?;
        let aliases = u64::try_from(inventory.aliases.len()).expect("alias count fits in u64");
        let resolved = u64::try_from(
            inventory
                .aliases
                .iter()
                .filter(|alias| alias.resolved_kind.is_some())
                .count(),
        )
        .expect("resolved alias count fits in u64");
        Ok(AliasStats {
            aliases,
            resolved,
            dangling: aliases - resolved,
        })
    }
}
