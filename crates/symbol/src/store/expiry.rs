// One slice of `store`: it shares the parent's imports and types rather
// than re-listing ~90 names. `allow`, not `expect`, because clippy
// reports an expectation on a `use` item as unfulfilled.
#[allow(clippy::wildcard_imports)]
use super::*;

pub(super) fn validate_file_expiry(expiry: FileExpiry) -> Result<FileExpiry, StoreError> {
    match expiry {
        FileExpiry::Policy(policy) => policy
            .validate()
            .map(FileExpiry::Policy)
            .map_err(Into::into),
        FileExpiry::Preserve | FileExpiry::Clear => Ok(expiry),
    }
}

pub(super) fn hash_file_expiry(hasher: &mut blake3::Hasher, expiry: FileExpiry) {
    match expiry {
        FileExpiry::Preserve => {
            hasher.update(&[0]);
        }
        FileExpiry::Clear => {
            hasher.update(&[1]);
        }
        FileExpiry::Policy(ExpiryPolicy::Relative { duration_seconds }) => {
            hasher.update(&[2]);
            hasher.update(&duration_seconds.to_le_bytes());
        }
        FileExpiry::Policy(ExpiryPolicy::Absolute {
            deadline_unix_seconds,
        }) => {
            hasher.update(&[3]);
            hasher.update(&deadline_unix_seconds.to_le_bytes());
        }
        FileExpiry::Policy(ExpiryPolicy::Decay(policy)) => {
            hasher.update(&[4]);
            hasher.update(&policy.min_age_seconds.to_le_bytes());
            hasher.update(&policy.max_age_seconds.to_le_bytes());
            hasher.update(&policy.max_size_bytes.to_le_bytes());
            hasher.update(&policy.power.to_bits().to_le_bytes());
        }
    }
}

pub(super) fn snapshot_expiry_policies_locked(
    db: &mut SqliteConnection,
    token: &str,
    site_id: i64,
) -> Result<(), StoreError> {
    let policies = expiry_policies::table
        .filter(expiry_policies::site_id.eq(site_id))
        .select(ExpiryPolicyRow::as_select())
        .load::<ExpiryPolicyRow>(db)?;
    for policy in policies {
        diesel::insert_into(undo_expiry_policies::table)
            .values((
                undo_expiry_policies::token.eq(token),
                undo_expiry_policies::path.eq(policy.path),
                undo_expiry_policies::target_kind.eq(policy.target_kind),
                undo_expiry_policies::mode.eq(policy.mode),
                undo_expiry_policies::duration_seconds.eq(policy.duration_seconds),
                undo_expiry_policies::deadline.eq(policy.deadline),
                undo_expiry_policies::min_age_seconds.eq(policy.min_age_seconds),
                undo_expiry_policies::max_age_seconds.eq(policy.max_age_seconds),
                undo_expiry_policies::max_size_bytes.eq(policy.max_size_bytes),
                undo_expiry_policies::power.eq(policy.power),
                undo_expiry_policies::refreshed.eq(policy.refreshed),
                undo_expiry_policies::own_deadline.eq(policy
                    .own_deadline
                    .expect("stored expiry deadline is present")),
                undo_expiry_policies::size_bytes.eq(policy.size_bytes),
            ))
            .execute(db)?;
    }
    Ok(())
}

pub(super) fn restore_expiry_policies_locked(
    db: &mut SqliteConnection,
    token: &str,
    site_id: i64,
) -> Result<(), StoreError> {
    let policies = undo_expiry_policies::table
        .filter(undo_expiry_policies::token.eq(token))
        .select(UndoExpiryPolicyRow::as_select())
        .load::<UndoExpiryPolicyRow>(db)?;
    for policy in policies {
        diesel::insert_into(expiry_policies::table)
            .values((
                expiry_policies::site_id.eq(site_id),
                expiry_policies::path.eq(policy.path),
                expiry_policies::target_kind.eq(policy.target_kind),
                expiry_policies::mode.eq(policy.mode),
                expiry_policies::duration_seconds.eq(policy.duration_seconds),
                expiry_policies::deadline.eq(policy.deadline),
                expiry_policies::min_age_seconds.eq(policy.min_age_seconds),
                expiry_policies::max_age_seconds.eq(policy.max_age_seconds),
                expiry_policies::max_size_bytes.eq(policy.max_size_bytes),
                expiry_policies::power.eq(policy.power),
                expiry_policies::refreshed.eq(policy.refreshed),
                expiry_policies::own_deadline.eq(Some(policy.own_deadline)),
                expiry_policies::size_bytes.eq(policy.size_bytes),
            ))
            .execute(db)?;
    }
    Ok(())
}

pub(super) fn expiry_display_path(name: &str, path: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{name}/{path}")
    }
}

