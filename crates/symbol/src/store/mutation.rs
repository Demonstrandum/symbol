// One slice of `store`: it shares the parent's imports and types rather
// than re-listing ~90 names. `allow`, not `expect`, because clippy
// reports an expectation on a `use` item as unfulfilled.
#[allow(clippy::wildcard_imports)]
use super::*;

pub(super) enum PreparedSpliceSource {
    Empty,
    #[cfg(test)]
    Bytes(Vec<u8>),
    File {
        path: PathBuf,
        size: u64,
        hash: ContentHash,
    },
}

pub(super) struct PreparedSplice {
    offset: u64,
    delete: u64,
    insert: PreparedSpliceSource,
    sanitized: TokenCounts,
}

pub fn validate_mutation_target(path: &str) -> Result<(), StoreError> {
    let path = normalize_folder(path)?;
    reject_reserved_path(&path)
}

pub(super) fn ensure_blob_locked(
    tx: &mut SqliteConnection,
    staged: &StagedFile,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(blobs::table)
        .values((
            blobs::hash.eq(staged.hash),
            blobs::bytes.eq(Vec::<u8>::new()),
            blobs::size.eq(staged.size),
        ))
        .on_conflict_do_nothing()
        .execute(tx)?;
    Ok(())
}

pub(super) fn check_tree_precondition(
    tx: &mut SqliteConnection,
    name: &str,
    expected: Option<&str>,
) -> Result<(), StoreError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let expected = TreeHash::try_from(expected)?;
    let (revision, tree_hash) = site_revision_locked(tx, name)?;
    if expected == tree_hash {
        Ok(())
    } else {
        Err(StoreError::PreconditionFailed {
            revision,
            tree_hash,
        })
    }
}

pub(super) fn entry_hash_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    path: &str,
    kind: i64,
) -> Result<ContentHash, StoreError> {
    if kind == database::schema::FILE_ENTRY_KIND {
        files::table
            .find((site_id, path))
            .select(files::hash)
            .first::<ContentHash>(db)
            .map_err(map_sql)
    } else if kind == database::schema::ALLOCATED_ENTRY_KIND {
        allocated_entries::table
            .find((site_id, path))
            .select(allocated_entries::hash)
            .first::<ContentHash>(db)
            .map_err(map_sql)
    } else {
        Err(StoreError::NotFound)
    }
}

pub(super) fn entry_size_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    path: &str,
    kind: i64,
) -> Result<u64, StoreError> {
    let size = if kind == database::schema::FILE_ENTRY_KIND {
        files::table
            .find((site_id, path))
            .select(files::size)
            .first::<i64>(db)?
    } else if kind == database::schema::ALLOCATED_ENTRY_KIND {
        allocated_entries::table
            .find((site_id, path))
            .select(allocated_entries::size)
            .first::<i64>(db)?
    } else if kind == database::schema::ALIAS_ENTRY_KIND {
        aliases::table
            .find((site_id, path))
            .select(aliases::site_id)
            .first::<i64>(db)?;
        0
    } else {
        return Err(StoreError::NotFound);
    };
    Ok(size.cast_unsigned())
}

pub(super) fn finish_entry_mutation(
    tx: &mut SqliteConnection,
    blob_files: &BlobFiles,
    site_id: i64,
    paths: &[&str],
    now: i64,
) -> Result<(), StoreError> {
    diesel::update(sites::table.find(site_id))
        .set((
            sites::updated.eq(now),
            sites::content_revision.eq(sites::content_revision + 1),
        ))
        .execute(tx)?;
    record_site_event(tx, site_id, StoredSiteEventKind::Publish, paths.len(), now)?;
    let alias_changes = paths
        .iter()
        .map(|path| AliasChange::Entry(path))
        .collect::<Vec<_>>();
    refresh_aliases_locked(tx, site_id, &alias_changes)?;
    refresh_expiry_for_changes_locked(tx, site_id, paths, now)?;
    regenerate_site(tx, blob_files, site_id, now)?;
    prune_undo_locked(tx, now)?;
    Ok(())
}

pub(super) fn prepare_splices(
    splices: &[Splice<'_>],
    temporary: &Path,
    maximum_staged_bytes: u64,
) -> Result<Vec<PreparedSplice>, StoreError> {
    let mut prepared = Vec::with_capacity(splices.len());
    let mut staged_bytes = 0_u64;
    for (index, splice) in splices.iter().enumerate() {
        let (insert, sanitized) = match splice.insert {
            SpliceSource::Empty => (PreparedSpliceSource::Empty, TokenCounts::default()),
            #[cfg(test)]
            SpliceSource::Bytes(bytes) => {
                let redacted = sanitize::redact_tokens(bytes);
                let bytes = redacted.as_bytes();
                staged_bytes = staged_bytes
                    .checked_add(u64::try_from(bytes.len()).expect("slice length fits in u64"))
                    .ok_or(StoreError::SpliceResultTooLarge)?;
                if staged_bytes > maximum_staged_bytes {
                    return Err(StoreError::SpliceResultTooLarge);
                }
                (
                    PreparedSpliceSource::Bytes(bytes.to_vec()),
                    redacted.counts(),
                )
            }
            SpliceSource::File(path) => {
                let copied = temporary.join(format!("insert-{index}"));
                let mut source = fs::File::open(path)?;
                let mut target = fs::File::create(&copied)?;
                let remaining = maximum_staged_bytes
                    .checked_sub(staged_bytes)
                    .ok_or(StoreError::SpliceResultTooLarge)?;
                let mut limited = (&mut source).take(remaining.saturating_add(1));
                let size = io::copy(&mut limited, &mut target)?;
                if size > remaining {
                    return Err(StoreError::SpliceResultTooLarge);
                }
                staged_bytes += size;
                target.sync_all()?;
                drop(target);
                let (sanitized_size, hash, sanitized) = sanitized_file_properties(&copied)?;
                let sanitized_size = sanitized_size.cast_unsigned();
                debug_assert_eq!(sanitized_size, size);
                (
                    PreparedSpliceSource::File {
                        path: copied,
                        size: sanitized_size,
                        hash,
                    },
                    sanitized,
                )
            }
        };
        prepared.push(PreparedSplice {
            offset: splice.offset,
            delete: splice.delete,
            insert,
            sanitized,
        });
    }
    Ok(prepared)
}

pub(super) fn splice_request_fingerprint(
    site: &str,
    path: &str,
    base_hash: &str,
    splices: &[PreparedSplice],
    expected_tree_hash: Option<&str>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-splice-request-v1\0");
    for value in [site, path, base_hash] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(expected_tree_hash.unwrap_or("").as_bytes());
    for splice in splices {
        hasher.update(&splice.offset.to_le_bytes());
        hasher.update(&splice.delete.to_le_bytes());
        match &splice.insert {
            PreparedSpliceSource::Empty => {
                hasher.update(&[0]);
            }
            #[cfg(test)]
            PreparedSpliceSource::Bytes(bytes) => {
                hasher.update(&[1]);
                hasher.update(&(bytes.len() as u64).to_le_bytes());
                hasher.update(blake3::hash(bytes).as_bytes());
            }
            PreparedSpliceSource::File { size, hash, .. } => {
                hasher.update(&[2]);
                hasher.update(&size.to_le_bytes());
                hasher.update(hash.to_hex().as_bytes());
            }
        }
    }
    hasher.finalize().to_hex().to_string()
}

pub(super) fn splice_blob_to_path(
    source: &Path,
    old_size: u64,
    splices: &[PreparedSplice],
    output: &Path,
    maximum_result_size: u64,
) -> Result<(String, u64), StoreError> {
    let mut previous_end = 0_u64;
    let mut result_size = old_size;
    for splice in splices {
        let end = splice
            .offset
            .checked_add(splice.delete)
            .ok_or(StoreError::SpliceRange)?;
        if splice.offset < previous_end {
            return Err(StoreError::InvalidSpliceOrder);
        }
        if splice.offset > old_size || end > old_size {
            return Err(StoreError::SpliceRange);
        }
        previous_end = end;
        let insertion_size = match &splice.insert {
            PreparedSpliceSource::Empty => 0,
            #[cfg(test)]
            PreparedSpliceSource::Bytes(bytes) => {
                u64::try_from(bytes.len()).expect("slice length fits in u64")
            }
            PreparedSpliceSource::File { size, .. } => *size,
        };
        result_size = result_size
            .checked_sub(splice.delete)
            .and_then(|size| size.checked_add(insertion_size))
            .ok_or(StoreError::SpliceResultTooLarge)?;
        if result_size > maximum_result_size {
            return Err(StoreError::SpliceResultTooLarge);
        }
    }
    let mut input = fs::File::open(source)?;
    let mut output = fs::File::create(output)?;
    let mut hasher = blake3::Hasher::new();
    let mut cursor = 0_u64;
    for splice in splices {
        copy_hashed(&mut input, &mut output, &mut hasher, splice.offset - cursor)?;
        input.seek(SeekFrom::Current(
            i64::try_from(splice.delete).map_err(|_| StoreError::SpliceRange)?,
        ))?;
        match &splice.insert {
            PreparedSpliceSource::Empty => {}
            #[cfg(test)]
            PreparedSpliceSource::Bytes(bytes) => {
                output.write_all(bytes)?;
                hasher.update(bytes);
            }
            PreparedSpliceSource::File { path, .. } => {
                let mut insertion = fs::File::open(path)?;
                copy_all_hashed(&mut insertion, &mut output, &mut hasher)?;
            }
        }
        cursor = splice.offset + splice.delete;
    }
    copy_hashed(&mut input, &mut output, &mut hasher, old_size - cursor)?;
    output.sync_all()?;
    Ok((hasher.finalize().to_hex().to_string(), result_size))
}

pub(super) fn copy_hashed(
    input: &mut fs::File,
    output: &mut fs::File,
    hasher: &mut blake3::Hasher,
    mut remaining: u64,
) -> io::Result<()> {
    let mut buffer = [0_u8; 64 * 1024];
    while remaining != 0 {
        let wanted = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded read size fits in usize");
        input.read_exact(&mut buffer[..wanted])?;
        output.write_all(&buffer[..wanted])?;
        hasher.update(&buffer[..wanted]);
        remaining -= u64::try_from(wanted).expect("buffer size fits in u64");
    }
    Ok(())
}

pub(super) fn copy_all_hashed(
    input: &mut impl Read,
    output: &mut fs::File,
    hasher: &mut blake3::Hasher,
) -> io::Result<u64> {
    let mut buffer = [0_u8; 64 * 1024];
    let mut size = 0_u64;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            return Ok(size);
        }
        output.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        size += u64::try_from(read).expect("read size fits in u64");
    }
}

