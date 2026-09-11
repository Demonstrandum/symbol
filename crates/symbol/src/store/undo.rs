// One slice of `store`: it shares the parent's imports and types rather
// than re-listing ~90 names. `allow`, not `expect`, because clippy
// reports an expectation on a `use` item as unfulfilled.
#[allow(clippy::wildcard_imports)]
use super::*;

impl UndoKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::DeletePath => "delete_path",
            Self::DeleteSite => "delete_site",
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Expiry => "expiry",
            Self::ExpireSweep => "expire_sweep",
            Self::PutFile => "put_file",
            Self::Allocate => "allocate",
            Self::Replace => "replace",
            Self::Splice => "splice",
            Self::Alias => "alias",
        }
    }

    pub(super) const fn from_i64(value: i64) -> Result<Self, StoreError> {
        match value {
            1 => Ok(Self::Put),
            2 => Ok(Self::DeletePath),
            3 => Ok(Self::DeleteSite),
            4 => Ok(Self::Copy),
            5 => Ok(Self::Move),
            6 => Ok(Self::Expiry),
            7 => Ok(Self::ExpireSweep),
            8 => Ok(Self::PutFile),
            9 => Ok(Self::Allocate),
            10 => Ok(Self::Replace),
            11 => Ok(Self::Splice),
            12 => Ok(Self::Alias),
            _ => Err(StoreError::UnsupportedUndoKind(value)),
        }
    }
}

pub(super) struct SiteSnapshot {
    name: String,
    existed: bool,
    public_url: String,
    created: Option<i64>,
    updated: i64,
    content_revision: i64,
    tree_hash: TreeHash,
}

pub(super) fn snapshot_site(
    tx: &mut SqliteConnection,
    name: &str,
    kind: UndoKind,
    now: i64,
) -> Result<UndoInfo, StoreError> {
    let description = match kind {
        UndoKind::Put | UndoKind::PutFile => format!("restore previous state of {name}"),
        UndoKind::DeletePath | UndoKind::DeleteSite => format!("restore deleted site {name}"),
        UndoKind::Copy => format!("remove copied site {name}"),
        UndoKind::Move => format!("restore previous name {name}"),
        UndoKind::Expiry => format!("restore previous expiry policy for {name}"),
        UndoKind::ExpireSweep => format!("restore expired content in {name}"),
        UndoKind::Allocate => format!("remove allocated content from {name}"),
        UndoKind::Replace => format!("restore replaced content in {name}"),
        UndoKind::Splice => format!("restore spliced content in {name}"),
        UndoKind::Alias => format!("restore aliases in {name}"),
    };
    snapshot_site_with_description(tx, name, kind, &description, now)
}

pub(super) fn insert_undo_allocated(
    tx: &mut SqliteConnection,
    token: &str,
    path: &str,
    metadata: Option<&AllocatedMetadata>,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(undo_allocated_deltas::table)
        .values((
            undo_allocated_deltas::token.eq(token),
            undo_allocated_deltas::path.eq(path),
            undo_allocated_deltas::existed.eq(i64::from(metadata.is_some())),
            undo_allocated_deltas::hash.eq(metadata.map(|value| value.hash)),
            undo_allocated_deltas::size.eq(metadata.map(|value| value.size)),
            undo_allocated_deltas::naming_mode.eq(metadata.map(|value| value.naming_mode as i64)),
            undo_allocated_deltas::prefix.eq(metadata.map(|value| value.prefix.as_str())),
            undo_allocated_deltas::suffix.eq(metadata.map(|value| value.suffix.as_str())),
            undo_allocated_deltas::extension
                .eq(metadata.and_then(|value| value.extension.as_deref())),
            undo_allocated_deltas::media_type.eq(metadata.map(|value| value.media_type.as_str())),
        ))
        .execute(tx)?;
    Ok(())
}