pub(super) fn expiry_target_kind_locked(
    db: &mut SqliteConnection,
    name: &str,
    rel: &str,
) -> Result<ExpiryTargetKind, StoreError> {
    if !rel.is_empty() {
        let site_id = site_id_locked(db, name)?;
        let alias_kind = aliases::table
            .find((site_id, rel))
            .select(aliases::resolved_kind)
            .first::<Option<i64>>(db)
            .optional()?;
        if let Some(kind) = alias_kind {
            return match kind {
                Some(kind) if kind == AliasResolvedKind::Directory as i64 => {
                    Ok(ExpiryTargetKind::Folder)
                }
                Some(kind) if kind == AliasResolvedKind::File as i64 => Ok(ExpiryTargetKind::File),
                None => expiry_policies::table
                    .find((site_id, rel))
                    .select(expiry_policies::target_kind)
                    .first::<i64>(db)
                    .optional()?
                    .map_or(Ok(ExpiryTargetKind::File), ExpiryTargetKind::try_from)
                    .map_err(StoreError::Expiry),
                Some(_) => Err(StoreError::InvalidAliasTarget),
            };
        }
    }
    match node_locked(db, name, rel)? {
        NodeKind::Missing => Err(StoreError::NotFound),
        NodeKind::Dir if rel.is_empty() => Ok(ExpiryTargetKind::Site),
        NodeKind::Dir => Ok(ExpiryTargetKind::Folder),
        NodeKind::File { .. } => Ok(ExpiryTargetKind::File),
    }
}

pub(super) fn aggregate_paths(path: &str) -> Vec<&str> {
    let mut paths = vec![""];
    let mut offset = 0;
    while let Some(relative) = path[offset..].find('/') {
        let end = offset + relative;
        paths.push(&path[..end]);
        offset = end + 1;
    }
    paths
}

pub(super) fn adjust_aggregates_locked(
    tx: &mut SqliteConnection,
    site_id: i64,
    path: &str,
    byte_delta: i64,
    count_delta: i64,
) -> Result<(), StoreError> {
    if path == MANIFEST_PATH {
        return Ok(());
    }
    for aggregate_path in aggregate_paths(path) {
        diesel::insert_into(path_aggregates::table)
            .values((
                path_aggregates::site_id.eq(site_id),
                path_aggregates::path.eq(aggregate_path),
                path_aggregates::logical_bytes.eq(byte_delta),
                path_aggregates::file_count.eq(count_delta),
            ))
            .on_conflict((path_aggregates::site_id, path_aggregates::path))
            .do_update()
            .set((
                path_aggregates::logical_bytes
                    .eq(path_aggregates::logical_bytes + excluded(path_aggregates::logical_bytes)),
                path_aggregates::file_count
                    .eq(path_aggregates::file_count + excluded(path_aggregates::file_count)),
            ))
            .execute(tx)?;
    }
    diesel::delete(
        path_aggregates::table
            .filter(path_aggregates::site_id.eq(site_id))
            .filter(
                path_aggregates::logical_bytes
                    .le(0_i64)
                    .or(path_aggregates::file_count.le(0_i64)),
            ),
    )
    .execute(tx)?;
    Ok(())
}

pub(super) fn rebuild_aggregates_locked(
    tx: &mut SqliteConnection,
    site_id: i64,
) -> Result<(), StoreError> {
    diesel::delete(path_aggregates::table.filter(path_aggregates::site_id.eq(site_id)))
        .execute(tx)?;
    let files = files::table
        .filter(files::site_id.eq(site_id))
        .filter(files::path.ne(MANIFEST_PATH))
        .select((files::path, files::size))
        .load::<(String, i64)>(tx)?;
    for (path, size) in files {
        adjust_aggregates_locked(tx, site_id, &path, size, 1)?;
    }
    let allocated = allocated_entries::table
        .filter(allocated_entries::site_id.eq(site_id))
        .select((allocated_entries::path, allocated_entries::size))
        .load::<(String, i64)>(tx)?;
    for (path, size) in allocated {
        adjust_aggregates_locked(tx, site_id, &path, size, 1)?;
    }
    Ok(())
}

pub(super) fn expiry_target_size_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    rel: &str,
    kind: ExpiryTargetKind,
) -> Result<u64, StoreError> {
    if !rel.is_empty()
        && aliases::table
            .find((site_id, rel))
            .select(aliases::site_id)
            .first::<i64>(db)
            .optional()?
            .is_some()
    {
        return Ok(0);
    }
    let size = match kind {
        ExpiryTargetKind::Site => path_aggregates::table
            .find((site_id, ""))
            .select(path_aggregates::logical_bytes)
            .first::<i64>(db)
            .optional()?
            .unwrap_or(0),
        ExpiryTargetKind::File => {
            if let Some(size) = files::table
                .find((site_id, rel))
                .select(files::size)
                .first::<i64>(db)
                .optional()?
            {
                size
            } else if let Some(size) = allocated_entries::table
                .find((site_id, rel))
                .select(allocated_entries::size)
                .first::<i64>(db)
                .optional()?
            {
                size
            } else {
                let (resolved, _, used_alias) = resolve_db_path_final(db, site_id, rel)?;
                match resolved {
                    GraphResolution::File(file) if used_alias => file.size,
                    GraphResolution::Missing
                    | GraphResolution::Directory
                    | GraphResolution::File(_) => return Err(StoreError::NotFound),
                }
            }
        }
        ExpiryTargetKind::Folder => {
            if let Some(size) = path_aggregates::table
                .find((site_id, rel))
                .select(path_aggregates::logical_bytes)
                .first::<i64>(db)
                .optional()?
            {
                size
            } else {
                let graph = aliases::table
                    .filter(aliases::site_id.eq(site_id))
                    .select((aliases::path, aliases::canonical_target))
                    .load::<(String, String)>(db)?
                    .into_iter()
                    .collect::<BTreeMap<_, _>>();
                let real_entries = load_real_entries(db, site_id)?;
                let (resolved, target) = resolve_graph_path_final(&real_entries, &graph, rel)?;
                if !matches!(resolved, GraphResolution::Directory) {
                    return Err(StoreError::NotFound);
                }
                path_aggregates::table
                    .find((site_id, target))
                    .select(path_aggregates::logical_bytes)
                    .first::<i64>(db)?
            }
        }
    };
    Ok(size.cast_unsigned())
}