pub(super) fn reject_reserved_path(path: &str) -> Result<(), StoreError> {
    if is_reserved_path(path) || is_virtual_namespace(path) {
        Err(StoreError::Upload(UploadError::ReservedPath))
    } else {
        Ok(())
    }
}

pub(super) fn is_virtual_namespace(path: &str) -> bool {
    path.split('/')
        .next()
        .is_some_and(|component| matches!(component, "FILES" | "UNDO" | "EXPIRES"))
}

pub(super) fn is_reserved_path(path: &str) -> bool {
    let terminal = path.rsplit('/').next().unwrap_or(path);
    RESERVED_TERMINALS.contains(&terminal)
}

pub(super) fn validate_idempotency_key(key: &str) -> Result<(), StoreError> {
    if key.is_empty()
        || key.len() > 256
        || !key
            .as_bytes()
            .iter()
            .all(|byte| (0x21..=0x7e).contains(byte))
    {
        return Err(StoreError::InvalidIdempotencyKey);
    }
    Ok(())
}

pub(super) fn idempotency_key_hash(key: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-idempotency-v1\0");
    hasher.update(key.as_bytes());
    hasher.finalize().to_hex().to_string()
}

pub(super) fn staged_entries_fingerprint(
    files: &[&StagedFile],
    archive_aliases: &[ArchiveAlias<'_>],
) -> String {
    let mut files = files.to_vec();
    files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-unnamed-put-v1\0");
    for file in files {
        hasher.update(&(file.path.len() as u64).to_le_bytes());
        hasher.update(file.path.as_bytes());
        hasher.update(file.hash.as_ref());
    }
    for alias in archive_aliases {
        hasher.update(b"\0alias\0");
        hasher.update(&(alias.path.len() as u64).to_le_bytes());
        hasher.update(alias.path.as_bytes());
        hasher.update(&(alias.target.len() as u64).to_le_bytes());
        hasher.update(alias.target.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

pub(super) fn sanitized_counts(files: &[&StagedFile]) -> TokenCounts {
    files
        .iter()
        .fold(TokenCounts::default(), |mut total, file| {
            total.management += file.sanitized.management;
            total.claim += file.sanitized.claim;
            total
        })
}

pub(super) fn idempotency_replay(
    tx: &mut SqliteConnection,
    key: &str,
    fingerprint: &str,
    kind: IdempotencyKind,
) -> Result<Option<PublishedMutation>, StoreError> {
    let key_hash = idempotency_key_hash(key);
    let record = idempotency_records::table
        .find(key_hash)
        .select((
            idempotency_records::fingerprint,
            idempotency_records::operation_kind,
            idempotency_records::result_metadata,
        ))
        .first::<(String, i64, String)>(tx)
        .optional()?;
    let Some((stored_fingerprint, stored_kind, metadata)) = record else {
        return Ok(None);
    };
    if stored_fingerprint != fingerprint || stored_kind != kind as i64 {
        return Err(StoreError::IdempotencyConflict);
    }
    let mut replay: PublishedMutation = serde_json::from_str(&metadata)
        .map_err(|err| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, err)))?;
    replay.mutation.replayed = true;
    Ok(Some(replay))
}

pub(super) fn store_idempotency(
    tx: &mut SqliteConnection,
    key: &str,
    fingerprint: &str,
    kind: IdempotencyKind,
    result: &PublishedMutation,
    now: i64,
) -> Result<(), StoreError> {
    let metadata = serde_json::to_string(result)
        .map_err(|err| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, err)))?;
    diesel::insert_into(idempotency_records::table)
        .values((
            idempotency_records::key_hash.eq(idempotency_key_hash(key)),
            idempotency_records::fingerprint.eq(fingerprint),
            idempotency_records::operation_kind.eq(kind as i64),
            idempotency_records::result_metadata.eq(metadata),
            idempotency_records::expires.eq(now + IDEMPOTENCY_RETENTION_MILLIS),
        ))
        .execute(tx)?;
    Ok(())
}

pub(super) fn entry_mutation_fingerprint(
    name: &str,
    current_path: Option<&str>,
    destination: &str,
    hash: ContentHash,
    kind: UndoKind,
    expected_tree_hash: Option<&str>,
) -> String {
    let hash_hex = hash.to_hex();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-entry-mutation-v1\0");
    hasher.update(name.as_bytes());
    hasher.update(&[0]);
    hasher.update(current_path.unwrap_or("").as_bytes());
    hasher.update(&[0]);
    hasher.update(destination.as_bytes());
    hasher.update(&[0]);
    hasher.update(hash_hex.as_bytes());
    hasher.update(expected_tree_hash.unwrap_or("").as_bytes());
    hasher.update(&(kind as i64).to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

pub(super) fn content_mutation_request_fingerprint(
    name: &str,
    current_path: &str,
    hash: ContentHash,
    expected_content_hash: Option<&str>,
    expected_tree_hash: Option<&str>,
    kind: UndoKind,
) -> String {
    let hash_hex = hash.to_hex();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-content-mutation-request-v1\0");
    for value in [
        name,
        current_path,
        hash_hex.as_str(),
        expected_content_hash.unwrap_or(""),
        expected_tree_hash.unwrap_or(""),
    ] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(&(kind as i64).to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

pub(super) fn entry_mutation_replay(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
) -> Result<Option<AllocatedFile>, StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(None);
    };
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
    let Some((stored_fingerprint, stored_kind, metadata)) = record else {
        return Ok(None);
    };
    if stored_fingerprint != fingerprint || stored_kind != IdempotencyKind::EntryMutation as i64 {
        return Err(StoreError::IdempotencyConflict);
    }
    let mut record: EntryMutationRecord = serde_json::from_str(&metadata)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    record.result.replayed = true;
    if let Some(mutation) = &mut record.result.mutation {
        mutation.replayed = true;
    }
    Ok(Some(record.result))
}

pub(super) fn entry_mutation_request_replay(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    request_fingerprint: &str,
) -> Result<Option<AllocatedFile>, StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(None);
    };
    validate_idempotency_key(&idempotency.key)?;
    let record = idempotency_records::table
        .find(idempotency_key_hash(&idempotency.key))
        .select((
            idempotency_records::operation_kind,
            idempotency_records::result_metadata,
        ))
        .first::<(i64, String)>(tx)
        .optional()?;
    let Some((stored_kind, metadata)) = record else {
        return Ok(None);
    };
    if stored_kind != IdempotencyKind::EntryMutation as i64 {
        return Err(StoreError::IdempotencyConflict);
    }
    let mut record: EntryMutationRecord = serde_json::from_str(&metadata)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    if record.request_fingerprint != request_fingerprint {
        return Err(StoreError::IdempotencyConflict);
    }
    record.result.replayed = true;
    if let Some(mutation) = &mut record.result.mutation {
        mutation.replayed = true;
    }
    Ok(Some(record.result))
}

pub(super) fn store_entry_mutation(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    result: &AllocatedFile,
    now: i64,
) -> Result<(), StoreError> {
    store_entry_mutation_with_request(tx, idempotency, fingerprint, fingerprint, result, now)
}

pub(super) fn store_entry_mutation_with_request(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    request_fingerprint: &str,
    result: &AllocatedFile,
    now: i64,
) -> Result<(), StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(());
    };
    let metadata = serde_json::to_string(&EntryMutationRecord {
        result: result.clone(),
        request_fingerprint: request_fingerprint.to_string(),
    })
    .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    diesel::insert_into(idempotency_records::table)
        .values((
            idempotency_records::key_hash.eq(idempotency_key_hash(&idempotency.key)),
            idempotency_records::fingerprint.eq(fingerprint),
            idempotency_records::operation_kind.eq(IdempotencyKind::EntryMutation as i64),
            idempotency_records::result_metadata.eq(metadata),
            idempotency_records::expires.eq(now + IDEMPOTENCY_RETENTION_MILLIS),
        ))
        .execute(tx)?;
    Ok(())
}

pub(super) fn prune_idempotency_locked(
    tx: &mut SqliteConnection,
    now: i64,
) -> Result<(), diesel::result::Error> {
    diesel::delete(idempotency_records::table.filter(idempotency_records::expires.le(now)))
        .execute(tx)?;
    Ok(())
}