pub(super) fn insert_undo_alias(
    tx: &mut SqliteConnection,
    token: &str,
    path: &str,
    alias: Option<&AliasRow>,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(undo_alias_deltas::table)
        .values((
            undo_alias_deltas::token.eq(token),
            undo_alias_deltas::path.eq(path),
            undo_alias_deltas::existed.eq(i64::from(alias.is_some())),
            undo_alias_deltas::canonical_target.eq(alias.map(|row| row.canonical_target.as_str())),
            undo_alias_deltas::resolved_kind.eq(alias.and_then(|row| row.resolved_kind)),
            undo_alias_deltas::resolved_hash.eq(alias.and_then(|row| row.resolved_hash)),
            undo_alias_deltas::resolved_size.eq(alias.and_then(|row| row.resolved_size)),
        ))
        .execute(tx)?;
    Ok(())
}

#[expect(clippy::too_many_lines)]
pub(super) fn snapshot_site_with_description(
    tx: &mut SqliteConnection,
    name: &str,
    kind: UndoKind,
    description: &str,
    now: i64,
) -> Result<UndoInfo, StoreError> {
    let token = undo_token()?;
    let expires = now + UNDO_RETENTION_MILLIS;
    diesel::insert_into(undo_operations::table)
        .values((
            undo_operations::token.eq(&token),
            undo_operations::kind.eq(kind as i64),
            undo_operations::description.eq(description),
            undo_operations::created.eq(now),
            undo_operations::expires.eq(expires),
            undo_operations::consumed.eq(0_i64),
        ))
        .execute(tx)?;
    diesel::insert_into(undo_names::table)
        .values((undo_names::token.eq(&token), undo_names::name.eq(name)))
        .execute(tx)?;
    let site = sites::table
        .filter(sites::name.eq(name))
        .select((
            sites::name,
            sites::public_url,
            sites::created,
            sites::updated,
            sites::content_revision,
            sites::tree_hash,
        ))
        .first::<(String, String, Option<i64>, i64, i64, TreeHash)>(tx)
        .optional()?
        .map_or_else(
            || SiteSnapshot {
                name: name.to_string(),
                existed: false,
                public_url: String::new(),
                created: None,
                updated: now,
                content_revision: 0,
                tree_hash: TreeHash::EMPTY,
            },
            |(name, public_url, created, updated, content_revision, tree_hash)| SiteSnapshot {
                name,
                existed: true,
                public_url,
                created,
                updated,
                content_revision,
                tree_hash,
            },
        );
    diesel::insert_into(undo_sites::table)
        .values((
            undo_sites::token.eq(&token),
            undo_sites::name.eq(&site.name),
            undo_sites::existed.eq(i64::from(site.existed)),
            undo_sites::public_url.eq(&site.public_url),
            undo_sites::created.eq(site.created),
            undo_sites::updated.eq(site.updated),
            undo_sites::content_revision.eq(site.content_revision),
            undo_sites::tree_hash.eq(site.tree_hash),
        ))
        .execute(tx)?;
    if site.existed {
        let site_id = site_id_locked(tx, name)?;
        let saved_files = files::table
            .filter(files::site_id.eq(site_id))
            .filter(files::path.ne(MANIFEST_PATH))
            .select((files::path, files::hash, files::size))
            .load::<(String, ContentHash, i64)>(tx)?;
        for (path, hash, size) in saved_files {
            diesel::insert_into(undo_files::table)
                .values((
                    undo_files::token.eq(&token),
                    undo_files::path.eq(path),
                    undo_files::hash.eq(hash),
                    undo_files::size.eq(size),
                ))
                .execute(tx)?;
        }
        let saved_allocated = allocated_entries::table
            .filter(allocated_entries::site_id.eq(site_id))
            .select((
                allocated_entries::path,
                allocated_entries::hash,
                allocated_entries::size,
                allocated_entries::naming_mode,
                allocated_entries::prefix,
                allocated_entries::suffix,
                allocated_entries::extension,
                allocated_entries::media_type,
            ))
            .load::<(
                String,
                ContentHash,
                i64,
                i64,
                String,
                String,
                Option<String>,
                String,
            )>(tx)?;
        for (path, hash, size, naming_mode, prefix, suffix, extension, media_type) in
            saved_allocated
        {
            insert_undo_allocated(
                tx,
                &token,
                &path,
                Some(&AllocatedMetadata {
                    hash,
                    size,
                    naming_mode: AllocatedNamingMode::try_from(naming_mode)?,
                    prefix,
                    suffix,
                    extension,
                    media_type,
                }),
            )?;
        }
        let saved_aliases = aliases::table
            .filter(aliases::site_id.eq(site_id))
            .select(AliasRow::as_select())
            .load::<AliasRow>(tx)?;
        for alias in saved_aliases {
            insert_undo_alias(tx, &token, &alias.path, Some(&alias))?;
        }
        snapshot_expiry_policies_locked(tx, &token, site_id)?;
    }
    Ok(UndoInfo {
        token,
        expires_at: format_timestamp(expires),
    })
}