#[expect(clippy::too_many_lines)]
pub(super) fn store_expiry_policy_locked(
    tx: &mut SqliteConnection,
    write: ExpiryPolicyWrite<'_>,
) -> Result<(), StoreError> {
    let ExpiryPolicyWrite {
        site_id,
        path,
        kind,
        policy,
        size,
        now,
    } = write;
    let (duration, deadline, min_age, max_age, max_size, power, refreshed, own_deadline) =
        match policy {
            ExpiryPolicy::Relative { duration_seconds } => {
                let duration_millis = i64::try_from(duration_seconds)
                    .map_err(|_| ExpiryError::DeadlineOverflow)?
                    .checked_mul(1000)
                    .ok_or(ExpiryError::DeadlineOverflow)?;
                (
                    Some(
                        i64::try_from(duration_seconds)
                            .map_err(|_| ExpiryError::DeadlineOverflow)?,
                    ),
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(now),
                    now.checked_add(duration_millis)
                        .ok_or(ExpiryError::DeadlineOverflow)?,
                )
            }
            ExpiryPolicy::Absolute {
                deadline_unix_seconds,
            } => (
                None,
                Some(deadline_unix_seconds),
                None,
                None,
                None,
                None,
                None,
                deadline_unix_seconds
                    .checked_mul(1000)
                    .ok_or(ExpiryError::DeadlineOverflow)?,
            ),
            ExpiryPolicy::Decay(decay) => {
                let retention = decay.retention_seconds(size)?;
                let retention_millis = i64::try_from(retention)
                    .map_err(|_| ExpiryError::DeadlineOverflow)?
                    .checked_mul(1000)
                    .ok_or(ExpiryError::DeadlineOverflow)?;
                (
                    None,
                    None,
                    Some(
                        i64::try_from(decay.min_age_seconds)
                            .map_err(|_| ExpiryError::DeadlineOverflow)?,
                    ),
                    Some(
                        i64::try_from(decay.max_age_seconds)
                            .map_err(|_| ExpiryError::DeadlineOverflow)?,
                    ),
                    Some(
                        i64::try_from(decay.max_size_bytes)
                            .map_err(|_| ExpiryError::DeadlineOverflow)?,
                    ),
                    Some(decay.power),
                    Some(now),
                    now.checked_add(retention_millis)
                        .ok_or(ExpiryError::DeadlineOverflow)?,
                )
            }
        };
    diesel::insert_into(expiry_policies::table)
        .values((
            expiry_policies::site_id.eq(site_id),
            expiry_policies::path.eq(path),
            expiry_policies::target_kind.eq(i64::from(kind)),
            expiry_policies::mode.eq(i64::from(policy.mode())),
            expiry_policies::duration_seconds.eq(duration),
            expiry_policies::deadline.eq(deadline),
            expiry_policies::min_age_seconds.eq(min_age),
            expiry_policies::max_age_seconds.eq(max_age),
            expiry_policies::max_size_bytes.eq(max_size),
            expiry_policies::power.eq(power),
            expiry_policies::refreshed.eq(refreshed),
            expiry_policies::own_deadline.eq(Some(own_deadline)),
            expiry_policies::size_bytes
                .eq(i64::try_from(size).map_err(|_| ExpiryError::DeadlineOverflow)?),
        ))
        .on_conflict((expiry_policies::site_id, expiry_policies::path))
        .do_update()
        .set((
            expiry_policies::target_kind.eq(excluded(expiry_policies::target_kind)),
            expiry_policies::mode.eq(excluded(expiry_policies::mode)),
            expiry_policies::duration_seconds.eq(excluded(expiry_policies::duration_seconds)),
            expiry_policies::deadline.eq(excluded(expiry_policies::deadline)),
            expiry_policies::min_age_seconds.eq(excluded(expiry_policies::min_age_seconds)),
            expiry_policies::max_age_seconds.eq(excluded(expiry_policies::max_age_seconds)),
            expiry_policies::max_size_bytes.eq(excluded(expiry_policies::max_size_bytes)),
            expiry_policies::power.eq(excluded(expiry_policies::power)),
            expiry_policies::refreshed.eq(excluded(expiry_policies::refreshed)),
            expiry_policies::own_deadline.eq(excluded(expiry_policies::own_deadline)),
            expiry_policies::size_bytes.eq(excluded(expiry_policies::size_bytes)),
        ))
        .execute(tx)?;
    Ok(())
}