#[expect(clippy::too_many_lines)]
pub(super) fn regenerate_site(
    tx: &mut SqliteConnection,
    blobs: &BlobFiles,
    site_id: i64,
    updated: i64,
) -> Result<TreeHash, StoreError> {
    let (name, public_url, revision, management_status) = sites::table
        .find(site_id)
        .select((
            sites::name,
            sites::public_url,
            sites::content_revision,
            sites::management_status,
        ))
        .first::<(String, String, i64, i64)>(tx)?;
    let managed = management_status != 0;
    let mut entries = files::table
        .filter(files::site_id.eq(site_id))
        .filter(files::path.ne(MANIFEST_PATH))
        .select((files::path, files::hash))
        .order(files::path)
        .load::<(String, ContentHash)>(tx)?;
    let allocated = allocated_entries::table
        .filter(allocated_entries::site_id.eq(site_id))
        .select((allocated_entries::path, allocated_entries::hash))
        .load::<(String, ContentHash)>(tx)?;
    for (path, hash) in allocated {
        entries.push((path, hash));
    }
    entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = blake3::Hasher::new();
    for (path, hash) in &entries {
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(hash.to_hex().as_bytes());
    }
    let alias_entries = aliases::table
        .filter(aliases::site_id.eq(site_id))
        .select((aliases::path, aliases::canonical_target))
        .order(aliases::path)
        .load::<(String, String)>(tx)?;
    for (path, target) in &alias_entries {
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(&(target.len() as u64).to_le_bytes());
        hasher.update(target.as_bytes());
    }
    let tree_hash = TreeHash::from(hasher.finalize());
    let tree_hash_wire = tree_hash.to_wire();
    let mut manifest = format!(
        "version = 1\nhost = \"{}\"\nname = \"{}\"\nmanaged = {managed}\ncontent_revision = {}\ntree_hash = \"{}\"\n\n[files]\n",
        toml_escape(&public_url),
        toml_escape(&name),
        revision,
        tree_hash_wire
    );
    for (path, hash) in &entries {
        writeln!(
            manifest,
            "\"{}\" = \"blake3:{}\"",
            toml_escape(path),
            hash.to_hex()
        )
        .expect("writing to String cannot fail");
    }
    if !alias_entries.is_empty() {
        manifest.push_str("\n[aliases]\n");
        for (path, target) in &alias_entries {
            writeln!(
                manifest,
                "\"{}\" = \"{}\"",
                toml_escape(path),
                toml_escape(target)
            )
            .expect("writing to String cannot fail");
        }
    }
    let expiry_paths = expiry_policies::table
        .filter(expiry_policies::site_id.eq(site_id))
        .select(expiry_policies::path)
        .order((expiry_policies::target_kind, expiry_policies::path))
        .load::<String>(tx)?;
    for path in expiry_paths {
        let stored = load_expiry_policy_locked(tx, site_id, &path)?
            .expect("selected expiry policy still exists");
        let table = match stored.kind {
            ExpiryTargetKind::Site => "[expiry.site]".to_string(),
            ExpiryTargetKind::Folder => {
                format!("[expiry.folders.\"{}\"]", toml_escape(&path))
            }
            ExpiryTargetKind::File => format!("[expiry.files.\"{}\"]", toml_escape(&path)),
        };
        write!(
            manifest,
            "\n{table}\nmode = \"{}\"\nexpires_at = \"{}\"\n",
            expiry_mode_name(stored.policy.mode()),
            format_timestamp(stored.own_deadline_millis),
        )
        .expect("writing to String cannot fail");
        match stored.policy {
            ExpiryPolicy::Relative { duration_seconds } => {
                writeln!(manifest, "duration_seconds = {duration_seconds}")
                    .expect("writing to String cannot fail");
            }
            ExpiryPolicy::Absolute { .. } => {}
            ExpiryPolicy::Decay(policy) => {
                write!(
                    manifest,
                    "min_age_seconds = {}\nmax_age_seconds = {}\nmax_size_bytes = {}\npower = {}\n",
                    policy.min_age_seconds,
                    policy.max_age_seconds,
                    policy.max_size_bytes,
                    policy.power,
                )
                .expect("writing to String cannot fail");
            }
        }
    }
    let staged = stage_bytes(MANIFEST_PATH, manifest.as_bytes());
    blobs.put_bytes(staged.hash, manifest.as_bytes())?;
    diesel::insert_into(blobs::table)
        .values((
            blobs::hash.eq(staged.hash),
            blobs::bytes.eq(Vec::<u8>::new()),
            blobs::size.eq(staged.size),
        ))
        .on_conflict_do_nothing()
        .execute(tx)?;
    ensure_file_entry(tx, site_id, MANIFEST_PATH)?;
    diesel::insert_into(files::table)
        .values(NewFile {
            site_id,
            path: MANIFEST_PATH.to_string(),
            hash: staged.hash,
            size: staged.size,
        })
        .on_conflict((files::site_id, files::path))
        .do_update()
        .set((
            files::hash.eq(excluded(files::hash)),
            files::size.eq(excluded(files::size)),
        ))
        .execute(tx)?;
    diesel::update(sites::table.find(site_id))
        .set((sites::tree_hash.eq(tree_hash), sites::updated.eq(updated)))
        .execute(tx)?;
    Ok(tree_hash)
}

pub(super) fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(super) fn stage_dir(dir: &Path) -> io::Result<Vec<StagedFile>> {
    let mut files = Vec::new();
    collect_stage(dir, dir, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

pub(super) fn collect_stage(base: &Path, dir: &Path, out: &mut Vec<StagedFile>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            collect_stage(base, &entry.path(), out)?;
        } else if ft.is_file() {
            let rel = entry
                .path()
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(file) = stage_file(&rel, entry.path())? {
                out.push(file);
            }
        }
    }
    Ok(())
}

pub(super) fn sanitized_file_properties(
    source: &Path,
) -> Result<(i64, ContentHash, TokenCounts), StoreError> {
    let sanitized = sanitize::sanitize_file(source)?;
    let mut file = fs::File::open(source)?;
    let mut hasher = blake3::Hasher::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size
            .checked_add(u64::try_from(read).expect("read size fits in u64"))
            .ok_or(StoreError::SpliceResultTooLarge)?;
    }
    Ok((
        i64::try_from(size).map_err(|_| StoreError::SpliceResultTooLarge)?,
        ContentHash::from(hasher.finalize()),
        sanitized,
    ))
}

pub(super) fn stage_temporary_file(path: &str, source: PathBuf) -> Result<StagedFile, StoreError> {
    let (size, hash, sanitized) = sanitized_file_properties(&source)?;
    Ok(StagedFile {
        path: path.to_string(),
        size,
        hash,
        source: StagedSource::Temporary(source),
        sanitized,
    })
}

pub(super) fn stage_borrowed_file(path: &str, source: PathBuf) -> Result<StagedFile, StoreError> {
    let (size, hash, sanitized) = sanitized_file_properties(&source)?;
    Ok(StagedFile {
        path: path.to_string(),
        size,
        hash,
        source: StagedSource::File(source),
        sanitized,
    })
}

pub(super) fn stage_bytes(path: &str, bytes: &[u8]) -> StagedFile {
    let sanitized = sanitize::redact_tokens(bytes);
    let bytes = sanitized.as_bytes();
    let hash = ContentHash::from(blake3::hash(bytes));
    StagedFile {
        path: path.to_string(),
        size: i64::try_from(bytes.len()).expect("file size fits in SQLite INTEGER"),
        hash,
        source: StagedSource::Bytes(bytes.to_vec()),
        sanitized: sanitized.counts(),
    }
}

pub(super) fn stage_file(path: &str, source: PathBuf) -> io::Result<Option<StagedFile>> {
    let sanitized = sanitize::sanitize_file(&source)?;
    let mut file = fs::File::open(&source)?;
    let mut hasher = blake3::Hasher::new();
    let mut prefix = [0_u8; 4];
    let mut prefix_len = 0;
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if prefix_len < prefix.len() {
            let copied = (prefix.len() - prefix_len).min(read);
            prefix[prefix_len..prefix_len + copied].copy_from_slice(&buffer[..copied]);
            prefix_len += copied;
        }
        hasher.update(&buffer[..read]);
        size += u64::try_from(read).expect("read size fits in u64");
    }
    if is_junk(Path::new(path), Some(&prefix[..prefix_len])) {
        return Ok(None);
    }
    Ok(Some(StagedFile {
        path: path.to_string(),
        size: i64::try_from(size).expect("file size fits in SQLite INTEGER"),
        hash: ContentHash::from(hasher.finalize()),
        source: StagedSource::File(source),
        sanitized,
    }))
}

pub(super) fn site_manifest(
    db: &mut SqliteConnection,
    name: &str,
) -> Result<Vec<ArchiveEntry>, StoreError> {
    let site_id = site_id_locked(db, name)?;
    let mut entries = files::table
        .filter(files::site_id.eq(site_id))
        .select((files::path, files::hash, files::size))
        .order(files::path)
        .load::<(String, ContentHash, i64)>(db)?
        .into_iter()
        .map(|(path, hash, size)| ArchiveEntry::File {
            path,
            hash,
            size: size.cast_unsigned(),
        })
        .collect::<Vec<_>>();
    let allocated = allocated_entries::table
        .filter(allocated_entries::site_id.eq(site_id))
        .select((
            allocated_entries::path,
            allocated_entries::hash,
            allocated_entries::size,
        ))
        .load::<(String, ContentHash, i64)>(db)?;
    for (path, hash, size) in allocated {
        entries.push(ArchiveEntry::File {
            hash,
            path,
            size: size.cast_unsigned(),
        });
    }
    entries.extend(
        aliases::table
            .filter(aliases::site_id.eq(site_id))
            .select((aliases::path, aliases::canonical_target))
            .load::<(String, String)>(db)?
            .into_iter()
            .map(|(path, target)| ArchiveEntry::Alias { path, target }),
    );
    entries.sort_unstable_by(|left, right| archive_path(left).cmp(archive_path(right)));
    Ok(entries)
}

pub(super) fn archive_path(entry: &ArchiveEntry) -> &str {
    match entry {
        ArchiveEntry::File { path, .. } | ArchiveEntry::Alias { path, .. } => path,
    }
}

