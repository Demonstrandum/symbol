// One slice of `store`: it shares the parent's imports and types rather
// than re-listing ~90 names. `allow`, not `expect`, because clippy
// reports an expectation on a `use` item as unfulfilled.
#[allow(clippy::wildcard_imports)]
use super::*;

pub(super) fn allocation_destination(
    hash: ContentHash,
    spec: AllocationSpec<'_>,
) -> Result<AllocationDestination, StoreError> {
    let hash_hex = hash.to_hex();
    if hash_hex.len() != 64
        || !hash_hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || !safe_name_fragment(spec.naming.prefix)
        || !safe_name_fragment(spec.naming.suffix)
    {
        return Err(StoreError::InvalidAllocatedName);
    }
    let folder = normalize_folder(spec.folder)?;
    let extension = spec.naming.extension.map(normalize_extension).transpose()?;
    let media_type = normalize_media_type(spec.media_type)?;
    let mut basename = String::with_capacity(
        spec.naming.prefix.len()
            + hash_hex.len()
            + spec.naming.suffix.len()
            + extension.as_ref().map_or(0, |value| value.len() + 1),
    );
    basename.push_str(spec.naming.prefix);
    basename.push_str(&hash_hex);
    basename.push_str(spec.naming.suffix);
    if let Some(extension) = &extension {
        basename.push('.');
        basename.push_str(extension);
    }
    let path = join_folder(&folder, &basename);
    safe_rel_path(&path).map_err(|_| StoreError::InvalidAllocatedName)?;
    reject_reserved_path(&path)?;
    Ok(AllocationDestination {
        path,
        naming_mode: AllocatedNamingMode::ContentAddressed,
        prefix: spec.naming.prefix.to_string(),
        suffix: spec.naming.suffix.to_string(),
        extension,
        media_type,
    })
}

pub(super) fn custom_destination(
    folder: &str,
    basename: &str,
    media_type: &str,
) -> Result<AllocationDestination, StoreError> {
    let folder = normalize_folder(folder)?;
    if basename.is_empty()
        || basename == "."
        || basename == ".."
        || basename.contains(['/', '\\'])
        || !safe_name_fragment(basename)
        || is_reserved_path(basename)
    {
        return Err(StoreError::InvalidAllocatedName);
    }
    safe_rel_path(basename).map_err(|_| StoreError::InvalidAllocatedName)?;
    Ok(AllocationDestination {
        path: join_folder(&folder, basename),
        naming_mode: AllocatedNamingMode::Custom,
        prefix: basename.to_string(),
        suffix: String::new(),
        extension: None,
        media_type: normalize_media_type(media_type)?,
    })
}

pub(super) fn relocated_destination(
    hash: ContentHash,
    current_path: &str,
    metadata: &AllocatedMetadata,
) -> Result<AllocationDestination, StoreError> {
    let folder = current_path
        .rsplit_once('/')
        .map_or("", |(folder, _)| folder);
    match metadata.naming_mode {
        AllocatedNamingMode::ContentAddressed => allocation_destination(
            hash,
            AllocationSpec {
                folder,
                naming: AllocatedName {
                    prefix: &metadata.prefix,
                    suffix: &metadata.suffix,
                    extension: metadata.extension.as_deref(),
                },
                media_type: &metadata.media_type,
            },
        ),
        AllocatedNamingMode::Custom => {
            let basename = current_path.rsplit('/').next().unwrap_or(current_path);
            custom_destination(folder, basename, &metadata.media_type)
        }
    }
}

pub(super) fn normalize_folder(folder: &str) -> Result<String, StoreError> {
    if folder.is_empty() {
        Ok(String::new())
    } else {
        normalize_rel(folder)
    }
}

pub(super) fn join_folder(folder: &str, basename: &str) -> String {
    if folder.is_empty() {
        basename.to_string()
    } else {
        format!("{folder}/{basename}")
    }
}

pub(super) fn normalize_media_type(media_type: &str) -> Result<String, StoreError> {
    let media_type = media_type.trim();
    media_type
        .parse::<mime_guess::mime::Mime>()
        .map(|media_type| media_type.to_string())
        .map_err(|_| StoreError::InvalidAllocatedName)
}

pub(super) fn pending_fingerprint(input: &PendingFingerprint<'_>) -> String {
    let hash_hex = input.hash.to_hex();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-pending-allocation-v1\0");
    for value in [
        input.site,
        input.folder,
        hash_hex.as_str(),
        input.media_type,
    ] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(&input.content_size.to_le_bytes());
    hash_file_expiry(&mut hasher, input.expiry);
    hasher.update(input.authorization_hash.unwrap_or("").as_bytes());
    for value in [
        input.extension.unwrap_or(""),
        input.expected_tree_hash.unwrap_or(""),
    ] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

pub(super) fn parse_pending_request_metadata(
    value: &str,
) -> Result<PendingRequestMetadata, StoreError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Ok(PendingRequestMetadata {
            request_fingerprint: value.to_string(),
            expiry: FileExpiry::Preserve,
            authorization_hash: None,
            extension: None,
            expected_tree_hash: None,
            legacy_fingerprint: true,
            sanitized: TokenCounts::default(),
        });
    }
    serde_json::from_str(value)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))
}

