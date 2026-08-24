diesel::table! {
    sites (id) {
        id -> BigInt,
        name -> Text,
        updated -> BigInt,
        public_url -> Text,
        content_revision -> BigInt,
        tree_hash -> Text,
        creator_kind -> Nullable<BigInt>,
        creator_hash -> Nullable<Binary>,
        claim_hash -> Nullable<Binary>,
        management_hash -> Nullable<Binary>,
        management_status -> BigInt,
    }
}

diesel::table! {
    blobs (hash) {
        hash -> Text,
        bytes -> Binary,
        size -> BigInt,
    }
}

diesel::table! {
    files (site_id, path) {
        site_id -> BigInt,
        path -> Text,
        hash -> Text,
        size -> BigInt,
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
    undo_sites (token) {
        token -> Text,
        name -> Text,
        existed -> BigInt,
        public_url -> Text,
        updated -> BigInt,
        content_revision -> BigInt,
        tree_hash -> Text,
    }
}

diesel::table! {
    undo_files (token, path) {
        token -> Text,
        path -> Text,
        hash -> Text,
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

diesel::joinable!(files -> sites (site_id));
diesel::joinable!(files -> blobs (hash));
diesel::joinable!(expiry_policies -> sites (site_id));
diesel::joinable!(path_aggregates -> sites (site_id));

diesel::allow_tables_to_appear_in_same_query!(
    sites,
    blobs,
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
);