pub(super) fn write_site_archive(
    blobs: &BlobFiles,
    files: &[ArchiveEntry],
    format: ArchiveFormat,
    output: &Path,
) -> io::Result<()> {
    match format {
        ArchiveFormat::Tar => {
            append_tar_entries(fs::File::create(output)?, blobs, files)?;
        }
        ArchiveFormat::TarGz => {
            let encoder = GzEncoder::new(fs::File::create(output)?, Compression::default());
            append_tar_entries(encoder, blobs, files)?.finish()?;
        }
        ArchiveFormat::Zip => write_zip_entries(fs::File::create(output)?, blobs, files)?,
    }
    Ok(())
}

pub(super) fn append_tar_entries<W: Write>(
    writer: W,
    blobs: &BlobFiles,
    files: &[ArchiveEntry],
) -> io::Result<W> {
    let mut archive = tar::Builder::new(writer);
    for entry in files {
        match entry {
            ArchiveEntry::File { path, hash, size } => {
                let mut header = tar::Header::new_gnu();
                header.set_size(*size);
                header.set_mode(0o644);
                header.set_cksum();
                archive.append_data(&mut header, path, fs::File::open(blobs.path(*hash))?)?;
            }
            ArchiveEntry::Alias { path, target } => {
                let relative = relative_alias_target(path, target);
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                header.set_mode(0o777);
                archive.append_link(&mut header, path, relative)?;
            }
        }
    }
    archive.into_inner()
}

pub(super) fn write_zip_entries(
    writer: fs::File,
    blobs: &BlobFiles,
    files: &[ArchiveEntry],
) -> io::Result<()> {
    let mut archive = zip::ZipWriter::new(writer);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for entry in files {
        match entry {
            ArchiveEntry::File { path, hash, .. } => {
                archive
                    .start_file(path, options)
                    .map_err(io::Error::other)?;
                io::copy(&mut fs::File::open(blobs.path(*hash))?, &mut archive)?;
            }
            ArchiveEntry::Alias { path, target } => {
                let relative = zip_safe_relative_alias_target(path, target)?;
                archive
                    .add_symlink(
                        path,
                        relative,
                        zip::write::SimpleFileOptions::default().unix_permissions(0o777),
                    )
                    .map_err(io::Error::other)?;
            }
        }
    }
    archive.finish().map(|_| ()).map_err(io::Error::other)
}

pub(super) fn gc_blobs(
    tx: &mut SqliteConnection,
    now: i64,
) -> Result<Vec<ContentHash>, diesel::result::Error> {
    let mut live = HashSet::new();
    live.extend(
        files::table
            .select(files::hash)
            .distinct()
            .load::<ContentHash>(tx)?,
    );
    live.extend(
        undo_files::table
            .inner_join(undo_operations::table.on(undo_operations::token.eq(undo_files::token)))
            .filter(undo_operations::consumed.eq(0_i64))
            .filter(undo_operations::expires.gt(now))
            .select(undo_files::hash)
            .distinct()
            .load::<ContentHash>(tx)?,
    );
    live.extend(
        undo_file_deltas::table
            .inner_join(
                undo_operations::table.on(undo_operations::token.eq(undo_file_deltas::token)),
            )
            .filter(undo_operations::consumed.eq(0_i64))
            .filter(undo_operations::expires.gt(now))
            .filter(undo_file_deltas::hash.is_not_null())
            .select(undo_file_deltas::hash)
            .load::<Option<ContentHash>>(tx)?
            .into_iter()
            .flatten(),
    );
    live.extend(
        allocated_entries::table
            .select(allocated_entries::hash)
            .load::<ContentHash>(tx)?,
    );
    live.extend(
        undo_allocated_deltas::table
            .inner_join(
                undo_operations::table.on(undo_operations::token.eq(undo_allocated_deltas::token)),
            )
            .filter(undo_operations::consumed.eq(0_i64))
            .filter(undo_operations::expires.gt(now))
            .filter(undo_allocated_deltas::existed.eq(1_i64))
            .select(undo_allocated_deltas::hash)
            .load::<Option<ContentHash>>(tx)?
            .into_iter()
            .flatten(),
    );
    live.extend(
        pending_allocations::table
            .select(pending_allocations::hash)
            .load::<ContentHash>(tx)?,
    );
    let hashes = blobs::table
        .select(blobs::hash)
        .load::<ContentHash>(tx)?
        .into_iter()
        .filter(|hash| !live.contains(hash))
        .collect::<Vec<_>>();
    for chunk in hashes.chunks(SQLITE_DELETE_BATCH_SIZE) {
        diesel::delete(blobs::table.filter(blobs::hash.eq_any(chunk.to_vec()))).execute(tx)?;
    }
    Ok(hashes)
}

pub(super) const fn stale_content_hash_error(current: ContentHash) -> StoreError {
    StoreError::StaleContentHash(current)
}

pub(super) fn base_hash_matches(current: ContentHash, base_hash: &str) -> bool {
    ContentHash::parse_wire(base_hash).is_ok_and(|expected| current == expected)
}

impl Store {
    #[expect(clippy::large_types_passed_by_value)]
    pub fn publish_uploaded_archive(
        &self,
        wanted: Option<&str>,
        filename: Option<&str>,
        source: &Path,
        kind: Kind,
        options: PublishOptions<'_>,
    ) -> Result<(String, MutationResult), StoreError> {
        let tmp = self.tmp_dir(wanted.unwrap_or("upload"));
        fs::create_dir_all(&tmp)?;
        let result = (|| {
            let plan = plan_archive(source, kind)?;
            if plan
                .members
                .iter()
                .any(|member| matches!(member, ArchiveMember::File { .. }))
            {
                write_payload_file(&tmp, source, kind, filename, true)?;
            }
            let staged = stage_dir(&tmp)?;
            if staged.is_empty()
                && !plan
                    .members
                    .iter()
                    .any(|member| matches!(member, ArchiveMember::Alias { .. }))
            {
                return Err(StoreError::Upload(UploadError::EmptyArchive));
            }
            self.publish_archive_staged(wanted, &staged, &plan, options)
        })();
        let _ = fs::remove_dir_all(tmp);
        result
    }

    #[expect(clippy::large_types_passed_by_value)]
    pub fn publish_uploaded_file(
        &self,
        wanted: Option<&str>,
        filename: &str,
        source: PathBuf,
        options: PublishOptions<'_>,
    ) -> Result<(String, MutationResult), StoreError> {
        let rel = safe_rel_path(filename)?
            .to_string_lossy()
            .replace('\\', "/");
        let Some(staged) = stage_file(&rel, source)? else {
            return Err(UploadError::Junk.into());
        };
        self.publish_staged(wanted, std::slice::from_ref(&staged), options)
    }

    #[cfg(test)]
    pub fn replace_site(
        &self,
        name: &str,
        bytes: &[u8],
        kind: Kind,
        filename: Option<&str>,
        unpack: bool,
    ) -> Result<usize, StoreError> {
        let name = parse_site_name(name)?.to_string();
        let tmp = self.tmp_dir(&name);
        if tmp.exists() {
            fs::remove_dir_all(&tmp)?;
        }
        fs::create_dir_all(&tmp)?;
        match write_payload(&tmp, bytes, kind, filename, unpack) {
            Ok(_) => {}
            Err(err) => {
                let _ = fs::remove_dir_all(&tmp);
                return Err(err.into());
            }
        }
        let staged = match stage_dir(&tmp) {
            Ok(files) if !files.is_empty() => files,
            Ok(_) => {
                let _ = fs::remove_dir_all(&tmp);
                return Err(UploadError::EmptyArchive.into());
            }
            Err(err) => {
                let _ = fs::remove_dir_all(&tmp);
                return Err(err.into());
            }
        };
        let n = staged.len();
        let result = self.merge_staged(&name, &staged, UndoKind::Put);
        let _ = fs::remove_dir_all(&tmp);
        result?;
        Ok(n)
    }

    #[cfg(test)]
    pub fn put_file(&self, name: &str, rel: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let name = parse_site_name(name)?.to_string();
        let rel = safe_rel_path(rel)?.to_string_lossy().replace('\\', "/");
        if is_junk(Path::new(&rel), Some(bytes)) {
            return Err(UploadError::Junk.into());
        }
        let staged = stage_bytes(&rel, bytes);
        self.upsert_file(&name, &staged).map(|_| ())
    }

    #[cfg(test)]
    pub fn put_uploaded_file(
        &self,
        name: &str,
        rel: &str,
        source: PathBuf,
        expected_tree_hash: Option<&str>,
    ) -> Result<MutationResult, StoreError> {
        self.put_uploaded_file_secured(
            name,
            rel,
            source,
            PublishOptions {
                expected_tree_hash,
                ..PublishOptions::default()
            },
        )
    }

    #[expect(clippy::large_types_passed_by_value)]
    pub fn put_uploaded_file_secured(
        &self,
        name: &str,
        rel: &str,
        source: PathBuf,
        options: PublishOptions<'_>,
    ) -> Result<MutationResult, StoreError> {
        let name = parse_site_name(name)?.to_string();
        let rel = safe_rel_path(rel)?.to_string_lossy().replace('\\', "/");
        let Some(staged) = stage_file(&rel, source)? else {
            return Err(UploadError::Junk.into());
        };
        reject_reserved_path(&staged.path)?;
        self.merge_staged_conditional(
            &name,
            std::slice::from_ref(&staged),
            UndoKind::PutFile,
            options.expected_tree_hash,
            options.creation,
            options.authorization,
        )
    }