pub(super) fn legacy_pending_fingerprint(
    site: &str,
    folder: &str,
    hash: ContentHash,
    content_size: u64,
    media_type: &str,
) -> String {
    let hash_hex = hash.to_hex();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-pending-allocation-v1\0");
    for value in [site, folder, hash_hex.as_str(), media_type] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(&content_size.to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

pub(super) fn cancellation_fingerprint(
    site: &str,
    token: &str,
    folder: &str,
    expected_tree_hash: Option<&str>,
    authorization_hash: Option<&str>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-allocation-cancellation-v1\0");
    for value in [
        site,
        token,
        folder,
        expected_tree_hash.unwrap_or(""),
        authorization_hash.unwrap_or(""),
    ] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

pub(super) fn pending_finalize_fingerprint(input: &PendingFinalizeFingerprint<'_>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-pending-finalize-v1\0");
    hasher.update(input.site.as_bytes());
    hasher.update(&[0]);
    hasher.update(input.token.as_bytes());
    hasher.update(&[0]);
    hasher.update(input.folder.unwrap_or("").as_bytes());
    hasher.update(&[0]);
    match input.final_name {
        #[cfg(test)]
        PendingFinalName::Generated(naming) => {
            hasher.update(&[1]);
            for value in [naming.prefix, naming.suffix, naming.extension.unwrap_or("")] {
                hasher.update(&(value.len() as u64).to_le_bytes());
                hasher.update(value.as_bytes());
            }
        }
        PendingFinalName::Custom(basename) => {
            hasher.update(&[2]);
            hasher.update(basename.as_bytes());
        }
    }
    hash_file_expiry(&mut hasher, input.expiry);
    hasher.update(input.authorization_hash.unwrap_or("").as_bytes());
    hasher.update(input.expected_tree_hash.unwrap_or("").as_bytes());
    hasher.finalize().to_hex().to_string()
}

pub(super) fn allocated_metadata_locked(
    db: &mut SqliteConnection,
    site_id: i64,
    path: &str,
) -> Result<AllocatedMetadata, StoreError> {
    let (hash, size, naming_mode, prefix, suffix, extension, media_type) = allocated_entries::table
        .find((site_id, path))
        .select((
            allocated_entries::hash,
            allocated_entries::size,
            allocated_entries::naming_mode,
            allocated_entries::prefix,
            allocated_entries::suffix,
            allocated_entries::extension,
            allocated_entries::media_type,
        ))
        .first::<(
            ContentHash,
            i64,
            i64,
            String,
            String,
            Option<String>,
            String,
        )>(db)
        .map_err(map_sql)?;
    Ok(AllocatedMetadata {
        hash,
        size,
        naming_mode: AllocatedNamingMode::try_from(naming_mode)?,
        prefix,
        suffix,
        extension,
        media_type,
    })
}

pub(super) fn allocation_metadata_matches(
    metadata: &AllocatedMetadata,
    destination: &AllocationDestination,
) -> bool {
    metadata.naming_mode == destination.naming_mode
        && metadata.prefix == destination.prefix
        && metadata.suffix == destination.suffix
        && metadata.extension == destination.extension
        && metadata.media_type == destination.media_type
}

pub(super) fn validate_allocated_commit_locked(
    tx: &mut SqliteConnection,
    site_id: i64,
    current_path: Option<&str>,
    destination: &AllocationDestination,
    staged_hash: ContentHash,
    expected_content_hash: Option<&str>,
) -> Result<bool, StoreError> {
    if let Some(current_path) = current_path {
        let current_kind = site_entries::table
            .find((site_id, current_path))
            .select(site_entries::kind)
            .first::<i64>(tx)
            .map_err(map_sql)?;
        if current_kind != database::schema::ALLOCATED_ENTRY_KIND {
            return Err(StoreError::NotFound);
        }
        let current_hash = allocated_entries::table
            .find((site_id, current_path))
            .select(allocated_entries::hash)
            .first::<ContentHash>(tx)?;
        if expected_content_hash
            .is_some_and(|expected| ContentHash::try_from(expected).ok() != Some(current_hash))
        {
            return Err(stale_content_hash_error(current_hash));
        }
        if current_hash == staged_hash {
            return Ok(false);
        }
    }
    if current_path == Some(destination.path.as_str()) {
        return Ok(true);
    }
    let destination_kind = site_entries::table
        .find((site_id, destination.path.as_str()))
        .select(site_entries::kind)
        .first::<i64>(tx)
        .optional()?;
    let Some(destination_kind) = destination_kind else {
        return Ok(true);
    };
    if destination_kind != database::schema::ALLOCATED_ENTRY_KIND {
        return Err(StoreError::DestinationConflict);
    }
    let metadata = allocated_metadata_locked(tx, site_id, &destination.path)?;
    if metadata.hash != staged_hash || !allocation_metadata_matches(&metadata, destination) {
        return Err(StoreError::DestinationConflict);
    }
    Ok(false)
}

pub(super) fn safe_name_fragment(fragment: &str) -> bool {
    fragment.bytes().all(|byte| {
        byte.is_ascii_graphic() && byte != b'/' && byte != b'\\' && byte != b'?' && byte != b'#'
    })
}

pub(super) fn normalize_extension(extension: &str) -> Result<String, StoreError> {
    let mut normalized = String::new();
    let mut separator = false;
    for character in extension.trim_matches('.').chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !normalized.is_empty() {
                normalized.push('-');
            }
            normalized.push(character.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    if normalized.is_empty() {
        Err(StoreError::InvalidAllocatedName)
    } else {
        Ok(normalized)
    }
}

pub fn normalize_allocated_extension(extension: &str) -> Result<String, StoreError> {
    normalize_extension(extension)
}

pub(super) fn prune_pending_locked(
    tx: &mut SqliteConnection,
    now: i64,
) -> Result<usize, diesel::result::Error> {
    diesel::delete(pending_allocations::table.filter(pending_allocations::expires.le(now)))
        .execute(tx)
}

pub(super) fn pending_allocation_replay(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    site_id: i64,
    now: i64,
) -> Result<Option<PendingAllocation>, StoreError> {
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
    let Some((stored_fingerprint, kind, metadata)) = record else {
        return Ok(None);
    };
    if stored_fingerprint != fingerprint || kind != IdempotencyKind::PendingAllocation as i64 {
        return Err(StoreError::IdempotencyConflict);
    }
    let mut result = serde_json::from_str::<PendingAllocation>(&metadata)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    let exists = pending_allocations::table
        .find(&result.token)
        .filter(pending_allocations::site_id.eq(site_id))
        .filter(pending_allocations::expires.gt(now))
        .select(pending_allocations::token)
        .first::<String>(tx)
        .optional()?
        .is_some();
    if !exists {
        return Err(StoreError::InvalidPendingAllocation);
    }
    result.replayed = true;
    Ok(Some(result))
}

pub(super) fn cancellation_replay(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
) -> Result<Option<AllocationCancellation>, StoreError> {
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
    if stored_fingerprint != fingerprint
        || stored_kind != IdempotencyKind::AllocationCancellation as i64
    {
        return Err(StoreError::IdempotencyConflict);
    }
    let mut result = serde_json::from_str::<AllocationCancellation>(&metadata)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    result.replayed = true;
    Ok(Some(result))
}

pub(super) fn store_cancellation(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    result: &AllocationCancellation,
    now: i64,
) -> Result<(), StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(());
    };
    validate_idempotency_key(&idempotency.key)?;
    let metadata = serde_json::to_string(result)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    diesel::insert_into(idempotency_records::table)
        .values((
            idempotency_records::key_hash.eq(idempotency_key_hash(&idempotency.key)),
            idempotency_records::fingerprint.eq(fingerprint),
            idempotency_records::operation_kind.eq(IdempotencyKind::AllocationCancellation as i64),
            idempotency_records::result_metadata.eq(metadata),
            idempotency_records::expires.eq(now + IDEMPOTENCY_RETENTION_MILLIS),
        ))
        .execute(tx)?;
    Ok(())
}

pub(super) fn store_pending_allocation(
    tx: &mut SqliteConnection,
    idempotency: Option<&Idempotency>,
    fingerprint: &str,
    result: &PendingAllocation,
    now: i64,
) -> Result<(), StoreError> {
    let Some(idempotency) = idempotency else {
        return Ok(());
    };
    validate_idempotency_key(&idempotency.key)?;
    let metadata = serde_json::to_string(result)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    diesel::insert_into(idempotency_records::table)
        .values((
            idempotency_records::key_hash.eq(idempotency_key_hash(&idempotency.key)),
            idempotency_records::fingerprint.eq(fingerprint),
            idempotency_records::operation_kind.eq(IdempotencyKind::PendingAllocation as i64),
            idempotency_records::result_metadata.eq(metadata),
            idempotency_records::expires.eq(now + IDEMPOTENCY_RETENTION_MILLIS),
        ))
        .execute(tx)?;
    Ok(())
}

pub(super) fn allocated_entry_mutation_fingerprint(
    input: &AllocatedEntryFingerprint<'_>,
) -> String {
    let hash_hex = input.hash.to_hex();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"symbol-allocated-entry-mutation-v2\0");
    for value in [
        input.name,
        input.current_path.unwrap_or(""),
        &input.destination.path,
        hash_hex.as_str(),
        &input.destination.prefix,
        &input.destination.suffix,
        input.destination.extension.as_deref().unwrap_or(""),
        &input.destination.media_type,
    ] {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(&(input.destination.naming_mode as i64).to_le_bytes());
    hasher.update(&(input.kind as i64).to_le_bytes());
    hash_file_expiry(&mut hasher, input.expiry);
    hasher.update(input.expected_tree_hash.unwrap_or("").as_bytes());
    hasher.finalize().to_hex().to_string()
}

impl Store {
    #[cfg(test)]
    pub fn allocate_bytes(
        &self,
        name: &str,
        bytes: &[u8],
        spec: AllocationSpec<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        self.allocate_source(name, AllocationSource::Bytes(bytes), spec, options)
    }

    pub fn allocate_file(
        &self,
        name: &str,
        source: &Path,
        spec: AllocationSpec<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        self.allocate_source(name, AllocationSource::File(source), spec, options)
    }

    pub fn allocate_source(
        &self,
        name: &str,
        source: AllocationSource<'_>,
        spec: AllocationSpec<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        let staged = self.stage_allocation_source(source, "")?;
        let destination = allocation_destination(staged.hash, spec)?;
        self.run_before_content_commit();
        self.commit_allocated(
            name,
            None,
            &destination,
            &staged,
            None,
            None,
            options,
            UndoKind::Allocate,
        )
    }

    #[cfg(test)]
    pub fn replace_allocated(
        &self,
        name: &str,
        current_path: &str,
        source: AllocationSource<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        let current_path = normalize_rel(current_path)?;
        let staged = self.stage_allocation_source(source, "")?;
        let request_fingerprint = content_mutation_request_fingerprint(
            name,
            &current_path,
            staged.hash,
            None,
            options.expected_tree_hash,
            UndoKind::Replace,
        );
        if let Some(replay) = self.replay_content_request(name, options, &request_fingerprint)? {
            return Ok(replay);
        }
        let metadata = self.allocated_metadata(name, &current_path)?;
        let destination = relocated_destination(staged.hash, &current_path, &metadata)?;
        self.run_before_content_commit();
        self.commit_allocated(
            name,
            Some(&current_path),
            &destination,
            &staged,
            None,
            None,
            options,
            UndoKind::Replace,
        )
    }

    #[cfg(test)]
    pub fn propose_allocation(
        &self,
        name: &str,
        source: AllocationSource<'_>,
        spec: PendingAllocationSpec<'_>,
        authorization: Option<&ManagementToken>,
    ) -> Result<PendingAllocation, StoreError> {
        self.propose_allocation_idempotent(
            name,
            source,
            spec,
            FileMutationOptions {
                authorization,
                ..FileMutationOptions::default()
            },
        )
    }

    pub fn propose_allocation_idempotent(
        &self,
        name: &str,
        source: AllocationSource<'_>,
        spec: PendingAllocationSpec<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<PendingAllocation, StoreError> {
        let name = parse_site_name(name)?;
        let staged = self.stage_allocation_source(source, "")?;
        let folder = normalize_folder(spec.folder)?;
        let media_type = normalize_media_type(spec.media_type)?;
        let extension = spec
            .extension
            .map(normalize_allocated_extension)
            .transpose()?;
        let expiry = validate_file_expiry(options.expiry)?;
        let authorization_hash = options.authorization.map(authorization_fingerprint);
        let request_fingerprint = pending_fingerprint(&PendingFingerprint {
            site: name,
            folder: &folder,
            hash: staged.hash,
            content_size: staged.size.cast_unsigned(),
            media_type: &media_type,
            expiry,
            authorization_hash: authorization_hash.as_deref(),
            extension: extension.as_deref(),
            expected_tree_hash: options.expected_tree_hash,
        });
        let now = self.now_millis();
        let expires = now + PENDING_RETENTION_MILLIS;
        self.run_before_content_commit();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, options.authorization)?;
        let site_id = site_id_locked(&mut tx, name)?;
        prune_idempotency_locked(&mut tx, now)?;
        prune_pending_locked(&mut tx, now)?;
        if let Some(replay) = pending_allocation_replay(
            &mut tx,
            options.idempotency,
            &request_fingerprint,
            site_id,
            now,
        )? {
            tx.commit()?;
            return Ok(replay);
        }
        check_tree_precondition(&mut tx, name, options.expected_tree_hash)?;
        let token = undo_token()?;
        self.materialize(&staged)?;
        ensure_blob_locked(&mut tx, &staged)?;
        let request_metadata = serde_json::to_string(&PendingRequestMetadata {
            request_fingerprint: request_fingerprint.clone(),
            expiry,
            authorization_hash,
            extension: extension.clone(),
            expected_tree_hash: options.expected_tree_hash.map(str::to_string),
            legacy_fingerprint: false,
            sanitized: staged.sanitized,
        })
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
        diesel::insert_into(pending_allocations::table)
            .values((
                pending_allocations::token.eq(&token),
                pending_allocations::site_id.eq(site_id),
                pending_allocations::folder.eq(&folder),
                pending_allocations::hash.eq(staged.hash),
                pending_allocations::size.eq(staged.size),
                pending_allocations::media_type.eq(&media_type),
                pending_allocations::request_fingerprint.eq(request_metadata),
                pending_allocations::created.eq(now),
                pending_allocations::expires.eq(expires),
            ))
            .execute(&mut *tx)?;
        let (content_revision, tree_hash) = site_revision_locked(&mut tx, name)?;
        let result = PendingAllocation {
            token,
            folder,
            hash: format!("blake3:{}", staged.hash.to_hex()),
            size: staged.size.cast_unsigned(),
            media_type,
            extension,
            expires_at: format_timestamp(expires),
            tree_hash: tree_hash.to_wire(),
            content_revision,
            replayed: false,
        };
        store_pending_allocation(
            &mut tx,
            options.idempotency,
            &request_fingerprint,
            &result,
            now,
        )?;
        tx.commit()?;
        drop(db);
        Ok(result)
    }

    #[cfg(test)]
    pub fn finalize_allocation(
        &self,
        name: &str,
        token: &str,
        naming: AllocatedName<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        self.finalize_pending(
            name,
            token,
            None,
            PendingFinalName::Generated(naming),
            options,
        )
    }

    #[cfg(test)]
    pub fn finalize_allocation_custom(
        &self,
        name: &str,
        token: &str,
        basename: &str,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        self.finalize_pending(
            name,
            token,
            None,
            PendingFinalName::Custom(basename),
            options,
        )
    }

    pub fn finalize_allocation_custom_in_folder(
        &self,
        name: &str,
        token: &str,
        folder: &str,
        basename: &str,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        self.finalize_pending(
            name,
            token,
            Some(folder),
            PendingFinalName::Custom(basename),
            options,
        )
    }

    #[expect(clippy::too_many_lines)]
    pub(super) fn finalize_pending(
        &self,
        name: &str,
        token: &str,
        expected_folder: Option<&str>,
        final_name: PendingFinalName<'_>,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocatedFile, StoreError> {
        let name = parse_site_name(name)?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, options.authorization)?;
        let expected_folder = expected_folder.map(normalize_folder).transpose()?;
        let authorization_hash = options.authorization.map(authorization_fingerprint);
        let finalize_fingerprint = pending_finalize_fingerprint(&PendingFinalizeFingerprint {
            site: name,
            token,
            folder: expected_folder.as_deref(),
            final_name,
            expiry: options.expiry,
            authorization_hash: authorization_hash.as_deref(),
            expected_tree_hash: options.expected_tree_hash,
        });
        prune_idempotency_locked(&mut tx, now)?;
        if let Some(replay) =
            entry_mutation_replay(&mut tx, options.idempotency, &finalize_fingerprint)?
        {
            return Ok(replay);
        }
        check_tree_precondition(&mut tx, name, options.expected_tree_hash)?;
        let site_id = site_id_locked(&mut tx, name)?;
        let pending = pending_allocations::table
            .find(token)
            .filter(pending_allocations::site_id.eq(site_id))
            .filter(pending_allocations::expires.gt(now))
            .select((
                pending_allocations::folder,
                pending_allocations::hash,
                pending_allocations::size,
                pending_allocations::media_type,
                pending_allocations::request_fingerprint,
            ))
            .first::<(String, ContentHash, i64, String, String)>(&mut *tx)
            .optional()?
            .map(|row| {
                let request = parse_pending_request_metadata(&row.4)?;
                Ok::<_, StoreError>(PendingMetadata {
                    folder: row.0,
                    hash: row.1,
                    size: row.2,
                    media_type: row.3,
                    request_fingerprint: request.request_fingerprint,
                    expiry: request.expiry,
                    authorization_hash: request.authorization_hash,
                    extension: request.extension,
                    expected_tree_hash: request.expected_tree_hash,
                    legacy_fingerprint: request.legacy_fingerprint,
                    sanitized: request.sanitized,
                })
            })
            .transpose()?
            .ok_or(StoreError::InvalidPendingAllocation)?;
        if expected_folder
            .as_deref()
            .is_some_and(|expected| expected != pending.folder)
        {
            return Err(StoreError::InvalidPendingAllocation);
        }
        let expected_pending_fingerprint = if pending.legacy_fingerprint {
            legacy_pending_fingerprint(
                name,
                &pending.folder,
                pending.hash,
                pending.size.cast_unsigned(),
                &pending.media_type,
            )
        } else {
            pending_fingerprint(&PendingFingerprint {
                site: name,
                folder: &pending.folder,
                hash: pending.hash,
                content_size: pending.size.cast_unsigned(),
                media_type: &pending.media_type,
                expiry: pending.expiry,
                authorization_hash: pending.authorization_hash.as_deref(),
                extension: pending.extension.as_deref(),
                expected_tree_hash: pending.expected_tree_hash.as_deref(),
            })
        };
        if pending.request_fingerprint != expected_pending_fingerprint {
            return Err(StoreError::InvalidPendingAllocation);
        }
        if !pending.legacy_fingerprint && pending.authorization_hash != authorization_hash {
            return Err(StoreError::Unauthorized);
        }
        if pending.expiry != FileExpiry::Preserve
            && options.expiry != FileExpiry::Preserve
            && options.expiry != pending.expiry
        {
            return Err(StoreError::IdempotencyConflict);
        }
        let options = FileMutationOptions {
            expiry: validate_file_expiry(if pending.expiry == FileExpiry::Preserve {
                options.expiry
            } else {
                pending.expiry
            })?,
            ..options
        };
        let destination = match final_name {
            #[cfg(test)]
            PendingFinalName::Generated(naming) => allocation_destination(
                pending.hash,
                AllocationSpec {
                    folder: &pending.folder,
                    naming,
                    media_type: &pending.media_type,
                },
            )?,
            PendingFinalName::Custom(basename) => {
                custom_destination(&pending.folder, basename, &pending.media_type)?
            }
        };
        let staged = StagedFile {
            path: destination.path.clone(),
            size: pending.size,
            hash: pending.hash,
            source: StagedSource::File(self.inner.blob_files.path(pending.hash)),
            sanitized: pending.sanitized,
        };
        let result = self.commit_allocated_locked(
            &mut tx,
            name,
            None,
            &destination,
            &staged,
            None,
            options,
            UndoKind::Allocate,
            now,
        )?;
        store_entry_mutation(
            &mut tx,
            options.idempotency,
            &finalize_fingerprint,
            &result,
            now,
        )?;
        diesel::delete(pending_allocations::table.find(token)).execute(&mut *tx)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(result)
    }

    #[cfg(test)]
    pub fn cancel_allocation(
        &self,
        name: &str,
        token: &str,
        authorization: Option<&ManagementToken>,
    ) -> Result<(), StoreError> {
        if self.cancel_allocation_in_folder(name, token, None, authorization)? {
            Ok(())
        } else {
            Err(StoreError::InvalidPendingAllocation)
        }
    }

    pub fn cancel_allocation_idempotent_for_folder(
        &self,
        name: &str,
        token: &str,
        folder: &str,
        options: FileMutationOptions<'_>,
    ) -> Result<AllocationCancellation, StoreError> {
        let name = parse_site_name(name)?;
        let folder = normalize_folder(folder)?;
        let authorization_hash = options.authorization.map(authorization_fingerprint);
        let fingerprint = cancellation_fingerprint(
            name,
            token,
            &folder,
            options.expected_tree_hash,
            authorization_hash.as_deref(),
        );
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, options.authorization)?;
        prune_idempotency_locked(&mut tx, now)?;
        if let Some(replay) = cancellation_replay(&mut tx, options.idempotency, &fingerprint)? {
            return Ok(replay);
        }
        check_tree_precondition(&mut tx, name, options.expected_tree_hash)?;
        let site_id = site_id_locked(&mut tx, name)?;
        let pending = pending_allocations::table
            .find(token)
            .select((
                pending_allocations::site_id,
                pending_allocations::folder,
                pending_allocations::request_fingerprint,
            ))
            .first::<(i64, String, String)>(&mut *tx)
            .optional()?
            .ok_or(StoreError::InvalidPendingAllocation)?;
        if pending.0 != site_id || pending.1 != folder {
            return Err(StoreError::InvalidPendingAllocation);
        }
        let request = parse_pending_request_metadata(&pending.2)?;
        if !request.legacy_fingerprint && request.authorization_hash != authorization_hash {
            return Err(StoreError::Unauthorized);
        }
        let deleted = diesel::delete(
            pending_allocations::table
                .find(token)
                .filter(pending_allocations::site_id.eq(site_id)),
        )
        .execute(&mut *tx)?;
        if deleted != 1 {
            return Err(StoreError::InvalidPendingAllocation);
        }
        let (revision, tree_hash) = sites::table
            .find(site_id)
            .select((sites::content_revision, sites::tree_hash))
            .first::<(i64, TreeHash)>(&mut *tx)?;
        let result = AllocationCancellation {
            replayed: false,
            tree_hash: tree_hash.to_wire(),
            content_revision: revision.cast_unsigned(),
        };
        store_cancellation(&mut tx, options.idempotency, &fingerprint, &result, now)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(result)
    }

    #[cfg(test)]
    pub(super) fn cancel_allocation_in_folder(
        &self,
        name: &str,
        token: &str,
        folder: Option<&str>,
        authorization: Option<&ManagementToken>,
    ) -> Result<bool, StoreError> {
        let name = parse_site_name(name)?;
        let folder = folder.map(normalize_folder).transpose()?;
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        authorize_locked(&mut tx, name, authorization)?;
        let site_id = site_id_locked(&mut tx, name)?;
        let pending = pending_allocations::table
            .find(token)
            .select((
                pending_allocations::site_id,
                pending_allocations::folder,
                pending_allocations::request_fingerprint,
            ))
            .first::<(i64, String, String)>(&mut *tx)
            .optional()?;
        let Some((stored_site_id, stored_folder, request_metadata)) = pending else {
            return Ok(false);
        };
        if stored_site_id != site_id {
            return Err(StoreError::InvalidPendingAllocation);
        }
        if folder
            .as_deref()
            .is_some_and(|expected| expected != stored_folder)
        {
            return Err(StoreError::InvalidPendingAllocation);
        }
        let request = parse_pending_request_metadata(&request_metadata)?;
        if !request.legacy_fingerprint
            && request.authorization_hash != authorization.map(authorization_fingerprint)
        {
            return Err(StoreError::Unauthorized);
        }
        diesel::delete(
            pending_allocations::table
                .find(token)
                .filter(pending_allocations::site_id.eq(site_id)),
        )
        .execute(&mut *tx)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(true)
    }

    #[cfg(test)]
    pub fn prune_pending_allocations(&self) -> Result<usize, StoreError> {
        let now = self.now_millis();
        let mut db = self.inner.writer.lock().unwrap();
        let mut tx = DbTransaction::begin(&mut db)?;
        let removed_pending = prune_pending_locked(&mut tx, now)?;
        let removed = gc_blobs(&mut tx, now)?;
        tx.commit()?;
        drop(db);
        self.remove_blob_files(&removed);
        Ok(removed_pending)
    }

    pub(super) fn allocated_metadata(
        &self,
        name: &str,
        path: &str,
    ) -> Result<AllocatedMetadata, StoreError> {
        let mut db = self.inner.readers.get();
        let site_id = site_id_locked(&mut db, name)?;
        allocated_metadata_locked(&mut db, site_id, path)
    }

    #[expect(clippy::too_many_arguments)]
    pub(super) fn commit_allocated(
        &self,
        name: &str,
        current_path: Option<&str>,
        destination: &AllocationDestination,
        staged: &StagedFile,
        expected_content_hash: Option<&str>,
        request_fingerprint: Option<&str>,
        options: FileMutationOptions<'_>,
        kind: UndoKind,
    ) -> Result<AllocatedFile, StoreError> {
        let now = self.now_millis();
        let options = FileMutationOptions {
            expiry: validate_file_expiry(options.expiry)?,
            ..options
        };
        let fingerprint = allocated_entry_mutation_fingerprint(&AllocatedEntryFingerprint {
            name,
            current_path,
            destination,
            hash: staged.hash,
            kind,
            expiry: options.expiry,
            expected_tree_hash: options.expected_tree_hash,
        });
        let computed_request_fingerprint;
        let request_fingerprint = if let Some(request_fingerprint) = request_fingerprint {
            request_fingerprint
        } else {
            computed_request_fingerprint = content_mutation_request_fingerprint(
                name,
                current_path.unwrap_or(""),
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
        if let Some(current_path) = current_path {
            reject_alias_write_locked(&mut tx, site_id, current_path, false)?;
        }
        reject_alias_write_locked(&mut tx, site_id, &destination.path, false)?;
        let needs_materialization = validate_allocated_commit_locked(
            &mut tx,
            site_id,
            current_path,
            destination,
            staged.hash,
            expected_content_hash,
        )?;
        if needs_materialization {
            self.materialize(staged)?;
        }
        let result = self.commit_allocated_locked(
            &mut tx,
            name,
            current_path,
            destination,
            staged,
            expected_content_hash,
            options,
            kind,
            now,
        )?;
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

    #[expect(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(super) fn commit_allocated_locked(
        &self,
        tx: &mut SqliteConnection,
        name: &str,
        current_path: Option<&str>,
        destination: &AllocationDestination,
        staged: &StagedFile,
        expected_content_hash: Option<&str>,
        options: FileMutationOptions<'_>,
        kind: UndoKind,
        now: i64,
    ) -> Result<AllocatedFile, StoreError> {
        let site_id = site_id_locked(tx, name)?;
        if let Some(current_path) = current_path {
            let current_kind = site_entries::table
                .find((site_id, current_path))
                .select(site_entries::kind)
                .first::<i64>(tx)
                .map_err(map_sql)?;
            if current_kind != database::schema::ALLOCATED_ENTRY_KIND {
                return Err(StoreError::NotFound);
            }
            let (current_hash, current_size) = allocated_entries::table
                .find((site_id, current_path))
                .select((allocated_entries::hash, allocated_entries::size))
                .first::<(ContentHash, i64)>(tx)?;
            if expected_content_hash
                .is_some_and(|expected| ContentHash::try_from(expected).ok() != Some(current_hash))
            {
                return Err(stale_content_hash_error(current_hash));
            }
            if current_hash == staged.hash {
                let (revision, tree_hash) = site_revision_locked(tx, name)?;
                return Ok(AllocatedFile {
                    path: current_path.to_string(),
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
                });
            }
        }
        let same_path = current_path == Some(destination.path.as_str());
        let destination_kind = if same_path {
            None
        } else {
            site_entries::table
                .find((site_id, destination.path.as_str()))
                .select(site_entries::kind)
                .first::<i64>(tx)
                .optional()?
        };
        if destination_kind
            .is_some_and(|entry_kind| entry_kind != database::schema::ALLOCATED_ENTRY_KIND)
        {
            return Err(StoreError::DestinationConflict);
        }
        let reused_size = if destination_kind.is_some() {
            let metadata = allocated_metadata_locked(tx, site_id, &destination.path)?;
            if metadata.hash != staged.hash || !allocation_metadata_matches(&metadata, destination)
            {
                return Err(StoreError::DestinationConflict);
            }
            Some(metadata.size)
        } else {
            None
        };
        if current_path.is_none()
            && let Some(size) = reused_size
        {
            if file_expiry_change_required(tx, site_id, &destination.path, options.expiry)? {
                let changed_paths = [destination.path.as_str()];
                let undo = snapshot_entry_deltas(
                    tx,
                    name,
                    kind,
                    &format!("restore previous allocated entry in {name}"),
                    &changed_paths,
                    now,
                )?;
                apply_file_expiry(
                    tx,
                    site_id,
                    &destination.path,
                    size.cast_unsigned(),
                    options.expiry,
                    now,
                )?;
                finish_entry_mutation(tx, &self.inner.blob_files, site_id, &changed_paths, now)?;
                let (revision, tree_hash) = site_revision_locked(tx, name)?;
                return Ok(AllocatedFile {
                    path: destination.path.clone(),
                    hash: format!("blake3:{}", staged.hash.to_hex()),
                    size: size.cast_unsigned(),
                    changed: true,
                    replayed: false,
                    mutation: Some(MutationResult {
                        created: false,
                        changed: true,
                        replayed: false,
                        files: 1,
                        revision,
                        tree_hash: tree_hash.to_wire(),
                        undo: Some(undo),
                        sanitized: staged.sanitized,
                    }),
                });
            }
            let (revision, tree_hash) = site_revision_locked(tx, name)?;
            return Ok(AllocatedFile {
                path: destination.path.clone(),
                hash: format!("blake3:{}", staged.hash.to_hex()),
                size: size.cast_unsigned(),
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
            });
        }
        let mut changed_paths = Vec::with_capacity(2);
        if let Some(current_path) = current_path {
            changed_paths.push(current_path);
        }
        if reused_size.is_none() && current_path != Some(destination.path.as_str()) {
            changed_paths.push(destination.path.as_str());
        }
        let undo = snapshot_entry_deltas(
            tx,
            name,
            kind,
            &format!("restore previous allocated entry in {name}"),
            &changed_paths,
            now,
        )?;
        ensure_blob_locked(tx, staged)?;
        if let Some(current_path) = current_path {
            let previous_size = allocated_entries::table
                .find((site_id, current_path))
                .select(allocated_entries::size)
                .first::<i64>(tx)?;
            if reused_size.is_some() {
                diesel::delete(expiry_policies::table.find((site_id, current_path))).execute(tx)?;
            } else {
                diesel::update(expiry_policies::table.find((site_id, current_path)))
                    .set(expiry_policies::path.eq(&destination.path))
                    .execute(tx)?;
            }
            diesel::delete(site_entries::table.find((site_id, current_path))).execute(tx)?;
            adjust_aggregates_locked(tx, site_id, current_path, -previous_size, -1)?;
        }
        if reused_size.is_none() {
            diesel::insert_into(site_entries::table)
                .values((
                    site_entries::site_id.eq(site_id),
                    site_entries::path.eq(&destination.path),
                    site_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                ))
                .execute(tx)?;
            diesel::insert_into(allocated_entries::table)
                .values((
                    allocated_entries::site_id.eq(site_id),
                    allocated_entries::path.eq(&destination.path),
                    allocated_entries::kind.eq(database::schema::ALLOCATED_ENTRY_KIND),
                    allocated_entries::hash.eq(staged.hash),
                    allocated_entries::size.eq(staged.size),
                    allocated_entries::naming_mode.eq(destination.naming_mode as i64),
                    allocated_entries::prefix.eq(&destination.prefix),
                    allocated_entries::suffix.eq(&destination.suffix),
                    allocated_entries::extension.eq(&destination.extension),
                    allocated_entries::media_type.eq(&destination.media_type),
                ))
                .execute(tx)?;
            adjust_aggregates_locked(tx, site_id, &destination.path, staged.size, 1)?;
        }
        apply_file_expiry(
            tx,
            site_id,
            &destination.path,
            reused_size.unwrap_or(staged.size).cast_unsigned(),
            options.expiry,
            now,
        )?;
        finish_entry_mutation(tx, &self.inner.blob_files, site_id, &changed_paths, now)?;
        let (revision, tree_hash) = site_revision_locked(tx, name)?;
        Ok(AllocatedFile {
            path: destination.path.clone(),
            hash: format!("blake3:{}", staged.hash.to_hex()),
            size: reused_size.unwrap_or(staged.size).cast_unsigned(),
            changed: true,
            replayed: false,
            mutation: Some(MutationResult {
                created: current_path.is_none(),
                changed: true,
                replayed: false,
                files: 1,
                revision,
                tree_hash: tree_hash.to_wire(),
                undo: Some(undo),
                sanitized: staged.sanitized,
            }),
        })
    }

    pub(super) fn stage_allocation_source(
        &self,
        source: AllocationSource<'_>,
        path: &str,
    ) -> Result<StagedFile, StoreError> {
        match source {
            #[cfg(test)]
            AllocationSource::Bytes(bytes) => Ok(stage_bytes(path, bytes)),
            AllocationSource::File(source) => {
                let temporary = TemporaryDirectory::create(self.tmp_dir("source"))?;
                let copied = temporary.path().join("content");
                let mut input = fs::File::open(source)?;
                let mut output = fs::File::create(&copied)?;
                io::copy(&mut input, &mut output)?;
                output.sync_all()?;
                let staged = stage_temporary_file(path, copied)?;
                temporary.persist();
                Ok(staged)
            }
        }
    }
}