pub(super) fn file_expiry_change_required(
    tx: &mut SqliteConnection,
    site_id: i64,
    path: &str,
    expiry: FileExpiry,
) -> Result<bool, StoreError> {
    let current = load_expiry_policy_locked(tx, site_id, path)?;
    Ok(match expiry {
        FileExpiry::Preserve => false,
        FileExpiry::Clear => current.is_some(),
        FileExpiry::Policy(policy) => current.is_none_or(|stored| stored.policy != policy),
    })
}

pub(super) fn apply_file_expiry(
    tx: &mut SqliteConnection,
    site_id: i64,
    path: &str,
    size: u64,
    expiry: FileExpiry,
    now: i64,
) -> Result<(), StoreError> {
    match expiry {
        FileExpiry::Preserve => {}
        FileExpiry::Clear => {
            diesel::delete(expiry_policies::table.find((site_id, path))).execute(tx)?;
        }
        FileExpiry::Policy(policy) => {
            store_expiry_policy_locked(
                tx,
                ExpiryPolicyWrite {
                    site_id,
                    path,
                    kind: ExpiryTargetKind::File,
                    policy,
                    size,
                    now,
                },
            )?;
        }
    }
    Ok(())
}

pub(super) fn load_expiry_policy_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    path: &str,
) -> Result<Option<StoredExpiryPolicy>, StoreError> {
    let row = expiry_policies::table
        .find((site_id, path))
        .select(ExpiryPolicyRow::as_select())
        .first::<ExpiryPolicyRow>(db)
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let raw_kind = row.target_kind;
    let raw_mode = row.mode;
    let duration = row.duration_seconds;
    let deadline = row.deadline;
    let min_age = row.min_age_seconds;
    let max_age = row.max_age_seconds;
    let max_size = row.max_size_bytes;
    let power = row.power;
    let refreshed = row.refreshed;
    let own_deadline = row.own_deadline.ok_or(ExpiryError::InvalidTimestamp)?;
    let size = row.size_bytes;
    let mode = ExpiryMode::try_from(raw_mode)?;
    let policy = match mode {
        ExpiryMode::Relative => ExpiryPolicy::Relative {
            duration_seconds: duration
                .ok_or(ExpiryError::InvalidDuration)?
                .cast_unsigned(),
        },
        ExpiryMode::Absolute => ExpiryPolicy::Absolute {
            deadline_unix_seconds: deadline.ok_or(ExpiryError::InvalidTimestamp)?,
        },
        ExpiryMode::Decay => ExpiryPolicy::Decay(DecayPolicy {
            min_age_seconds: min_age.ok_or(ExpiryError::InvalidDuration)?.cast_unsigned(),
            max_age_seconds: max_age.ok_or(ExpiryError::InvalidDuration)?.cast_unsigned(),
            max_size_bytes: max_size.ok_or(ExpiryError::InvalidSize)?.cast_unsigned(),
            power: power.ok_or(ExpiryError::InvalidPower)?,
        }),
    };
    Ok(Some(StoredExpiryPolicy {
        kind: ExpiryTargetKind::try_from(raw_kind)?,
        policy,
        refreshed_millis: refreshed,
        own_deadline_millis: own_deadline,
        size_bytes: size.cast_unsigned(),
    }))
}

pub(super) fn own_expiry_report(stored: StoredExpiryPolicy) -> OwnExpiryReport {
    let (min_age_seconds, max_age_seconds, max_size_bytes, power) = match stored.policy {
        ExpiryPolicy::Decay(policy) => (
            Some(policy.min_age_seconds),
            Some(policy.max_age_seconds),
            Some(policy.max_size_bytes),
            Some(policy.power),
        ),
        ExpiryPolicy::Relative { .. } | ExpiryPolicy::Absolute { .. } => (None, None, None, None),
    };
    OwnExpiryReport {
        mode: stored.policy.mode(),
        min_age_seconds,
        max_age_seconds,
        max_size_bytes,
        power,
        retention_seconds: stored
            .policy
            .retention_seconds(stored.size_bytes)
            .expect("stored expiry policy is valid"),
        expires_at: format_timestamp(stored.own_deadline_millis),
    }
}

pub(super) struct ExpiryReportState {
    report: ExpiryReport,
    effective_millis: Option<i64>,
}