#[expect(clippy::too_many_lines)]
pub(super) fn snapshot_entry_deltas(
    tx: &mut SqliteConnection,
    name: &str,
    kind: UndoKind,
    description: &str,
    paths: &[&str],
    now: i64,
) -> Result<UndoInfo, StoreError> {
    let token = undo_token()?;
    let expires = now + UNDO_RETENTION_MILLIS;
    diesel::insert_into(undo_operations::table)
        .values((
            undo_operations::token.eq(&token),
            undo_operations::kind.eq(kind as i64),
            undo_operations::description.eq(description),
            undo_operations::created.eq(now),
            undo_operations::expires.eq(expires),
            undo_operations::consumed.eq(0_i64),
        ))
        .execute(tx)?;
    diesel::insert_into(undo_names::table)
        .values((undo_names::token.eq(&token), undo_names::name.eq(name)))
        .execute(tx)?;
    let (site_id, public_url, created, updated, content_revision, tree_hash) = sites::table
        .filter(sites::name.eq(name))
        .select((
            sites::id,
            sites::public_url,
            sites::created,
            sites::updated,
            sites::content_revision,
            sites::tree_hash,
        ))
        .first::<(i64, String, Option<i64>, i64, i64, TreeHash)>(tx)?;
    diesel::insert_into(undo_sites::table)
        .values((
            undo_sites::token.eq(&token),
            undo_sites::name.eq(name),
            undo_sites::existed.eq(1_i64),
            undo_sites::public_url.eq(public_url),
            undo_sites::created.eq(created),
            undo_sites::updated.eq(updated),
            undo_sites::content_revision.eq(content_revision),
            undo_sites::tree_hash.eq(tree_hash),
        ))
        .execute(tx)?;
    if matches!(kind, UndoKind::Alias) {
        let mut previous = HashMap::new();
        for chunk in paths.chunks(SQLITE_DELETE_BATCH_SIZE) {
            previous.extend(
                aliases::table
                    .filter(aliases::site_id.eq(site_id))
                    .filter(aliases::path.eq_any(chunk))
                    .select(AliasRow::as_select())
                    .load::<AliasRow>(tx)?
                    .into_iter()
                    .map(|row| (row.path.clone(), row)),
            );
        }
        for path in paths {
            insert_undo_alias(tx, &token, path, previous.get(*path))?;
        }
        snapshot_expiry_policies_locked(tx, &token, site_id)?;
        return Ok(UndoInfo {
            token,
            expires_at: format_timestamp(expires),
        });
    }
    let operation_is_allocated = matches!(kind, UndoKind::Allocate)
        || (matches!(kind, UndoKind::Replace | UndoKind::Splice)
            && site_entries::table
                .filter(site_entries::site_id.eq(site_id))
                .filter(site_entries::path.eq_any(paths))
                .filter(site_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND))
                .select(count_star())
                .first::<i64>(tx)?
                > 0);
    for path in paths {
        let entry_kind = site_entries::table
            .find((site_id, *path))
            .select(site_entries::kind)
            .first::<i64>(tx)
            .optional()?;
        match entry_kind {
            Some(entry_kind) if entry_kind == database::schema::FILE_ENTRY_KIND => {
                let (hash, size) = files::table
                    .find((site_id, *path))
                    .select((files::hash, files::size))
                    .first::<(ContentHash, i64)>(tx)?;
                diesel::insert_into(undo_file_deltas::table)
                    .values((
                        undo_file_deltas::token.eq(&token),
                        undo_file_deltas::path.eq(*path),
                        undo_file_deltas::existed.eq(1_i64),
                        undo_file_deltas::kind.eq(Some(entry_kind)),
                        undo_file_deltas::hash.eq(Some(hash)),
                        undo_file_deltas::size.eq(Some(size)),
                    ))
                    .execute(tx)?;
            }
            Some(entry_kind) if entry_kind == database::schema::ALLOCATED_ENTRY_KIND => {
                let metadata = allocated_metadata_locked(tx, site_id, path)?;
                insert_undo_allocated(tx, &token, path, Some(&metadata))?;
            }
            Some(entry_kind) if entry_kind == database::schema::ALIAS_ENTRY_KIND => {
                let alias = aliases::table
                    .find((site_id, *path))
                    .select(AliasRow::as_select())
                    .first::<AliasRow>(tx)?;
                insert_undo_alias(tx, &token, path, Some(&alias))?;
            }
            Some(_) => return Err(StoreError::DestinationConflict),
            None if matches!(kind, UndoKind::Alias) => {
                insert_undo_alias(tx, &token, path, None)?;
            }
            None if operation_is_allocated => {
                insert_undo_allocated(tx, &token, path, None)?;
            }
            None => {
                diesel::insert_into(undo_file_deltas::table)
                    .values((
                        undo_file_deltas::token.eq(&token),
                        undo_file_deltas::path.eq(*path),
                        undo_file_deltas::existed.eq(0_i64),
                        undo_file_deltas::kind.eq(Option::<i64>::None),
                        undo_file_deltas::hash.eq(Option::<ContentHash>::None),
                        undo_file_deltas::size.eq(Option::<i64>::None),
                    ))
                    .execute(tx)?;
            }
        }
    }
    snapshot_expiry_policies_locked(tx, &token, site_id)?;
    Ok(UndoInfo {
        token,
        expires_at: format_timestamp(expires),
    })
}