    pub fn replace_file_content(
        &self,
        name: &str,
        path: &str,
        base_hash: &str,
        source: AllocationSource<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        let name = parse_site_name(name)?;
        let path = normalize_rel(path)?;
        reject_reserved_path(&path)?;
        let staged = self.stage_allocation_source(source, &path)?;
        let request_fingerprint = content_mutation_request_fingerprint(
            name,
            &path,
            staged.hash,
            Some(base_hash),
            options.expected_tree_hash,
            UndoKind::Replace,
        );
        if let Some(replay) = self.replay_content_request(name, options, &request_fingerprint)? {
            return Ok(replay);
        }
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        reject_alias_write_locked(&mut db, site_id, &path, false)?;
        let kind = site_entries::table
            .find((site_id, path.as_str()))
            .select(site_entries::kind)
            .first::<i64>(&mut *db)
            .map_err(map_sql)?;
        let current_hash = entry_hash_locked(&mut db, site_id, &path, kind)?;
        drop(db);
        if !base_hash_matches(current_hash, base_hash) {
            return Err(stale_content_hash_error(current_hash));
        }
        if kind == database::schema::ALLOCATED_ENTRY_KIND {
            let metadata = self.allocated_metadata(name, &path)?;
            let destination = relocated_destination(staged.hash, &path, &metadata)?;
            self.run_before_content_commit();
            self.commit_allocated(
                name,
                Some(&path),
                &destination,
                &staged,
                Some(base_hash),
                None,
                options,
                UndoKind::Replace,
            )
        } else {
            self.run_before_content_commit();
            self.commit_regular(
                name,
                &path,
                &staged,
                Some(base_hash),
                None,
                options,
                UndoKind::Replace,
            )
        }
    }

    #[cfg(test)]
    pub fn splice_file(
        &self,
        name: &str,
        path: &str,
        base_hash: &str,
        splices: &[Splice<'_>],
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        self.splice_file_with_limit(
            name,
            path,
            base_hash,
            splices,
            options,
            MAX_SPLICE_RESULT_SIZE,
        )
    }

    #[expect(clippy::too_many_arguments)]
    pub fn splice_file_with_limit(
        &self,
        name: &str,
        path: &str,
        base_hash: &str,
        splices: &[Splice<'_>],
        options: FileMutationOptions<'_>,
        maximum_result_size: u64,
    ) -> Result<AllocatedFile, StoreError> {
        let name = parse_site_name(name)?;
        let path = normalize_rel(path)?;
        reject_reserved_path(&path)?;
        let temporary = TemporaryDirectory::create(self.tmp_dir("splice"))?;
        let output = temporary.path().join("result");
        let prepared = prepare_splices(splices, temporary.path(), maximum_result_size)?;
        let request_fingerprint = splice_request_fingerprint(
            name,
            &path,
            base_hash,
            &prepared,
            options.expected_tree_hash,
        );
        if let Some(replay) = self.replay_content_request(name, options, &request_fingerprint)? {
            return Ok(replay);
        }
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        reject_alias_write_locked(&mut db, site_id, &path, false)?;
        let kind = site_entries::table
            .find((site_id, path.as_str()))
            .select(site_entries::kind)
            .first::<i64>(&mut *db)
            .map_err(map_sql)?;
        let current_hash = entry_hash_locked(&mut db, site_id, &path, kind)?;
        if !base_hash_matches(current_hash, base_hash) {
            return Err(stale_content_hash_error(current_hash));
        }
        let old_size = entry_size_locked(&mut db, site_id, &path, kind)?;
        drop(db);
        self.run_before_content_commit();
        splice_blob_to_path(
            &self.inner.blob_files.path(current_hash),
            old_size,
            &prepared,
            &output,
            maximum_result_size,
        )?;
        let mut staged = stage_borrowed_file(&path, output)?;
        for splice in &prepared {
            staged.sanitized.management += splice.sanitized.management;
            staged.sanitized.claim += splice.sanitized.claim;
        }
        if kind == database::schema::ALLOCATED_ENTRY_KIND {
            let metadata = self.allocated_metadata(name, &path)?;
            let destination = relocated_destination(staged.hash, &path, &metadata)?;
            self.run_before_content_commit();
            self.commit_allocated(
                name,
                Some(&path),
                &destination,
                &staged,
                Some(base_hash),
                Some(&request_fingerprint),
                options,
                UndoKind::Splice,
            )
        } else {
            self.run_before_content_commit();
            self.commit_regular(
                name,
                &path,
                &staged,
                Some(base_hash),
                Some(&request_fingerprint),
                options,
                UndoKind::Splice,
            )
        }
    }

    #[cfg(test)]
    pub fn pop_site(&self, name: &str) -> Result<Vec<u8>, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        let archive = site_files(&mut tx, &self.inner.blob_files, name)?;
        let packed = pack_tar_gz(&archive.files)?;
        snapshot_site(&mut tx, name, UndoKind::DeleteSite, self.now_millis())?;
        retain_management_tombstone(&mut tx, name, self.now_millis())?;
        diesel::delete(sites::table.filter(sites::name.eq(name))).execute(&mut *tx)?;
        let removed = gc_blobs(&mut tx, self.now_millis())?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(packed)
    }

    #[cfg(test)]
    pub fn pack_site(&self, name: &str, format: ArchiveFormat) -> Result<Vec<u8>, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        let archive = site_files(&mut db, &self.inner.blob_files, name)?;
        drop(db);
        match format {
            ArchiveFormat::Tar => pack_tar(&archive.files),
            ArchiveFormat::TarGz => pack_tar_gz(&archive.files),
            ArchiveFormat::Zip => pack_zip(&archive.files),
        }
        .map_err(StoreError::Io)
    }