pub(super) fn base_expiry_report_locked(
    db: &mut SqliteConnection,
    name: &str,
    site_id: i64,
    rel: &str,
    now: i64,
) -> Result<ExpiryReportState, StoreError> {
    let kind = expiry_target_kind_locked(db, name, rel)?;
    let size = expiry_target_size_locked(db, site_id, rel, kind)?;
    let own = load_expiry_policy_locked(db, site_id, rel)?;
    let mut inherited = Vec::new();
    for ancestor in expiry_ancestor_paths(rel) {
        if let Some(stored) = load_expiry_policy_locked(db, site_id, &ancestor)? {
            inherited.push((ancestor, stored));
        }
    }
    let own_policy = own.map(own_expiry_report);
    let mut effective = own.map(|stored| stored.own_deadline_millis);
    let mut limited_by = None;
    let inherited_caps = inherited
        .into_iter()
        .map(|(path, stored)| {
            if effective.is_none_or(|deadline| stored.own_deadline_millis < deadline) {
                effective = Some(stored.own_deadline_millis);
                limited_by = Some(ExpiryLimit {
                    kind: stored.kind,
                    path: (!path.is_empty()).then_some(path.clone()),
                });
            }
            InheritedExpiryCap {
                kind: stored.kind,
                path: (!path.is_empty()).then_some(path),
                expires_at: format_timestamp(stored.own_deadline_millis),
            }
        })
        .collect();
    Ok(ExpiryReportState {
        report: ExpiryReport {
            target: ExpiryTarget {
                site: name.to_string(),
                path: (!rel.is_empty()).then(|| rel.to_string()),
                kind,
            },
            size,
            refreshed_at: own
                .and_then(|stored| stored.refreshed_millis)
                .map(format_timestamp),
            own_policy,
            inherited_caps,
            effective_expires_at: effective.map(format_timestamp),
            remaining_seconds: effective
                .map(|deadline| remaining_seconds(deadline / 1000, now / 1000)),
            limited_by,
        },
        effective_millis: effective,
    })
}

pub(super) fn effective_expiry_source(report: &ExpiryReport) -> ExpiryLimit {
    report.limited_by.clone().unwrap_or_else(|| ExpiryLimit {
        kind: report.target.kind,
        path: report.target.path.clone(),
    })
}

pub(super) fn apply_resolved_expiry_cap(
    state: &mut ExpiryReportState,
    cap: &ExpiryReportState,
    now: i64,
) {
    let Some(deadline) = cap.effective_millis else {
        return;
    };
    let source = effective_expiry_source(&cap.report);
    let expires_at = format_timestamp(deadline);
    if !state.report.inherited_caps.iter().any(|existing| {
        existing.kind == source.kind
            && existing.path == source.path
            && existing.expires_at == expires_at
    }) {
        state.report.inherited_caps.push(InheritedExpiryCap {
            kind: source.kind,
            path: source.path.clone(),
            expires_at,
        });
    }
    if state
        .effective_millis
        .is_none_or(|current| deadline < current)
    {
        state.effective_millis = Some(deadline);
        state.report.effective_expires_at = Some(format_timestamp(deadline));
        state.report.remaining_seconds = Some(remaining_seconds(deadline / 1000, now / 1000));
        state.report.limited_by = Some(source);
    }
}

pub(super) fn expiry_report_locked(
    db: &mut SqliteConnection,
    name: &str,
    site_id: i64,
    rel: &str,
    now: i64,
) -> Result<ExpiryReport, StoreError> {
    let mut state = base_expiry_report_locked(db, name, site_id, rel, now)?;
    let (resolution, physical, dependencies) = resolve_db_path_trace(db, site_id, rel)?;
    let mut capped_paths = BTreeSet::new();
    for dependency in dependencies {
        if dependency == rel || !capped_paths.insert(dependency.clone()) {
            continue;
        }
        let cap = base_expiry_report_locked(db, name, site_id, &dependency, now)?;
        apply_resolved_expiry_cap(&mut state, &cap, now);
    }
    if !matches!(resolution, GraphResolution::Missing)
        && physical != rel
        && capped_paths.insert(physical.clone())
    {
        let cap = base_expiry_report_locked(db, name, site_id, &physical, now)?;
        apply_resolved_expiry_cap(&mut state, &cap, now);
    }
    Ok(state.report)
}

pub(super) fn expiry_ancestor_paths(rel: &str) -> Vec<String> {
    if rel.is_empty() {
        return Vec::new();
    }
    let mut ancestors = Vec::new();
    let mut current = rel;
    while let Some((parent, _)) = current.rsplit_once('/') {
        ancestors.push(parent.to_string());
        current = parent;
    }
    ancestors.push(String::new());
    ancestors
}

pub(super) fn policy_is_affected(path: &str, kind: ExpiryTargetKind, changed: &str) -> bool {
    match kind {
        ExpiryTargetKind::Site => true,
        ExpiryTargetKind::File => path == changed,
        ExpiryTargetKind::Folder => {
            changed == path
                || changed
                    .strip_prefix(path)
                    .is_some_and(|tail| tail.starts_with('/'))
        }
    }
}

pub(super) fn copy_expiry_policies_locked(
    tx: &mut SqliteConnection,
    source_id: i64,
    destination_id: i64,
    now: i64,
) -> Result<(), StoreError> {
    let paths = expiry_policies::table
        .filter(expiry_policies::site_id.eq(source_id))
        .select(expiry_policies::path)
        .order(expiry_policies::path)
        .load::<String>(tx)?;
    for path in paths {
        let stored = load_expiry_policy_locked(tx, source_id, &path)?
            .expect("selected expiry policy still exists");
        let size = expiry_target_size_locked(tx, destination_id, &path, stored.kind)?;
        store_expiry_policy_locked(
            tx,
            ExpiryPolicyWrite {
                site_id: destination_id,
                path: &path,
                kind: stored.kind,
                policy: stored.policy,
                size,
                now,
            },
        )?;
    }
    Ok(())
}