#[expect(clippy::too_many_lines)]
pub(super) fn restore_entry_deltas(
    tx: &mut SqliteConnection,
    blob_files: &BlobFiles,
    name: &str,
    token: &str,
) -> Result<i64, StoreError> {
    let (updated, content_revision) = undo_sites::table
        .find(token)
        .select((undo_sites::updated, undo_sites::content_revision))
        .first::<(i64, i64)>(tx)?;
    let site_id = site_id_locked(tx, name)?;
    let file_deltas = undo_file_deltas::table
        .filter(undo_file_deltas::token.eq(token))
        .select((
            undo_file_deltas::path,
            undo_file_deltas::existed,
            undo_file_deltas::hash,
            undo_file_deltas::size,
        ))
        .load::<(String, i64, Option<ContentHash>, Option<i64>)>(tx)?;
    let allocated_deltas = undo_allocated_deltas::table
        .filter(undo_allocated_deltas::token.eq(token))
        .select(UndoAllocatedMetadata::as_select())
        .load::<UndoAllocatedMetadata>(tx)?;
    let alias_deltas = undo_alias_deltas::table
        .filter(undo_alias_deltas::token.eq(token))
        .select(UndoAliasRow::as_select())
        .load::<UndoAliasRow>(tx)?;
    let entry_change_paths = file_deltas
        .iter()
        .map(|row| row.0.clone())
        .chain(allocated_deltas.iter().map(|row| row.path.clone()))
        .collect::<Vec<_>>();
    let alias_change_paths = alias_deltas
        .iter()
        .map(|row| row.path.clone())
        .collect::<Vec<_>>();
    let changed_paths = entry_change_paths
        .iter()
        .chain(&alias_change_paths)
        .map(String::as_str)
        .collect::<Vec<_>>();
    for paths in changed_paths.chunks(512) {
        diesel::delete(
            site_entries::table
                .filter(site_entries::site_id.eq(site_id))
                .filter(site_entries::path.eq_any(paths)),
        )
        .execute(tx)?;
    }
    for (path, existed, hash, size) in file_deltas {
        if existed == 0 {
            continue;
        }
        let hash = hash.expect("existing file delta has hash");
        let size = size.expect("existing file delta has size");
        ensure_file_entry(tx, site_id, &path)?;
        diesel::insert_into(files::table)
            .values(NewFile {
                site_id,
                path,
                hash,
                size,
            })
            .execute(tx)?;
    }
    for delta in allocated_deltas {
        if delta.existed == 0 {
            continue;
        }
        let path = delta.path;
        diesel::insert_into(site_entries::table)
            .values((
                site_entries::site_id.eq(site_id),
                site_entries::path.eq(&path),
                site_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
            ))
            .execute(tx)?;
        diesel::insert_into(allocated_entries::table)
            .values((
                allocated_entries::site_id.eq(site_id),
                allocated_entries::path.eq(&path),
                allocated_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                allocated_entries::hash.eq(delta.hash.expect("existing allocated delta has hash")),
                allocated_entries::size.eq(delta.size.expect("existing allocated delta has size")),
                allocated_entries::naming_mode.eq(delta
                    .naming_mode
                    .expect("existing allocated delta has naming mode")),
                allocated_entries::prefix
                    .eq(delta.prefix.expect("existing allocated delta has prefix")),
                allocated_entries::suffix
                    .eq(delta.suffix.expect("existing allocated delta has suffix")),
                allocated_entries::extension.eq(delta.extension),
                allocated_entries::media_type.eq(delta
                    .media_type
                    .expect("existing allocated delta has media type")),
            ))
            .execute(tx)?;
    }
    for delta in alias_deltas {
        if delta.existed == 0 {
            continue;
        }
        let path = delta.path;
        diesel::insert_into(site_entries::table)
            .values((
                site_entries::site_id.eq(site_id),
                site_entries::path.eq(&path),
                site_entries::kind.eq(database::schema::ALIAS_ENTRY_KIND),
            ))
            .execute(tx)?;
        diesel::insert_into(aliases::table)
            .values((
                aliases::site_id.eq(site_id),
                aliases::path.eq(&path),
                aliases::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                aliases::canonical_target.eq(delta
                    .canonical_target
                    .expect("existing alias delta has target")),
                aliases::resolved_kind.eq(delta.resolved_kind),
                aliases::resolved_hash.eq(delta.resolved_hash),
                aliases::resolved_size.eq(delta.resolved_size),
            ))
            .execute(tx)?;
    }
    diesel::delete(expiry_policies::table.filter(expiry_policies::site_id.eq(site_id)))
        .execute(tx)?;
    restore_expiry_policies_locked(tx, token, site_id)?;
    rebuild_aggregates_locked(tx, site_id)?;
    diesel::update(sites::table.find(site_id))
        .set(sites::content_revision.eq(content_revision))
        .execute(tx)?;
    let alias_changes = entry_change_paths
        .iter()
        .map(|path| AliasChange::Entry(path))
        .chain(
            alias_change_paths
                .iter()
                .map(|path| AliasChange::Alias(path)),
        )
        .collect::<Vec<_>>();
    refresh_aliases_locked(tx, site_id, &alias_changes)?;
    regenerate_site(tx, blob_files, site_id, updated)?;
    record_site_event(tx, site_id, StoredSiteEventKind::Restore, 0, updated)?;
    Ok(updated)
}

