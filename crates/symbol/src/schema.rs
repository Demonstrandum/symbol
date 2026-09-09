diesel::table! {
    sites (id) {
        id -> BigInt,
        name -> Text,
        created -> Nullable<BigInt>,
        updated -> BigInt,
        public_url -> Text,
        content_revision -> BigInt,
        tree_hash -> Binary,
        creator_kind -> Nullable<BigInt>,
        creator_hash -> Nullable<Binary>,
        claim_hash -> Nullable<Binary>,
        management_hash -> Nullable<Binary>,
        management_status -> BigInt,
    }
}

diesel::table! {
    blobs (hash) {
        hash -> Binary,
        bytes -> Binary,
        size -> BigInt,
    }
}

diesel::table! {
    files (site_id, path) {
        site_id -> BigInt,
        path -> Text,
        kind -> BigInt,
        hash -> Binary,
        size -> BigInt,
    }
}

diesel::table! {
    site_entries (site_id, path) {
        site_id -> BigInt,
        path -> Text,
        kind -> BigInt,
    }
}

diesel::table! {
    metadata (key) {
        key -> Text,
        value -> Text,
    }
}

diesel::table! {
    undo_operations (token) {
        token -> Text,
        kind -> BigInt,
        description -> Text,
        created -> BigInt,
        expires -> BigInt,
        consumed -> BigInt,
        rowid -> BigInt,
    }
}

diesel::table! {
    undo_names (token, name) {
        token -> Text,
        name -> Text,
    }
}

diesel::table! {
    site_events (id) {
        id -> BigInt,
        site_id -> BigInt,
        kind -> BigInt,
        occurred -> BigInt,
        files -> BigInt,
    }
}

diesel::table! {
    undo_sites (token) {
        token -> Text,
        name -> Text,
        existed -> BigInt,
        public_url -> Text,
        created -> Nullable<BigInt>,
        updated -> BigInt,
        content_revision -> BigInt,
        tree_hash -> Binary,
    }
}

diesel::table! {
    undo_files (token, path) {
        token -> Text,
        path -> Text,
        hash -> Binary,
        size -> BigInt,
    }
}

diesel::table! {
    expiry_policies (site_id, path) {
        site_id -> BigInt,
        path -> Text,
        target_kind -> BigInt,
        mode -> BigInt,
        duration_seconds -> Nullable<BigInt>,
        deadline -> Nullable<BigInt>,
        min_age_seconds -> Nullable<BigInt>,
        max_age_seconds -> Nullable<BigInt>,
        max_size_bytes -> Nullable<BigInt>,
        power -> Nullable<Double>,
        refreshed -> Nullable<BigInt>,
        own_deadline -> Nullable<BigInt>,
        size_bytes -> BigInt,
    }
}

diesel::table! {
    undo_expiry_policies (token, path) {
        token -> Text,
        path -> Text,
        target_kind -> BigInt,
        mode -> BigInt,
        duration_seconds -> Nullable<BigInt>,
        deadline -> Nullable<BigInt>,
        min_age_seconds -> Nullable<BigInt>,
        max_age_seconds -> Nullable<BigInt>,
        max_size_bytes -> Nullable<BigInt>,
        power -> Nullable<Double>,
        refreshed -> Nullable<BigInt>,
        own_deadline -> BigInt,
        size_bytes -> BigInt,
    }
}

diesel::table! {
    idempotency_records (key_hash) {
        key_hash -> Text,
        fingerprint -> Text,
        operation_kind -> BigInt,
        result_metadata -> Text,
        expires -> BigInt,
    }
}

diesel::table! {
    management_tombstones (name) {
        name -> Text,
        management_hash -> Binary,
        created -> BigInt,
    }
}

diesel::table! {
    management_audit (id) {
        id -> BigInt,
        site_name -> Text,
        action -> BigInt,
        occurred -> BigInt,
        source_ip -> Nullable<Text>,
    }
}

diesel::table! {
    management_idempotency (key_hash) {
        key_hash -> Text,
        fingerprint -> Text,
        expires -> BigInt,
    }
}

diesel::table! {
    path_aggregates (site_id, path) {
        site_id -> BigInt,
        path -> Text,
        logical_bytes -> BigInt,
        file_count -> BigInt,
    }
}

diesel::table! {
    undo_file_deltas (token, path) {
        token -> Text,
        path -> Text,
        existed -> BigInt,
        kind -> Nullable<BigInt>,
        hash -> Nullable<Binary>,
        size -> Nullable<BigInt>,
    }
}

diesel::table! {
    allocated_entries (site_id, path) {
        site_id -> BigInt,
        path -> Text,
        kind -> BigInt,
        hash -> Binary,
        size -> BigInt,
        naming_mode -> BigInt,
        prefix -> Text,
        suffix -> Text,
        extension -> Nullable<Text>,
        media_type -> Text,
    }
}

diesel::table! {
    pending_allocations (token) {
        token -> Text,
        site_id -> BigInt,
        folder -> Text,
        hash -> Binary,
        size -> BigInt,
        media_type -> Text,
        request_fingerprint -> Text,
        created -> BigInt,
        expires -> BigInt,
    }
}

diesel::table! {
    undo_allocated_deltas (token, path) {
        token -> Text,
        path -> Text,
        existed -> BigInt,
        hash -> Nullable<Binary>,
        size -> Nullable<BigInt>,
        naming_mode -> Nullable<BigInt>,
        prefix -> Nullable<Text>,
        suffix -> Nullable<Text>,
        extension -> Nullable<Text>,
        media_type -> Nullable<Text>,
    }
}

diesel::table! {
    aliases (site_id, path) {
        site_id -> BigInt,
        path -> Text,
        kind -> BigInt,
        canonical_target -> Text,
        resolved_kind -> Nullable<BigInt>,
        resolved_hash -> Nullable<Binary>,
        resolved_size -> Nullable<BigInt>,
    }
}

diesel::table! {
    undo_alias_deltas (token, path) {
        token -> Text,
        path -> Text,
        existed -> BigInt,
        canonical_target -> Nullable<Text>,
        resolved_kind -> Nullable<BigInt>,
        resolved_hash -> Nullable<Binary>,
        resolved_size -> Nullable<BigInt>,
    }
}

diesel::joinable!(site_entries -> sites (site_id));
diesel::joinable!(site_events -> sites (site_id));
diesel::joinable!(files -> sites (site_id));
diesel::joinable!(files -> blobs (hash));
diesel::joinable!(expiry_policies -> sites (site_id));
diesel::joinable!(path_aggregates -> sites (site_id));
diesel::joinable!(pending_allocations -> sites (site_id));
diesel::joinable!(undo_file_deltas -> undo_operations (token));
diesel::joinable!(undo_allocated_deltas -> undo_operations (token));
diesel::joinable!(undo_alias_deltas -> undo_operations (token));

diesel::allow_tables_to_appear_in_same_query!(
    sites,
    blobs,
    site_entries,
    site_events,
    files,
    metadata,
    undo_operations,
    undo_names,
    undo_sites,
    undo_files,
    expiry_policies,
    undo_expiry_policies,
    idempotency_records,
    management_tombstones,
    management_audit,
    management_idempotency,
    path_aggregates,
    undo_file_deltas,
    allocated_entries,
    pending_allocations,
    undo_allocated_deltas,
    aliases,
    undo_alias_deltas,
);