pub(super) fn refresh_expiry_for_changes_locked(
    tx: &mut SqliteConnection,
    site_id: i64,
    changed_paths: &[&str],
    now: i64,
) -> Result<(), StoreError> {
    let policies = expiry_policies::table
        .filter(expiry_policies::site_id.eq(site_id))
        .select((expiry_policies::path, expiry_policies::target_kind))
        .load::<(String, i64)>(tx)?;
    if policies.is_empty() {
        return Ok(());
    }
    let policy_paths = policies.iter().map(|(path, _)| path).collect::<Vec<_>>();
    let mut alias_kinds = HashMap::new();
    for chunk in policy_paths.chunks(30_000) {
        alias_kinds.extend(
            aliases::table
                .filter(aliases::site_id.eq(site_id))
                .filter(aliases::path.eq_any(chunk))
                .select((aliases::path, aliases::resolved_kind))
                .load::<(String, Option<i64>)>(tx)?,
        );
    }
    for (path, raw_kind) in policies {
        let stored_kind = ExpiryTargetKind::try_from(raw_kind)?;
        let directly_affected = changed_paths
            .iter()
            .any(|changed| policy_is_affected(&path, stored_kind, changed));
        let alias_dependency_affected = if alias_kinds.contains_key(&path) {
            let (_, target, dependencies) = resolve_db_path_trace(tx, site_id, &path)?;
            changed_paths.iter().any(|changed| {
                dependencies.iter().any(|dependency| *changed == dependency)
                    || *changed == target
                    || changed
                        .strip_prefix(&target)
                        .is_some_and(|suffix| suffix.starts_with('/'))
                    || target
                        .strip_prefix(*changed)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
        } else {
            false
        };
        if !directly_affected && !alias_dependency_affected {
            continue;
        }
        let Some(stored) = load_expiry_policy_locked(tx, site_id, &path)? else {
            continue;
        };
        let kind = match alias_kinds.get(&path) {
            Some(Some(kind)) if *kind == AliasResolvedKind::File as i64 => ExpiryTargetKind::File,
            Some(Some(kind)) if *kind == AliasResolvedKind::Directory as i64 => {
                ExpiryTargetKind::Folder
            }
            Some(None) | None => stored_kind,
            Some(Some(_)) => return Err(StoreError::InvalidAliasTarget),
        };
        let size = expiry_target_size_locked(tx, site_id, &path, kind)?;
        store_expiry_policy_locked(
            tx,
            ExpiryPolicyWrite {
                site_id,
                path: &path,
                kind,
                policy: stored.policy,
                size,
                now,
            },
        )?;
    }
    Ok(())
}

pub(super) fn finish_partial_expiry_locked(
    tx: &mut SqliteConnection,
    blobs: &BlobFiles,
    site_id: i64,
    changed_path: &str,
    now: i64,
) -> Result<(), StoreError> {
    let remaining = site_entries::table
        .filter(site_entries::site_id.eq(site_id))
        .filter(site_entries::path.ne(MANIFEST_PATH))
        .filter(
            site_entries::kind
                .eq(database::schema::FILE_ENTRY_KIND)
                .or(site_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND))
                .or(site_entries::kind.eq(database::schema::ALIAS_ENTRY_KIND)),
        )
        .select(count_star())
        .first::<i64>(tx)?;
    if remaining == 0 {
        let name = sites::table
            .find(site_id)
            .select(sites::name)
            .first::<String>(tx)?;
        retain_management_tombstone(tx, &name, now)?;
        diesel::delete(sites::table.find(site_id)).execute(tx)?;
        return Ok(());
    }
    diesel::update(sites::table.find(site_id))
        .set(sites::content_revision.eq(sites::content_revision + 1))
        .execute(tx)?;
    refresh_aliases_locked(tx, site_id, &[AliasChange::Subtree(changed_path)])?;
    refresh_expiry_for_changes_locked(tx, site_id, &[changed_path], now)?;
    regenerate_site(tx, blobs, site_id, now)?;
    Ok(())
}

pub(super) fn prune_unlisted_locked(
    tx: &mut SqliteConnection,
    site_id: i64,
    keep: &HashSet<&str>,
) -> Result<(), StoreError> {
    let prune = site_entries::table
        .filter(site_entries::site_id.eq(site_id))
        .select(site_entries::path)
        .load::<String>(tx)?
        .into_iter()
        .filter(|path| !keep.contains(path.as_str()))
        .collect::<Vec<_>>();
    if !prune.is_empty() {
        let removed_files = files::table
            .filter(files::site_id.eq(site_id))
            .filter(files::path.eq_any(&prune))
            .select((files::path, files::size))
            .load::<(String, i64)>(tx)?;
        let removed_allocated = allocated_entries::table
            .filter(allocated_entries::site_id.eq(site_id))
            .filter(allocated_entries::path.eq_any(&prune))
            .select((allocated_entries::path, allocated_entries::size))
            .load::<(String, i64)>(tx)?;
        for (path, size) in removed_files.iter().chain(&removed_allocated) {
            adjust_aggregates_locked(tx, site_id, path, -*size, -1)?;
        }
        diesel::delete(
            site_entries::table
                .filter(site_entries::site_id.eq(site_id))
                .filter(site_entries::path.eq_any(&prune)),
        )
        .execute(tx)?;
    }
    let prune_expiry = expiry_policies::table
        .filter(expiry_policies::site_id.eq(site_id))
        .select(expiry_policies::path)
        .load::<String>(tx)?
        .into_iter()
        .filter(|path| !path.is_empty() && !keep.contains(path.as_str()))
        .collect::<Vec<_>>();
    if !prune_expiry.is_empty() {
        diesel::delete(
            expiry_policies::table
                .filter(expiry_policies::site_id.eq(site_id))
                .filter(expiry_policies::path.eq_any(&prune_expiry)),
        )
        .execute(tx)?;
    }
    Ok(())
}

pub(super) const fn expiry_mode_name(mode: ExpiryMode) -> &'static str {
    match mode {
        ExpiryMode::Relative => "relative",
        ExpiryMode::Absolute => "absolute",
        ExpiryMode::Decay => "decay",
    }
}