pub(super) fn retain_management_tombstone(
    tx: &mut SqliteConnection,
    name: &str,
    now: i64,
) -> Result<(), diesel::result::Error> {
    let hash = sites::table
        .filter(sites::name.eq(name))
        .filter(sites::management_status.eq(1_i64))
        .select(sites::management_hash)
        .first::<Option<Vec<u8>>>(tx)
        .optional()?
        .flatten();
    if let Some(hash) = hash {
        diesel::insert_into(management_tombstones::table)
            .values((
                management_tombstones::name.eq(name),
                management_tombstones::management_hash.eq(hash),
                management_tombstones::created.eq(now),
            ))
            .on_conflict(management_tombstones::name)
            .do_update()
            .set((
                management_tombstones::management_hash
                    .eq(excluded(management_tombstones::management_hash)),
                management_tombstones::created.eq(excluded(management_tombstones::created)),
            ))
            .execute(tx)?;
    }
    Ok(())
}

pub(super) fn prune_undo_locked(
    tx: &mut SqliteConnection,
    now: i64,
) -> Result<(), diesel::result::Error> {
    diesel::delete(
        undo_operations::table.filter(
            undo_operations::consumed
                .eq(1_i64)
                .or(undo_operations::expires.le(now)),
        ),
    )
    .execute(tx)?;
    let rows = undo_names::table
        .inner_join(undo_operations::table.on(undo_operations::token.eq(undo_names::token)))
        .select((undo_names::name, undo_names::token))
        .order((
            undo_names::name,
            undo_operations::created.desc(),
            undo_operations::rowid.desc(),
        ))
        .load::<(String, String)>(tx)?;
    let mut previous = None::<String>;
    let mut position = 0_i64;
    let mut stale = Vec::new();
    for (name, token) in rows {
        if previous.as_deref() == Some(name.as_str()) {
            position += 1;
        } else {
            previous = Some(name);
            position = 1;
        }
        if position > UNDO_LIMIT_PER_SITE {
            stale.push(token);
        }
    }
    stale.sort_unstable();
    stale.dedup();
    for tokens in stale.chunks(SQLITE_DELETE_BATCH_SIZE) {
        diesel::delete(undo_operations::table.filter(undo_operations::token.eq_any(tokens)))
            .execute(tx)?;
    }
    Ok(())
}

