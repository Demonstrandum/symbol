//! Diesel view of tables while hash columns are still hex `Text` (schema v6–v10).

diesel::table! {
    files (site_id, path) {
        site_id -> BigInt,
        path -> Text,
        kind -> BigInt,
        hash -> Text,
        size -> BigInt,
    }
}