impl Store {
    #[cfg(test)]
    pub fn set_expiry(
        &self,
        name: &str,
        rel: &str,
        policy: Option<ExpiryPolicy>,
    ) -> Result<ExpiryMutation, StoreError> {
        self.set_expiry_secured(name, rel, policy, None)
    }

    pub fn set_expiry_secured(
        &self,
        name: &str,
        rel: &str,
        policy: Option<ExpiryPolicy>,
        authorization: Option<&ManagementToken>,
    ) -> Result<ExpiryMutation, StoreError> {
        let name = parse_site_name(name)?;
        let rel = normalize_rel(rel)?;
        let policy = policy.map(ExpiryPolicy::validate).transpose()?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, authorization)?;
        let site_id = site_id_locked(&mut tx, name)?;
        reject_expiry_below_alias_locked(&mut tx, site_id, &rel)?;
        let kind = expiry_target_kind_locked(&mut tx, name, &rel)?;
        let previous = expiry_policies::table
            .find((site_id, rel.as_str()))
            .select(expiry_policies::site_id)
            .first::<i64>(&mut *tx)
            .optional()?
            .is_some();
        let undo = if previous || policy.is_some() {
            Some(snapshot_site_with_description(
                &mut tx,
                name,
                UndoKind::Expiry,
                &format!(
                    "restore previous expiry policy for {}",
                    expiry_display_path(name, &rel)
                ),
                now,
            )?)
        } else {
            None
        };
        if let Some(policy) = policy {
            let size = expiry_target_size_locked(&mut tx, site_id, &rel, kind)?;
            store_expiry_policy_locked(
                &mut tx,
                ExpiryPolicyWrite {
                    site_id,
                    path: &rel,
                    kind,
                    policy,
                    size,
                    now,
                },
            )?;
        } else {
            diesel::delete(expiry_policies::table.find((site_id, rel.as_str())))
                .execute(&mut *tx)?;
        }
        regenerate_site(&mut tx, &self.inner.blob_files, site_id, now)?;
        prune_undo_locked(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        let report = self.expiry_report(name, &rel)?;
        Ok(ExpiryMutation { report, undo })
    }

    pub fn set_default_expiry_secured(
        &self,
        name: &str,
        rel: &str,
        authorization: Option<&ManagementToken>,
    ) -> Result<ExpiryMutation, StoreError> {
        self.set_expiry_secured(
            name,
            rel,
            Some(ExpiryPolicy::Decay(self.inner.expiry_defaults)),
            authorization,
        )
    }