pub(super) fn undo_token() -> Result<String, StoreError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(token, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(token)
}

impl Store {
    pub fn undo_stack(&self, name: &str) -> Result<UndoStack, StoreError> {
        let name = parse_site_name(name)?;
        let now = self.now_millis();
        let mut db = self.inner.readers.get();
        let rows = undo_operations::table
            .inner_join(undo_names::table.on(undo_names::token.eq(undo_operations::token)))
            .filter(undo_names::name.eq(name))
            .filter(undo_operations::consumed.eq(0_i64))
            .filter(undo_operations::expires.gt(now))
            .select((
                undo_operations::token,
                undo_operations::kind,
                undo_operations::description,
                undo_operations::created,
                undo_operations::expires,
            ))
            .order((
                undo_operations::created.desc(),
                undo_operations::rowid.desc(),
            ))
            .load::<(String, i64, String, i64, i64)>(&mut *db)?;
        let entries = rows
            .into_iter()
            .map(|(token, kind, description, created, expires)| {
                Ok(UndoEntry {
                    token,
                    kind: UndoKind::from_i64(kind)?.as_str().to_string(),
                    description,
                    created_at: format_timestamp(created),
                    expires_at: format_timestamp(expires),
                    remaining_seconds: u64::try_from((expires - now).max(0) / 1000)
                        .expect("remaining time is non-negative"),
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(UndoStack {
            site: name.to_string(),
            entries,
        })
    }

    #[cfg(test)]
    pub fn undo(&self, name: &str, guard: Option<&str>) -> Result<UndoResult, StoreError> {
        self.undo_secured(name, guard, None)
    }

    #[expect(clippy::too_many_lines)]
    pub fn undo_secured(
        &self,
        name: &str,
        guard: Option<&str>,
        authorization: Option<&ManagementToken>,
    ) -> Result<UndoResult, StoreError> {
        let name = parse_site_name(name)?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, authorization)?;
        let latest = undo_operations::table
            .inner_join(undo_names::table.on(undo_names::token.eq(undo_operations::token)))
            .filter(undo_names::name.eq(name))
            .filter(undo_operations::consumed.eq(0_i64))
            .filter(undo_operations::expires.gt(now))
            .select(undo_operations::token)
            .order((
                undo_operations::created.desc(),
                undo_operations::rowid.desc(),
            ))
            .first::<String>(&mut *tx)
            .optional()?
            .ok_or(StoreError::NotFound)?;
        if guard.is_some_and(|token| token != latest) {
            return Err(StoreError::StaleUndo(latest.into()));
        }
        let latest_kind = UndoKind::from_i64(
            undo_operations::table
                .find(&latest)
                .select(undo_operations::kind)
                .first::<i64>(&mut *tx)?,
        )?;
        let has_deltas = matches!(
            latest_kind,
            UndoKind::Put
                | UndoKind::PutFile
                | UndoKind::DeletePath
                | UndoKind::Allocate
                | UndoKind::Replace
                | UndoKind::Splice
                | UndoKind::Alias
        ) && (undo_file_deltas::table
            .filter(undo_file_deltas::token.eq(&latest))
            .select(count_star())
            .first::<i64>(&mut *tx)?
            + undo_allocated_deltas::table
                .filter(undo_allocated_deltas::token.eq(&latest))
                .select(count_star())
                .first::<i64>(&mut *tx)?
            + undo_alias_deltas::table
                .filter(undo_alias_deltas::token.eq(&latest))
                .select(count_star())
                .first::<i64>(&mut *tx)?
            > 0);
        if has_deltas {
            let restored = restore_entry_deltas(&mut tx, &self.inner.blob_files, name, &latest)?;
            diesel::update(undo_operations::table.find(&latest))
                .set(undo_operations::consumed.eq(1_i64))
                .execute(&mut *tx)?;
            prune_undo_locked(&mut tx, now)?;
            let removed = gc_blobs(&mut tx, now)?;
            tx.commit()?;
            drop(db);
            self.remove_blob_files(&removed);
            return Ok(UndoResult {
                restored_at: format_timestamp(restored),
            });
        }
        let (snapshot_name, existed, public_url, created, updated, content_revision, tree_hash) =
            undo_sites::table
                .find(&latest)
                .select((
                    undo_sites::name,
                    undo_sites::existed,
                    undo_sites::public_url,
                    undo_sites::created,
                    undo_sites::updated,
                    undo_sites::content_revision,
                    undo_sites::tree_hash,
                ))
                .first::<(String, i64, String, Option<i64>, i64, i64, TreeHash)>(&mut *tx)?;
        let snapshot = SiteSnapshot {
            name: snapshot_name,
            existed: existed != 0,
            public_url,
            created,
            updated,
            content_revision,
            tree_hash,
        };
        let names = undo_names::table
            .filter(undo_names::token.eq(&latest))
            .select(undo_names::name)
            .load::<String>(&mut *tx)?;
        for name in names {
            retain_management_tombstone(&mut tx, &name, now)?;
            diesel::delete(sites::table.filter(sites::name.eq(name))).execute(&mut *tx)?;
        }
        if snapshot.existed {
            let site_id = diesel::insert_into(sites::table)
                .values(NewSite {
                    name: &snapshot.name,
                    created: snapshot.created,
                    updated: snapshot.updated,
                    public_url: &snapshot.public_url,
                    content_revision: snapshot.content_revision,
                    tree_hash: snapshot.tree_hash,
                    creator_kind: None,
                    creator_hash: None,
                    claim_hash: None,
                    management_hash: None,
                    management_status: 0,
                })
                .returning(sites::id)
                .get_result::<i64>(&mut *tx)?;
            let retained_hash = management_tombstones::table
                .inner_join(undo_names::table.on(undo_names::name.eq(management_tombstones::name)))
                .filter(undo_names::token.eq(&latest))
                .select(management_tombstones::management_hash)
                .first::<Vec<u8>>(&mut *tx)
                .optional()?;
            if let Some(hash) = retained_hash {
                diesel::update(sites::table.find(site_id))
                    .set((
                        sites::management_hash.eq(Some(hash)),
                        sites::management_status.eq(1_i64),
                    ))
                    .execute(&mut *tx)?;
            }
            let saved_files = undo_files::table
                .filter(undo_files::token.eq(&latest))
                .select((undo_files::path, undo_files::hash, undo_files::size))
                .load::<(String, ContentHash, i64)>(&mut *tx)?;
            for (path, hash, size) in saved_files {
                ensure_file_entry(&mut tx, site_id, &path)?;
                diesel::insert_into(files::table)
                    .values(NewFile {
                        site_id,
                        path,
                        hash,
                        size,
                    })
                    .execute(&mut *tx)?;
            }
            let saved_allocated = undo_allocated_deltas::table
                .filter(undo_allocated_deltas::token.eq(&latest))
                .filter(undo_allocated_deltas::existed.eq(1_i64))
                .select((
                    undo_allocated_deltas::path,
                    undo_allocated_deltas::hash,
                    undo_allocated_deltas::size,
                    undo_allocated_deltas::naming_mode,
                    undo_allocated_deltas::prefix,
                    undo_allocated_deltas::suffix,
                    undo_allocated_deltas::extension,
                    undo_allocated_deltas::media_type,
                ))
                .load::<(
                    String,
                    Option<ContentHash>,
                    Option<i64>,
                    Option<i64>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                )>(&mut *tx)?;
            for (path, hash, size, naming_mode, prefix, suffix, extension, media_type) in
                saved_allocated
            {
                diesel::insert_into(site_entries::table)
                    .values((
                        site_entries::site_id.eq(site_id),
                        site_entries::path.eq(&path),
                        site_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                    ))
                    .execute(&mut *tx)?;
                diesel::insert_into(allocated_entries::table)
                    .values(
                        (
                            allocated_entries::site_id.eq(site_id),
                            allocated_entries::path.eq(path),
                            allocated_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                            allocated_entries::hash
                                .eq(hash.expect("whole-site allocated snapshot has hash")),
                            allocated_entries::size
                                .eq(size.expect("whole-site allocated snapshot has size")),
                            allocated_entries::naming_mode
                                .eq(naming_mode
                                    .expect("whole-site allocated snapshot has naming mode")),
                            allocated_entries::prefix
                                .eq(prefix.expect("whole-site allocated snapshot has prefix")),
                            allocated_entries::suffix
                                .eq(suffix.expect("whole-site allocated snapshot has suffix")),
                            allocated_entries::extension.eq(extension),
                            allocated_entries::media_type
                                .eq(media_type
                                    .expect("whole-site allocated snapshot has media type")),
                        ),
                    )
                    .execute(&mut *tx)?;
            }
            let saved_aliases = undo_alias_deltas::table
                .filter(undo_alias_deltas::token.eq(&latest))
                .filter(undo_alias_deltas::existed.eq(1_i64))
                .select(UndoAliasRow::as_select())
                .load::<UndoAliasRow>(&mut *tx)?;
            for alias in saved_aliases {
                diesel::insert_into(site_entries::table)
                    .values((
                        site_entries::site_id.eq(site_id),
                        site_entries::path.eq(&alias.path),
                        site_entries::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                    ))
                    .execute(&mut *tx)?;
                diesel::insert_into(aliases::table)
                    .values((
                        aliases::site_id.eq(site_id),
                        aliases::path.eq(alias.path),
                        aliases::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                        aliases::canonical_target.eq(alias
                            .canonical_target
                            .expect("whole-site alias snapshot has target")),
                        aliases::resolved_kind.eq(alias.resolved_kind),
                        aliases::resolved_hash.eq(alias.resolved_hash),
                        aliases::resolved_size.eq(alias.resolved_size),
                    ))
                    .execute(&mut *tx)?;
            }
            restore_expiry_policies_locked(&mut tx, &latest, site_id)?;
            rebuild_aggregates_locked(&mut tx, site_id)?;
            regenerate_site(&mut tx, &self.inner.blob_files, site_id, snapshot.updated)?;
            record_site_event(
                &mut tx,
                site_id,
                StoredSiteEventKind::Restore,
                0,
                snapshot.updated,
            )?;
            let tombstone_names = undo_names::table
                .filter(undo_names::token.eq(&latest))
                .select(undo_names::name)
                .load::<String>(&mut *tx)?;
            for names in tombstone_names.chunks(SQLITE_DELETE_BATCH_SIZE) {
                diesel::delete(
                    management_tombstones::table.filter(management_tombstones::name.eq_any(names)),
                )
                .execute(&mut *tx)?;
            }
        }
        diesel::update(undo_operations::table.find(&latest))
            .set(undo_operations::consumed.eq(1_i64))
            .execute(&mut *tx)?;
        prune_undo_locked(&mut tx, now)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(UndoResult {
            restored_at: format_timestamp(snapshot.updated),
        })
    }

    #[cfg(test)]
    pub(super) fn prune_undo_and_gc(&self) -> Result<(), StoreError> {
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        prune_undo_locked(&mut tx, now)?;
        prune_idempotency_locked(&mut tx, now)?;
        prune_pending_locked(&mut tx, now)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(())
    }
}