    pub fn pack_site_to_path(
        &self,
        name: &str,
        format: ArchiveFormat,
        output: &Path,
    ) -> Result<u64, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.readers.get();
        let entries = site_manifest(&mut db, name)?;
        drop(db);
        write_site_archive(&self.inner.blob_files, &entries, format, output)?;
        Ok(fs::metadata(output)?.len())
    }

    pub fn pop_site_to_path_secured(
        &self,
        name: &str,
        format: ArchiveFormat,
        output: &Path,
        authorization: Option<&ManagementToken>,
    ) -> Result<PopResult, StoreError> {
        let name = parse_site_name(name)?;
        let mut db = self.inner.writer.lock().unwrap();
        let entries = site_manifest(&mut db, name)?;
        write_site_archive(&self.inner.blob_files, &entries, format, output)?;
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, authorization)?;
        let undo = snapshot_site(&mut tx, name, UndoKind::DeleteSite, self.now_millis())?;
        retain_management_tombstone(&mut tx, name, self.now_millis())?;
        diesel::delete(sites::table.filter(sites::name.eq(name))).execute(&mut *tx)?;
        prune_undo_locked(&mut tx, self.now_millis())?;
        let removed = gc_blobs(&mut tx, self.now_millis())?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(PopResult {
            size: fs::metadata(output)?.len(),
            undo,
        })
    }

    #[cfg(test)]
    pub fn copy_site(
        &self,
        source: &str,
        destination: Option<&str>,
        idempotency: Option<&Idempotency>,
    ) -> Result<(String, MutationResult), StoreError> {
        self.copy_site_secured(
            source,
            destination,
            idempotency,
            CreationSecurity::default(),
        )
    }

    #[expect(clippy::too_many_lines)]
    pub fn copy_site_secured(
        &self,
        source: &str,
        destination: Option<&str>,
        idempotency: Option<&Idempotency>,
        creation: CreationSecurity,
    ) -> Result<(String, MutationResult), StoreError> {
        let source = parse_site_name(source)?;
        let destination = destination
            .map(parse_site_name)
            .transpose()?
            .map(str::to_string);
        let now = self.now_millis();
        let fingerprint = format!("copy:{source}");
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        prune_idempotency_locked(&mut tx, now)?;
        if !site_exists_locked(&mut tx, source)? {
            return Err(StoreError::NotFound);
        }
        if destination.is_none()
            && let Some(idempotency) = idempotency
        {
            validate_idempotency_key(&idempotency.key)?;
            if let Some(replay) = idempotency_replay(
                &mut tx,
                &idempotency.key,
                &fingerprint,
                IdempotencyKind::AutoCopy,
            )? {
                return Ok((replay.name, replay.mutation));
            }
        }
        let generated = destination.is_none();
        let existing_names = sites::table
            .select(sites::name)
            .load::<String>(&mut *tx)?
            .into_iter()
            .collect::<HashSet<_>>();
        let destination = destination
            .unwrap_or_else(|| generate_id(|candidate| existing_names.contains(candidate)));
        if site_exists_locked(&mut tx, &destination)? {
            return Err(StoreError::DestinationConflict);
        }
        let (source_id, public_url, revision) = sites::table
            .filter(sites::name.eq(source))
            .select((sites::id, sites::public_url, sites::content_revision))
            .first::<(i64, String, i64)>(&mut *tx)?;
        let undo = snapshot_site_with_description(
            &mut tx,
            &destination,
            UndoKind::Copy,
            &format!("remove copied site {destination}"),
            now,
        )?;
        let destination_id = diesel::insert_into(sites::table)
            .values(NewSite {
                name: &destination,
                created: Some(now),
                updated: now,
                public_url: &public_url,
                content_revision: revision,
                tree_hash: TreeHash::EMPTY,
                creator_kind: creation.creator.map(|creator| creator.kind as i64),
                creator_hash: creation.creator.map(|creator| creator.hash.to_vec()),
                claim_hash: creation.claim_hash.map(|hash| hash.as_bytes().to_vec()),
                management_hash: creation
                    .management_hash
                    .map(|hash| hash.as_bytes().to_vec()),
                management_status: i64::from(creation.management_hash.is_some()),
            })
            .returning(sites::id)
            .get_result::<i64>(&mut *tx)?;
        let copied_files = files::table
            .filter(files::site_id.eq(source_id))
            .filter(files::path.ne(MANIFEST_PATH))
            .select((files::path, files::hash, files::size))
            .load::<(String, ContentHash, i64)>(&mut *tx)?;
        for (path, hash, size) in copied_files {
            ensure_file_entry(&mut tx, destination_id, &path)?;
            diesel::insert_into(files::table)
                .values(NewFile {
                    site_id: destination_id,
                    path,
                    hash,
                    size,
                })
                .execute(&mut *tx)?;
        }
        let copied_allocated = allocated_entries::table
            .filter(allocated_entries::site_id.eq(source_id))
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
            )>(&mut *tx)?;
        for (path, hash, size, naming_mode, prefix, suffix, extension, media_type) in
            copied_allocated
        {
            diesel::insert_into(site_entries::table)
                .values((
                    site_entries::site_id.eq(destination_id),
                    site_entries::path.eq(&path),
                    site_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                ))
                .execute(&mut *tx)?;
            diesel::insert_into(allocated_entries::table)
                .values((
                    allocated_entries::site_id.eq(destination_id),
                    allocated_entries::path.eq(path),
                    allocated_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                    allocated_entries::hash.eq(hash),
                    allocated_entries::size.eq(size),
                    allocated_entries::naming_mode.eq(naming_mode),
                    allocated_entries::prefix.eq(prefix),
                    allocated_entries::suffix.eq(suffix),
                    allocated_entries::extension.eq(extension),
                    allocated_entries::media_type.eq(media_type),
                ))
                .execute(&mut *tx)?;
        }
        let copied_aliases = aliases::table
            .filter(aliases::site_id.eq(source_id))
            .select((
                aliases::path,
                aliases::canonical_target,
                aliases::resolved_kind,
                aliases::resolved_hash,
                aliases::resolved_size,
            ))
            .load::<(
                String,
                String,
                Option<i64>,
                Option<ContentHash>,
                Option<i64>,
            )>(&mut *tx)?;
        for (path, target, resolved_kind, resolved_hash, resolved_size) in copied_aliases {
            diesel::insert_into(site_entries::table)
                .values((
                    site_entries::site_id.eq(destination_id),
                    site_entries::path.eq(&path),
                    site_entries::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                ))
                .execute(&mut *tx)?;
            diesel::insert_into(aliases::table)
                .values((
                    aliases::site_id.eq(destination_id),
                    aliases::path.eq(path),
                    aliases::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                    aliases::canonical_target.eq(target),
                    aliases::resolved_kind.eq(resolved_kind),
                    aliases::resolved_hash.eq(resolved_hash),
                    aliases::resolved_size.eq(resolved_size),
                ))
                .execute(&mut *tx)?;
        }
        let aggregates = path_aggregates::table
            .filter(path_aggregates::site_id.eq(source_id))
            .select((
                path_aggregates::path,
                path_aggregates::logical_bytes,
                path_aggregates::file_count,
            ))
            .load::<(String, i64, i64)>(&mut *tx)?;
        for (path, logical_bytes, file_count) in aggregates {
            diesel::insert_into(path_aggregates::table)
                .values((
                    path_aggregates::site_id.eq(destination_id),
                    path_aggregates::path.eq(path),
                    path_aggregates::logical_bytes.eq(logical_bytes),
                    path_aggregates::file_count.eq(file_count),
                ))
                .execute(&mut *tx)?;
        }
        copy_expiry_policies_locked(&mut tx, source_id, destination_id, now)?;
        let files = site_entries::table
            .filter(site_entries::site_id.eq(destination_id))
            .filter(site_entries::path.ne(MANIFEST_PATH))
            .filter(site_entries::kind.ne(database::schema::ALIAS_ENTRY_KIND))
            .select(count_star())
            .first::<i64>(&mut *tx)?;
        let tree_hash = regenerate_site(&mut tx, &self.inner.blob_files, destination_id, now)?;
        prune_undo_locked(&mut tx, now)?;
        let mutation = MutationResult {
            created: true,
            changed: true,
            replayed: false,
            files: usize::try_from(files).expect("file count fits in usize"),
            revision: revision.cast_unsigned(),
            tree_hash: tree_hash.to_wire(),
            undo: Some(undo),
            sanitized: TokenCounts::default(),
        };
        if generated && let Some(idempotency) = idempotency {
            let published = PublishedMutation {
                name: destination.clone(),
                mutation: mutation.clone(),
            };
            store_idempotency(
                &mut tx,
                &idempotency.key,
                &fingerprint,
                IdempotencyKind::AutoCopy,
                &published,
                now,
            )?;
        }
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok((destination, mutation))
    }

    #[cfg(test)]
    pub fn move_site(
        &self,
        source: &str,
        destination: &str,
    ) -> Result<(String, MutationResult), StoreError> {
        self.move_site_secured(source, destination, None)
    }

    pub fn move_site_secured(
        &self,
        source: &str,
        destination: &str,
        authorization: Option<&ManagementToken>,
    ) -> Result<(String, MutationResult), StoreError> {
        let source = parse_site_name(source)?;
        let destination = parse_site_name(destination)?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, source, authorization)?;
        if !site_exists_locked(&mut tx, source)? {
            return Err(StoreError::NotFound);
        }
        if site_exists_locked(&mut tx, destination)? {
            return Err(StoreError::DestinationConflict);
        }
        let undo = snapshot_site_with_description(
            &mut tx,
            source,
            UndoKind::Move,
            &format!("move {destination} back to {source}"),
            now,
        )?;
        diesel::insert_into(undo_names::table)
            .values((
                undo_names::token.eq(&undo.token),
                undo_names::name.eq(destination),
            ))
            .execute(&mut *tx)?;
        diesel::update(sites::table.filter(sites::name.eq(source)))
            .set((sites::name.eq(destination), sites::updated.eq(now)))
            .execute(&mut *tx)?;
        let (site_id, revision) = sites::table
            .filter(sites::name.eq(destination))
            .select((sites::id, sites::content_revision))
            .first::<(i64, i64)>(&mut *tx)?;
        record_site_event(&mut tx, site_id, StoredSiteEventKind::Rename, 0, now)?;
        let files = site_entries::table
            .filter(site_entries::site_id.eq(site_id))
            .filter(site_entries::path.ne(MANIFEST_PATH))
            .filter(site_entries::kind.ne(database::schema::ALIAS_ENTRY_KIND))
            .select(count_star())
            .first::<i64>(&mut *tx)?;
        let tree_hash = regenerate_site(&mut tx, &self.inner.blob_files, site_id, now)?;
        prune_undo_locked(&mut tx, now)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok((
            destination.to_string(),
            MutationResult {
                created: false,
                changed: true,
                replayed: false,
                files: usize::try_from(files).expect("file count fits in usize"),
                revision: revision.cast_unsigned(),
                tree_hash: tree_hash.to_wire(),
                undo: Some(undo),
                sanitized: TokenCounts::default(),
            },
        ))
    }

    #[cfg(test)]
    pub fn delete_file(&self, name: &str, rel: &str) -> Result<MutationResult, StoreError> {
        self.delete_file_secured(name, rel, None)
    }

    pub fn delete_file_secured(
        &self,
        name: &str,
        rel: &str,
        authorization: Option<&ManagementToken>,
    ) -> Result<MutationResult, StoreError> {
        let name = parse_site_name(name)?;
        let rel = safe_rel_path(rel)?.to_string_lossy().replace('\\', "/");
        let (prefix_start, prefix_end) = descendant_bounds(&rel);
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, authorization)?;
        reject_reserved_path(&rel)?;
        let site_id = site_id_locked(&mut tx, name)?;
        reject_alias_write_locked(&mut tx, site_id, &rel, true)?;
        let mut paths = site_entries::table
            .filter(site_entries::site_id.eq(site_id))
            .filter(
                site_entries::path.eq(&rel).or(site_entries::path
                    .ge(&prefix_start)
                    .and(site_entries::path.lt(&prefix_end))),
            )
            .select(site_entries::path)
            .load::<String>(&mut *tx)?;
        let exact_kind = site_entries::table
            .find((site_id, rel.as_str()))
            .select(site_entries::kind)
            .first::<i64>(&mut *tx)
            .optional()?;
        if exact_kind == Some(database::schema::ALIAS_ENTRY_KIND) {
            paths.clear();
            paths.push(rel.clone());
        }
        if paths.is_empty() {
            return Err(StoreError::NotFound);
        }
        let path_refs = paths.iter().map(String::as_str).collect::<Vec<_>>();
        let undo = snapshot_entry_deltas(
            &mut tx,
            name,
            UndoKind::DeletePath,
            &format!("restore deleted {rel}"),
            &path_refs,
            self.now_millis(),
        )?;
        let removed_files = files::table
            .filter(files::site_id.eq(site_id))
            .filter(files::path.eq_any(&paths))
            .select((files::path, files::size))
            .load::<(String, i64)>(&mut *tx)?;
        let removed_allocated = allocated_entries::table
            .filter(allocated_entries::site_id.eq(site_id))
            .filter(allocated_entries::path.eq_any(&paths))
            .select((allocated_entries::path, allocated_entries::size))
            .load::<(String, i64)>(&mut *tx)?;
        for (path, size) in removed_files.iter().chain(&removed_allocated) {
            adjust_aggregates_locked(&mut tx, site_id, path, -*size, -1)?;
        }
        let deleted = diesel::delete(
            site_entries::table
                .filter(site_entries::site_id.eq(site_id))
                .filter(site_entries::path.eq_any(&paths)),
        )
        .execute(&mut *tx)?;
        diesel::delete(
            expiry_policies::table
                .filter(expiry_policies::site_id.eq(site_id))
                .filter(
                    expiry_policies::path.eq(&rel).or(expiry_policies::path
                        .ge(&prefix_start)
                        .and(expiry_policies::path.lt(&prefix_end))),
                ),
        )
        .execute(&mut *tx)?;
        diesel::update(sites::table.find(site_id))
            .set(sites::content_revision.eq(sites::content_revision + 1))
            .execute(&mut *tx)?;
        refresh_aliases_locked(&mut tx, site_id, &[AliasChange::Subtree(&rel)])?;
        refresh_expiry_for_changes_locked(&mut tx, site_id, &[&rel], self.now_millis())?;
        regenerate_site(&mut tx, &self.inner.blob_files, site_id, self.now_millis())?;
        prune_undo_locked(&mut tx, self.now_millis())?;
        let removed = gc_blobs(&mut tx, self.now_millis())?;
        let (revision, tree_hash) =
            site_revision_locked(&mut tx, name).unwrap_or((0, TreeHash::EMPTY));
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(MutationResult {
            created: false,
            changed: true,
            replayed: false,
            files: deleted,
            revision,
            tree_hash: tree_hash.to_wire(),
            undo: Some(undo),
            sanitized: TokenCounts::default(),
        })
    }

    pub(super) fn commit_site(&self, name: &str, files: &[StagedFile]) -> Result<(), StoreError> {
        self.merge_staged(name, files, UndoKind::Put).map(|_| ())
    }

    #[cfg(test)]
    pub(super) fn upsert_file(
        &self,
        name: &str,
        file: &StagedFile,
    ) -> Result<MutationResult, StoreError> {
        reject_reserved_path(&file.path)?;
        self.merge_staged(name, std::slice::from_ref(file), UndoKind::Put)
    }

    #[expect(clippy::large_types_passed_by_value)]
    pub(super) fn publish_staged(
        &self,
        wanted: Option<&str>,
        files: &[StagedFile],
        options: PublishOptions<'_>,
    ) -> Result<(String, MutationResult), StoreError> {
        self.publish_staged_entries(wanted, files, &[], options)
    }

    #[expect(clippy::large_types_passed_by_value)]
    pub(super) fn publish_archive_staged(
        &self,
        wanted: Option<&str>,
        files: &[StagedFile],
        plan: &ArchivePlan,
        options: PublishOptions<'_>,
    ) -> Result<(String, MutationResult), StoreError> {
        let aliases = plan
            .members
            .iter()
            .filter_map(|member| match member {
                ArchiveMember::Alias {
                    path,
                    canonical_target,
                } => Some(ArchiveAlias {
                    path: path.as_str(),
                    target: canonical_target.as_str(),
                }),
                ArchiveMember::File { .. } => None,
            })
            .collect::<Vec<_>>();
        self.publish_staged_entries(wanted, files, &aliases, options)
    }

    #[expect(clippy::large_types_passed_by_value)]
    pub(super) fn publish_staged_entries(
        &self,
        wanted: Option<&str>,
        files: &[StagedFile],
        archive_aliases: &[ArchiveAlias<'_>],
        options: PublishOptions<'_>,
    ) -> Result<(String, MutationResult), StoreError> {
        let wanted = wanted.filter(|name| !name.is_empty());
        if let Some(name) = wanted {
            let name = parse_site_name(name)?.to_string();
            let mutation = self.merge_staged_entries_conditional(
                &name,
                files,
                archive_aliases,
                UndoKind::Put,
                options.expected_tree_hash,
                options.creation,
                options.authorization,
                options.replace,
            )?;
            return Ok((name, mutation));
        }
        if options.expected_tree_hash.is_some() {
            return Err(StoreError::PreconditionFailed {
                revision: 0,
                tree_hash: TreeHash::EMPTY,
            });
        }
        let files = files
            .iter()
            .filter(|file| file.path != MANIFEST_PATH)
            .collect::<Vec<_>>();
        if files.is_empty() && archive_aliases.is_empty() {
            return Err(StoreError::Upload(UploadError::EmptyArchive));
        }
        for file in &files {
            reject_reserved_path(&file.path)?;
        }
        let fingerprint = staged_entries_fingerprint(&files, archive_aliases);
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        prune_idempotency_locked(&mut tx, now)?;
        if let Some(idempotency) = options.idempotency {
            validate_idempotency_key(&idempotency.key)?;
            if let Some(replay) = idempotency_replay(
                &mut tx,
                &idempotency.key,
                &fingerprint,
                IdempotencyKind::UnnamedPut,
            )? {
                return Ok((replay.name, replay.mutation));
            }
        }
        let existing_names = sites::table
            .select(sites::name)
            .load::<String>(&mut *tx)?
            .into_iter()
            .collect::<HashSet<_>>();
        let name = generate_id(|candidate| existing_names.contains(candidate));
        let mutation = self.merge_staged_locked(
            &mut tx,
            &files,
            MergeContext {
                name: &name,
                kind: UndoKind::Put,
                expected_tree_hash: None,
                now,
                creation: options.creation,
                authorization: None,
                archive_aliases,
                replace: false,
            },
        )?;
        let published = PublishedMutation {
            name: name.clone(),
            mutation: mutation.clone(),
        };
        if let Some(idempotency) = options.idempotency {
            store_idempotency(
                &mut tx,
                &idempotency.key,
                &fingerprint,
                IdempotencyKind::UnnamedPut,
                &published,
                now,
            )?;
        }
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok((name, mutation))
    }

    pub(super) fn merge_staged(
        &self,
        name: &str,
        files: &[StagedFile],
        kind: UndoKind,
    ) -> Result<MutationResult, StoreError> {
        self.merge_staged_conditional(name, files, kind, None, CreationSecurity::default(), None)
    }

    #[expect(clippy::too_many_arguments)]
    pub(super) fn merge_staged_conditional(
        &self,
        name: &str,
        files: &[StagedFile],
        kind: UndoKind,
        expected_tree_hash: Option<&str>,
        creation: CreationSecurity,
        authorization: Option<&ManagementToken>,
    ) -> Result<MutationResult, StoreError> {
        self.merge_staged_entries_conditional(
            name,
            files,
            &[],
            kind,
            expected_tree_hash,
            creation,
            authorization,
            false,
        )
    }

    #[expect(clippy::too_many_arguments)]
    pub(super) fn merge_staged_entries_conditional(
        &self,
        name: &str,
        files: &[StagedFile],
        archive_aliases: &[ArchiveAlias<'_>],
        kind: UndoKind,
        expected_tree_hash: Option<&str>,
        creation: CreationSecurity,
        authorization: Option<&ManagementToken>,
        replace: bool,
    ) -> Result<MutationResult, StoreError> {
        let files = files
            .iter()
            .filter(|file| file.path != MANIFEST_PATH)
            .collect::<Vec<_>>();
        if files.is_empty() && archive_aliases.is_empty() {
            return Err(StoreError::Upload(UploadError::EmptyArchive));
        }
        for file in &files {
            reject_reserved_path(&file.path)?;
        }
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        let mutation = self.merge_staged_locked(
            &mut tx,
            &files,
            MergeContext {
                name,
                kind,
                expected_tree_hash,
                now,
                creation,
                authorization,
                archive_aliases,
                replace,
            },
        )?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(mutation)
    }

    #[expect(
        clippy::large_types_passed_by_value,
        clippy::too_many_lines,
        clippy::cognitive_complexity
    )]
    pub(super) fn merge_staged_locked(
        &self,
        tx: &mut SqliteConnection,
        files: &[&StagedFile],
        context: MergeContext<'_>,
    ) -> Result<MutationResult, StoreError> {
        let MergeContext {
            name,
            kind,
            expected_tree_hash,
            now,
            creation,
            authorization,
            archive_aliases,
            replace,
        } = context;
        let existed = site_exists_locked(tx, name)?;
        if existed {
            authorize_locked(tx, name, authorization)?;
        }
        if let Some(expected) = expected_tree_hash {
            let (revision, tree_hash) = if existed {
                site_revision_locked(tx, name)?
            } else {
                (0, TreeHash::EMPTY)
            };
            if TreeHash::try_from(expected)? != tree_hash {
                return Err(StoreError::PreconditionFailed {
                    revision,
                    tree_hash,
                });
            }
        }
        let existing_site_id = sites::table
            .filter(sites::name.eq(name))
            .select(sites::id)
            .first::<i64>(tx)
            .optional()?;
        if let Some(site_id) = existing_site_id {
            reject_alias_writes_locked(
                tx,
                site_id,
                files.iter().map(|file| file.path.as_str()),
                false,
            )?;
            validate_archive_alias_conflicts_locked(tx, site_id, archive_aliases)?;
        }
        let mut changed = false;
        for file in files {
            let current = if let Some(site_id) = existing_site_id {
                files::table
                    .find((site_id, file.path.as_str()))
                    .select(files::hash)
                    .first::<ContentHash>(tx)
                    .optional()?
            } else {
                None
            };
            changed |= current != Some(file.hash);
        }
        if let Some(site_id) = existing_site_id {
            for alias in archive_aliases {
                let current = aliases::table
                    .find((site_id, alias.path))
                    .select(aliases::canonical_target)
                    .first::<String>(tx)
                    .optional()?;
                changed |= current.as_deref() != Some(alias.target);
            }
        } else {
            changed |= !archive_aliases.is_empty();
        }
        let keep = files
            .iter()
            .map(|file| file.path.as_str())
            .chain(archive_aliases.iter().map(|alias| alias.path))
            .chain(std::iter::once(MANIFEST_PATH))
            .collect::<HashSet<_>>();
        let mut prune_paths = Vec::new();
        if replace && let Some(site_id) = existing_site_id {
            prune_paths = site_entries::table
                .filter(site_entries::site_id.eq(site_id))
                .select(site_entries::path)
                .load::<String>(tx)?
                .into_iter()
                .filter(|path| !keep.contains(path.as_str()))
                .collect();
            changed |= !prune_paths.is_empty();
            if !changed {
                changed |= expiry_policies::table
                    .filter(expiry_policies::site_id.eq(site_id))
                    .select(expiry_policies::path)
                    .load::<String>(tx)?
                    .into_iter()
                    .any(|path| !path.is_empty() && !keep.contains(path.as_str()));
            }
        }
        if !changed {
            let (revision, tree_hash) = site_revision_locked(tx, name)?;
            return Ok(MutationResult {
                created: false,
                changed: false,
                replayed: false,
                files: files.len() + archive_aliases.len(),
                revision,
                tree_hash: tree_hash.to_wire(),
                undo: None,
                sanitized: sanitized_counts(files),
            });
        }
        if let Some(site_id) = existing_site_id
            && !archive_aliases.is_empty()
        {
            validate_alias_graph_with_staged(tx, site_id, files, archive_aliases)?;
        }
        for file in files {
            self.materialize(file)?;
        }
        let description = if existed {
            format!("restore previous state of {name}")
        } else {
            format!("remove newly created site {name}")
        };
        let changed_paths = files
            .iter()
            .map(|file| file.path.as_str())
            .chain(archive_aliases.iter().map(|alias| alias.path))
            .chain(prune_paths.iter().map(String::as_str))
            .collect::<Vec<_>>();
        let undo = if existed && !replace {
            snapshot_entry_deltas(tx, name, kind, &description, &changed_paths, now)?
        } else {
            snapshot_site_with_description(tx, name, kind, &description, now)?
        };
        for file in files {
            diesel::insert_into(blobs::table)
                .values((
                    blobs::hash.eq(file.hash),
                    blobs::bytes.eq(Vec::<u8>::new()),
                    blobs::size.eq(file.size),
                ))
                .on_conflict_do_nothing()
                .execute(tx)?;
        }
        diesel::insert_into(sites::table)
            .values(NewSite {
                name,
                created: (!existed).then_some(now),
                updated: now,
                public_url: &self.inner.public_url,
                content_revision: 0,
                tree_hash: TreeHash::EMPTY,
                creator_kind: creation.creator.map(|creator| creator.kind as i64),
                creator_hash: creation.creator.map(|creator| creator.hash.to_vec()),
                claim_hash: creation.claim_hash.map(|hash| hash.as_bytes().to_vec()),
                management_hash: creation
                    .management_hash
                    .map(|hash| hash.as_bytes().to_vec()),
                management_status: i64::from(creation.management_hash.is_some()),
            })
            .on_conflict_do_nothing()
            .execute(tx)?;
        let site_id = site_id_locked(tx, name)?;
        if !existed {
            record_site_event(tx, site_id, StoredSiteEventKind::Created, 0, now)?;
        }
        for file in files {
            let previous_size = files::table
                .find((site_id, file.path.as_str()))
                .select(files::size)
                .first::<i64>(tx)
                .optional()?;
            ensure_file_entry(tx, site_id, &file.path)?;
            diesel::insert_into(files::table)
                .values(NewFile {
                    site_id,
                    path: file.path.clone(),
                    hash: file.hash,
                    size: file.size,
                })
                .on_conflict((files::site_id, files::path))
                .do_update()
                .set((
                    files::hash.eq(excluded(files::hash)),
                    files::size.eq(excluded(files::size)),
                ))
                .execute(tx)?;
            adjust_aggregates_locked(
                tx,
                site_id,
                &file.path,
                file.size - previous_size.unwrap_or(0),
                i64::from(previous_size.is_none()),
            )?;
        }
        for alias in archive_aliases {
            diesel::insert_into(site_entries::table)
                .values((
                    site_entries::site_id.eq(site_id),
                    site_entries::path.eq(alias.path),
                    site_entries::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                ))
                .on_conflict_do_nothing()
                .execute(tx)?;
            diesel::insert_into(aliases::table)
                .values((
                    aliases::site_id.eq(site_id),
                    aliases::path.eq(alias.path),
                    aliases::kind.eq(database::schema::ALIAS_ENTRY_KIND),
                    aliases::canonical_target.eq(alias.target),
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
                .execute(tx)?;
        }
        if replace && existed {
            prune_unlisted_locked(tx, site_id, &keep)?;
        }
        let revision = if existed {
            sites::table
                .find(site_id)
                .select(sites::content_revision + 1)
                .first::<i64>(tx)?
        } else {
            1
        };
        diesel::update(sites::table.find(site_id))
            .set((sites::updated.eq(now), sites::content_revision.eq(revision)))
            .execute(tx)?;
        record_site_event(
            tx,
            site_id,
            StoredSiteEventKind::Publish,
            changed_paths.len(),
            now,
        )?;
        if replace && existed {
            refresh_all_aliases_locked(tx, site_id)?;
        } else {
            let alias_changes = files
                .iter()
                .map(|file| AliasChange::Entry(file.path.as_str()))
                .chain(
                    archive_aliases
                        .iter()
                        .map(|alias| AliasChange::Alias(alias.path)),
                )
                .collect::<Vec<_>>();
            refresh_aliases_locked(tx, site_id, &alias_changes)?;
        }
        refresh_expiry_for_changes_locked(tx, site_id, &changed_paths, now)?;
        let tree_hash = regenerate_site(tx, &self.inner.blob_files, site_id, now)?;
        prune_undo_locked(tx, now)?;
        Ok(MutationResult {
            created: !existed,
            changed: true,
            replayed: false,
            files: files.len() + archive_aliases.len(),
            revision: revision.cast_unsigned(),
            tree_hash: tree_hash.to_wire(),
            undo: Some(undo),
            sanitized: sanitized_counts(files),
        })
    }

    pub(super) fn replay_content_request(
        &self,
        name: &str,
        options: FileMutationOptions<'_>,
        request_fingerprint: &str,
    ) -> Result<Option<AllocatedFile>, StoreError> {
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, options.authorization)?;
        prune_idempotency_locked(&mut tx, now)?;
        let replay =
            entry_mutation_request_replay(&mut tx, options.idempotency, request_fingerprint);
        drop(tx);
        drop(db);
        replay
    }

    #[expect(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(super) fn commit_regular(
        &self,
        name: &str,
        path: &str,
        staged: &StagedFile,
        expected_content_hash: Option<&str>,
        request_fingerprint: Option<&str>,
        options: FileMutationOptions<'_>,
        kind: UndoKind,
    ) -> Result<AllocatedFile, StoreError> {
        let now = self.now_millis();
        let fingerprint = entry_mutation_fingerprint(
            name,
            Some(path),
            path,
            staged.hash,
            kind,
            options.expected_tree_hash,
        );
        let computed_request_fingerprint;
        let request_fingerprint = if let Some(request_fingerprint) = request_fingerprint {
            request_fingerprint
        } else {
            computed_request_fingerprint = content_mutation_request_fingerprint(
                name,
                path,
                staged.hash,
                expected_content_hash,
                options.expected_tree_hash,
                kind,
            );
            &computed_request_fingerprint
        };
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, options.authorization)?;
        prune_idempotency_locked(&mut tx, now)?;
        if let Some(replay) = entry_mutation_replay(&mut tx, options.idempotency, &fingerprint)? {
            return Ok(replay);
        }
        check_tree_precondition(&mut tx, name, options.expected_tree_hash)?;
        let site_id = site_id_locked(&mut tx, name)?;
        reject_alias_write_locked(&mut tx, site_id, path, false)?;
        let current = files::table
            .find((site_id, path))
            .select((files::hash, files::size))
            .first::<(ContentHash, i64)>(&mut *tx)
            .optional()?;
        let Some((current_hash, current_size)) = current else {
            return Err(StoreError::NotFound);
        };
        if expected_content_hash
            .is_some_and(|expected| ContentHash::try_from(expected).ok() != Some(current_hash))
        {
            return Err(stale_content_hash_error(current_hash));
        }
        if current_hash == staged.hash {
            let (revision, tree_hash) = site_revision_locked(&mut tx, name)?;
            let result = AllocatedFile {
                path: path.to_string(),
                hash: format!("blake3:{}", current_hash.to_hex()),
                size: current_size.cast_unsigned(),
                changed: false,
                replayed: false,
                mutation: Some(MutationResult {
                    created: false,
                    changed: false,
                    replayed: false,
                    files: 1,
                    revision,
                    tree_hash: tree_hash.to_wire(),
                    undo: None,
                    sanitized: staged.sanitized,
                }),
            };
            store_entry_mutation_with_request(
                &mut tx,
                options.idempotency,
                &fingerprint,
                request_fingerprint,
                &result,
                now,
            )?;
            tx.commit()?;
            return Ok(result);
        }
        self.materialize(staged)?;
        let undo = snapshot_entry_deltas(
            &mut tx,
            name,
            kind,
            &format!("restore previous {path}"),
            &[path],
            now,
        )?;
        ensure_blob_locked(&mut tx, staged)?;
        diesel::update(files::table.find((site_id, path)))
            .set((files::hash.eq(staged.hash), files::size.eq(staged.size)))
            .execute(&mut *tx)?;
        adjust_aggregates_locked(&mut tx, site_id, path, staged.size - current_size, 0)?;
        finish_entry_mutation(&mut tx, &self.inner.blob_files, site_id, &[path], now)?;
        let (revision, tree_hash) = site_revision_locked(&mut tx, name)?;
        let mutation = MutationResult {
            created: false,
            changed: true,
            replayed: false,
            files: 1,
            revision,
            tree_hash: tree_hash.to_wire(),
            undo: Some(undo),
            sanitized: staged.sanitized,
        };
        let result = AllocatedFile {
            path: path.to_string(),
            hash: format!("blake3:{}", staged.hash.to_hex()),
            size: staged.size.cast_unsigned(),
            changed: true,
            replayed: false,
            mutation: Some(mutation),
        };
        store_entry_mutation_with_request(
            &mut tx,
            options.idempotency,
            &fingerprint,
            request_fingerprint,
            &result,
            now,
        )?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(result)
    }

    pub(super) fn materialize(&self, file: &StagedFile) -> Result<(), StoreError> {
        match &file.source {
            StagedSource::Bytes(bytes) => self.inner.blob_files.put_bytes(file.hash, bytes)?,
            StagedSource::File(path) | StagedSource::Temporary(path) => {
                self.inner.blob_files.put_file(file.hash, path)?;
            }
        }
        Ok(())
    }
}