    pub fn expiry_site_report(&self, name: &str) -> Result<ExpirySiteReport, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        let paths = expiry_policies::table
            .filter(expiry_policies::site_id.eq(site_id))
            .select(expiry_policies::path)
            .order(expiry_policies::path)
            .load::<String>(&mut *db)?;
        drop(db);
        let entries = paths
            .into_iter()
            .map(|path| self.expiry_report(name, &path))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ExpirySiteReport {
            site: name.to_string(),
            entries,
        })
    }

    pub fn expiry_report(&self, name: &str, rel: &str) -> Result<ExpiryReport, StoreError> {
        let name = parse_site_name(name)?;
        let rel = normalize_rel(rel)?;
        let now = self.now_millis();
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        expiry_report_locked(&mut db, name, site_id, &rel, now)
    }

    pub fn next_expiry_delay(&self) -> Result<std::time::Duration, StoreError> {
        let mut db = self.inner.readers.get();
        let next = expiry_policies::table
            .select(min(expiry_policies::own_deadline))
            .first::<Option<i64>>(&mut *db)?;
        let millis = next.map_or(60_000, |deadline| {
            deadline.saturating_sub(self.now_millis()).clamp(0, 60_000)
        });
        Ok(std::time::Duration::from_millis(
            u64::try_from(millis).expect("delay is non-negative"),
        ))
    }

    #[expect(clippy::too_many_lines)]
    pub fn sweep_expired(&self) -> Result<usize, StoreError> {
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        let due = expiry_policies::table
            .inner_join(sites::table)
            .filter(expiry_policies::own_deadline.le(now))
            .select((
                sites::name,
                expiry_policies::path,
                expiry_policies::target_kind,
            ))
            .order((
                sites::id,
                expiry_policies::target_kind,
                expiry_policies::path,
            ))
            .load::<(String, String, i64)>(&mut *tx)?;
        let mut swept_sites = HashSet::new();
        let mut removed_targets = 0;
        for (name, path, raw_kind) in due {
            if !site_exists_locked(&mut tx, &name)? {
                continue;
            }
            let kind = ExpiryTargetKind::try_from(raw_kind)?;
            let site_id = site_id_locked(&mut tx, &name)?;
            let deadline = expiry_policies::table
                .find((site_id, path.as_str()))
                .select(expiry_policies::own_deadline)
                .first::<Option<i64>>(&mut *tx)
                .optional()?
                .flatten();
            let still_due = deadline.is_some_and(|deadline| deadline <= now);
            if !still_due {
                continue;
            }
            if swept_sites.insert(name.clone()) {
                snapshot_site_with_description(
                    &mut tx,
                    &name,
                    UndoKind::ExpireSweep,
                    &format!("restore expired content in {name}"),
                    now,
                )?;
            }
            let exact_alias = !path.is_empty()
                && site_entries::table
                    .find((site_id, path.as_str()))
                    .select(site_entries::kind)
                    .first::<i64>(&mut *tx)
                    .optional()?
                    == Some(database::schema::ALIAS_ENTRY_KIND);
            if exact_alias {
                diesel::delete(site_entries::table.find((site_id, path.as_str())))
                    .execute(&mut *tx)?;
                diesel::delete(expiry_policies::table.find((site_id, path.as_str())))
                    .execute(&mut *tx)?;
                finish_partial_expiry_locked(&mut tx, &self.inner.blob_files, site_id, &path, now)?;
                removed_targets += 1;
                continue;
            }
            match kind {
                ExpiryTargetKind::Site => {
                    retain_management_tombstone(&mut tx, &name, now)?;
                    diesel::delete(sites::table.find(site_id)).execute(&mut *tx)?;
                }
                ExpiryTargetKind::File => {
                    let entry_kind = site_entries::table
                        .find((site_id, path.as_str()))
                        .select(site_entries::kind)
                        .first::<i64>(&mut *tx)?;
                    let size =
                        i64::try_from(entry_size_locked(&mut tx, site_id, &path, entry_kind)?)
                            .expect("stored size fits in i64");
                    diesel::delete(site_entries::table.find((site_id, path.as_str())))
                        .execute(&mut *tx)?;
                    adjust_aggregates_locked(&mut tx, site_id, &path, -size, -1)?;
                    diesel::delete(expiry_policies::table.find((site_id, path.as_str())))
                        .execute(&mut *tx)?;
                    finish_partial_expiry_locked(
                        &mut tx,
                        &self.inner.blob_files,
                        site_id,
                        &path,
                        now,
                    )?;
                }
                ExpiryTargetKind::Folder => {
                    let alias_folder = site_entries::table
                        .find((site_id, path.as_str()))
                        .select(site_entries::kind)
                        .first::<i64>(&mut *tx)
                        .optional()?
                        == Some(database::schema::ALIAS_ENTRY_KIND);
                    if alias_folder {
                        diesel::delete(site_entries::table.find((site_id, path.as_str())))
                            .execute(&mut *tx)?;
                        diesel::delete(expiry_policies::table.find((site_id, path.as_str())))
                            .execute(&mut *tx)?;
                    } else {
                        let (start, end) = descendant_bounds(&path);
                        let removed_files = files::table
                            .filter(files::site_id.eq(site_id))
                            .filter(files::path.ge(&start))
                            .filter(files::path.lt(&end))
                            .select((files::path, files::size))
                            .load::<(String, i64)>(&mut *tx)?;
                        let removed_allocated = allocated_entries::table
                            .filter(allocated_entries::site_id.eq(site_id))
                            .filter(allocated_entries::path.ge(&start))
                            .filter(allocated_entries::path.lt(&end))
                            .select((allocated_entries::path, allocated_entries::size))
                            .load::<(String, i64)>(&mut *tx)?;
                        diesel::delete(
                            site_entries::table
                                .filter(site_entries::site_id.eq(site_id))
                                .filter(site_entries::path.ge(&start))
                                .filter(site_entries::path.lt(&end)),
                        )
                        .execute(&mut *tx)?;
                        for (removed_path, size) in
                            removed_files.into_iter().chain(removed_allocated)
                        {
                            adjust_aggregates_locked(&mut tx, site_id, &removed_path, -size, -1)?;
                        }
                        diesel::delete(
                            expiry_policies::table
                                .filter(expiry_policies::site_id.eq(site_id))
                                .filter(
                                    expiry_policies::path.eq(&path).or(expiry_policies::path
                                        .ge(&start)
                                        .and(expiry_policies::path.lt(&end))),
                                ),
                        )
                        .execute(&mut *tx)?;
                    }
                    finish_partial_expiry_locked(
                        &mut tx,
                        &self.inner.blob_files,
                        site_id,
                        &path,
                        now,
                    )?;
                }
            }
            removed_targets += 1;
        }
        prune_undo_locked(&mut tx, now)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(removed_targets)
    }
}
