use std::fmt::Write as _;

use axum::Json;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};

use crate::page;
use crate::store::{DirList, EntryKind, SiteList};

#[derive(serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum ListingKind {
    Site,
    Directory,
    File,
}

#[derive(serde::Serialize)]
struct ListingEntry {
    kind: ListingKind,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    files: Option<u64>,
    bytes: u64,
}

#[derive(serde::Serialize)]
struct ListingJson {
    path: String,
    files: u64,
    bytes: u64,
    entries: Vec<ListingEntry>,
}

pub fn sites(headers: &HeaderMap, list: &SiteList) -> Response {
    if wants_json(headers) {
        let entries = list
            .entries
            .iter()
            .map(|entry| ListingEntry {
                kind: ListingKind::Site,
                name: entry.name.clone(),
                files: Some(entry.files),
                bytes: entry.bytes,
            })
            .collect();
        return json_response(ListingJson {
            path: "/".to_string(),
            files: list.files,
            bytes: list.bytes,
            entries,
        });
    }
    let flavor = page::negotiate(headers);
    match flavor {
        page::Flavor::Plain | page::Flavor::Man => {
            let body = render_sites_plain(list);
            (
                [
                    (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                    (header::CACHE_CONTROL, "no-cache"),
                    (header::VARY, "Accept, User-Agent"),
                ],
                body,
            )
                .into_response()
        }
        page::Flavor::Html => {
            let mut rows = String::new();
            for entry in &list.entries {
                writeln!(
                    rows,
                    "    <a class=\"row\" href=\"/{name}/FILES/\"><span class=\"name\">{esc}/</span><span class=\"meta\">{files} files · {size}</span></a>\n",
                    name = entry.name,
                    esc = html_escape(&entry.name),
                    files = entry.files,
                    size = size_label(entry.bytes),
                )
                .unwrap();
            }
            let summary = format!("{} files · {} logical", list.files, size_label(list.bytes));
            let body = format!(
                r#"<!DOCTYPE html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>sites</title>
<style>
{STYLE}
</style>
<main>
  <header>
    <span>FILES</span>
    <span>sites</span>
    <span><a href="/">docs</a></span>
  </header>
  <div class="summary">{summary}</div>
  <div class="list">
{rows}  </div>
</main>
"#,
            );
            (
                [
                    (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                    (header::CACHE_CONTROL, "no-cache"),
                    (header::VARY, "Accept, User-Agent"),
                ],
                body,
            )
                .into_response()
        }
    }
}

pub fn listing(
    headers: &HeaderMap,
    site: &str,
    rel: &str,
    list: &DirList,
    files_view: bool,
) -> Response {
    if wants_json(headers) {
        let entries = list
            .entries
            .iter()
            .map(|entry| ListingEntry {
                kind: match entry.kind {
                    EntryKind::Directory => ListingKind::Directory,
                    EntryKind::File => ListingKind::File,
                },
                name: entry.name.clone(),
                files: (entry.kind == EntryKind::Directory).then_some(entry.files),
                bytes: entry.bytes,
            })
            .collect();
        return json_response(ListingJson {
            path: display_path(site, rel),
            files: list.files,
            bytes: list.bytes,
            entries,
        });
    }
    let flavor = page::negotiate(headers);
    match flavor {
        page::Flavor::Plain | page::Flavor::Man => {
            let body = render_plain(site, rel, list);
            (
                [
                    (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                    (header::CACHE_CONTROL, "no-cache"),
                    (header::VARY, "Accept, User-Agent"),
                ],
                body,
            )
                .into_response()
        }
        page::Flavor::Html => {
            let body = render_html(site, rel, list, files_view);
            (
                [
                    (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                    (header::CACHE_CONTROL, "no-cache"),
                    (header::VARY, "Accept, User-Agent"),
                ],
                body,
            )
                .into_response()
        }
    }
}

fn render_sites_plain(list: &SiteList) -> String {
    let mut sizes: Vec<HumanSize> = list
        .entries
        .iter()
        .map(|entry| HumanSize::new(entry.bytes))
        .collect();
    sizes.push(HumanSize::new(list.bytes));
    let name_width = list
        .entries
        .iter()
        .map(|entry| entry.name.len() + 1)
        .max()
        .unwrap_or(0)
        .max(12);
    let count_width = list
        .entries
        .iter()
        .map(|entry| digits(entry.files))
        .chain(std::iter::once(digits(list.files)))
        .max()
        .unwrap()
        .max(2);
    let layout = ListingLayout {
        name: name_width,
        count: count_width,
        size: SizeLayout::from_sizes(&sizes),
    };
    let mut out = String::new();
    for entry in &list.entries {
        push_listing_row(
            &mut out,
            &format!("{}/", entry.name),
            Some(entry.files),
            entry.bytes,
            layout,
            "",
        );
    }
    push_listing_row(&mut out, "", Some(list.files), list.bytes, layout, " total");
    out
}

fn render_plain(site: &str, rel: &str, list: &DirList) -> String {
    let display = display_path(site, rel);
    let mut sizes: Vec<HumanSize> = list
        .entries
        .iter()
        .map(|entry| HumanSize::new(entry.bytes))
        .collect();
    sizes.push(HumanSize::new(list.bytes));
    let name_width = list
        .entries
        .iter()
        .map(|entry| entry.name.len() + usize::from(entry.kind == EntryKind::Directory))
        .chain(std::iter::once(display.len()))
        .max()
        .unwrap()
        .max(12);
    let count_width = list
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::Directory)
        .map(|entry| digits(entry.files))
        .chain(std::iter::once(digits(list.files)))
        .max()
        .unwrap()
        .max(2);
    let layout = ListingLayout {
        name: name_width,
        count: count_width,
        size: SizeLayout::from_sizes(&sizes),
    };
    let mut out = String::new();
    push_listing_row(&mut out, &display, Some(list.files), list.bytes, layout, "");
    if !rel.is_empty() {
        out.push_str("../\n");
    }
    for entry in &list.entries {
        let name = match entry.kind {
            EntryKind::Directory => format!("{}/", entry.name),
            EntryKind::File => entry.name.clone(),
        };
        let files = (entry.kind == EntryKind::Directory).then_some(entry.files);
        push_listing_row(&mut out, &name, files, entry.bytes, layout, "");
    }
    out
}

fn render_html(site: &str, rel: &str, list: &DirList, files_view: bool) -> String {
    let display = display_path(site, rel);
    let parent = parent_href(site, rel, files_view);
    let mut rows = String::new();
    if let Some(href) = parent {
        writeln!(
            rows,
            "    <a class=\"row\" href=\"{href}\"><span class=\"name\">..</span><span class=\"meta\">dir</span></a>\n"
        )
        .unwrap();
    }
    for entry in &list.entries {
        let href = entry_href(site, rel, &entry.name, entry.kind, files_view);
        let meta = match entry.kind {
            EntryKind::Directory => {
                format!("{} files · {}", entry.files, size_label(entry.bytes))
            }
            EntryKind::File => size_label(entry.bytes),
        };
        let name = match entry.kind {
            EntryKind::Directory => format!("{}/", html_escape(&entry.name)),
            EntryKind::File => html_escape(&entry.name),
        };
        writeln!(
            rows,
            "    <a class=\"row\" href=\"{href}\"><span class=\"name\">{name}</span><span class=\"meta\">{meta}</span></a>\n"
        )
        .unwrap();
    }
    let files_href = format!("/{site}/FILES/");
    let site_href = format!("/{site}/");
    let see_site = if files_view
        && list.entries.iter().any(|entry| {
            entry.kind == EntryKind::File
                && matches!(entry.name.as_str(), "index.html" | "index.htm")
        }) {
        format!(
            "<a class=\"see-site\" href=\"{}\">see site</a>",
            html_escape(&dir_href(site, rel, false))
        )
    } else {
        String::new()
    };
    format!(
        r#"<!DOCTYPE html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>
{style}
</style>
<main>
  <header>
    <span>FILES</span>
    <span>{title}</span>
    <span><a href="{site_href}">site</a> · <a href="{files_href}">files</a></span>
  </header>
  <div class="summary"><span>{files} files · {size} logical</span>{see_site}</div>
  <div class="list">
{rows}  </div>
</main>
"#,
        title = html_escape(&display),
        style = STYLE,
        rows = rows,
        site_href = site_href,
        files_href = files_href,
        files = list.files,
        size = size_label(list.bytes),
        see_site = see_site,
    )
}

fn display_path(site: &str, rel: &str) -> String {
    if rel.is_empty() {
        format!("{site}/")
    } else {
        format!("{site}/{rel}/")
    }
}

fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| {
            accept
                .split(',')
                .any(|part| part.trim().split(';').next() == Some("application/json"))
        })
}

fn json_response(value: ListingJson) -> Response {
    (
        [
            (header::CACHE_CONTROL, "no-cache"),
            (header::VARY, "Accept, User-Agent"),
        ],
        Json(value),
    )
        .into_response()
}

fn push_listing_row(
    out: &mut String,
    name: &str,
    files: Option<u64>,
    bytes: u64,
    layout: ListingLayout,
    suffix: &str,
) {
    let count = files.map_or_else(
        || " ".repeat(layout.count + 6),
        |files| format!("{files:>width$} files", width = layout.count),
    );
    writeln!(
        out,
        "{name:<width$} {count}   {}{suffix}\n",
        HumanSize::new(bytes).aligned(layout.size),
        width = layout.name,
    )
    .unwrap();
}

fn parent_href(site: &str, rel: &str, files_view: bool) -> Option<String> {
    if rel.is_empty() {
        return if files_view {
            Some("/FILES".to_string())
        } else {
            None
        };
    }
    let parent = match rel.rsplit_once('/') {
        Some((p, _)) => p,
        None => "",
    };
    Some(dir_href(site, parent, files_view))
}

fn dir_href(site: &str, rel: &str, files_view: bool) -> String {
    if files_view {
        if rel.is_empty() {
            format!("/{site}/FILES/")
        } else {
            format!("/{site}/FILES/{rel}/")
        }
    } else if rel.is_empty() {
        format!("/{site}/")
    } else {
        format!("/{site}/{rel}/")
    }
}

fn entry_href(site: &str, rel: &str, name: &str, kind: EntryKind, files_view: bool) -> String {
    let path = if rel.is_empty() {
        name.to_string()
    } else {
        format!("{rel}/{name}")
    };
    match kind {
        EntryKind::Directory => dir_href(site, &path, files_view),
        EntryKind::File => format!("/{site}/{path}"),
    }
}

fn size_label(n: u64) -> String {
    let size = HumanSize::new(n);
    if size.fraction.is_empty() {
        format!("{} {}", size.integer, size.unit)
    } else {
        format!("{}.{} {}", size.integer, size.fraction, size.unit)
    }
}

#[derive(Clone)]
struct HumanSize {
    integer: String,
    fraction: String,
    unit: &'static str,
}

impl HumanSize {
    #[allow(
        clippy::cast_precision_loss,
        reason = "human-readable sizes are intentionally approximate"
    )]
    fn new(bytes: u64) -> Self {
        let (value, unit) = if bytes < 1024 {
            (bytes as f64, "B")
        } else if bytes < 1024 * 1024 {
            (bytes as f64 / 1024.0, "KiB")
        } else if bytes < 1024 * 1024 * 1024 {
            (bytes as f64 / 1024.0 / 1024.0, "MiB")
        } else if bytes < 1024_u64.pow(4) {
            (bytes as f64 / 1024.0_f64.powi(3), "GiB")
        } else {
            (bytes as f64 / 1024.0_f64.powi(4), "TiB")
        };
        let precision = if unit == "B" || value >= 100.0 {
            0
        } else if value >= 10.0 {
            1
        } else {
            2
        };
        let text = format!("{value:.precision$}");
        let (integer, fraction) = text
            .split_once('.')
            .map_or((text.as_str(), ""), |(integer, fraction)| {
                (integer, fraction)
            });
        Self {
            integer: integer.to_string(),
            fraction: fraction.to_string(),
            unit,
        }
    }

    fn aligned(&self, layout: SizeLayout) -> String {
        let mut number = format!("{:>width$}", self.integer, width = layout.integer);
        if layout.fraction > 0 {
            if self.fraction.is_empty() {
                number.push_str(&" ".repeat(layout.fraction + 1));
            } else {
                number.push('.');
                number.push_str(&self.fraction);
                number.push_str(&" ".repeat(layout.fraction - self.fraction.len()));
            }
        }
        format!("{number} {:<width$}", self.unit, width = layout.unit)
    }
}

#[derive(Clone, Copy)]
struct SizeLayout {
    integer: usize,
    fraction: usize,
    unit: usize,
}

#[derive(Clone, Copy)]
struct ListingLayout {
    name: usize,
    count: usize,
    size: SizeLayout,
}

impl SizeLayout {
    fn from_sizes(sizes: &[HumanSize]) -> Self {
        Self {
            integer: sizes
                .iter()
                .map(|size| size.integer.len())
                .max()
                .unwrap_or(1),
            fraction: sizes
                .iter()
                .map(|size| size.fraction.len())
                .max()
                .unwrap_or(0),
            unit: sizes.iter().map(|size| size.unit.len()).max().unwrap_or(1),
        }
    }
}

const fn digits(n: u64) -> usize {
    if n == 0 { 1 } else { n.ilog10() as usize + 1 }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const STYLE: &str = r#":root {
    --bg: #1c1916;
    --paper: #11100e;
    --ink: #d9d0c4;
    --dim: #8a8176;
    --rule: #3d3833;
    --mark: #e85d04;
    --ok: #c4d39d;
  }
  * { box-sizing: border-box; }
  html, body { margin: 0; background: var(--bg); color: var(--ink); }
  body {
    font: 15px/1.45 ui-monospace, "Cascadia Code", "SF Mono", Menlo, Consolas, monospace;
    padding: 2.5rem 1.25rem 4rem;
  }
  main { max-width: 44rem; margin: 0 auto; }
  header {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    border-bottom: 1px solid var(--rule);
    padding-bottom: .6rem;
    margin-bottom: 1.2rem;
    color: var(--dim);
    font-size: 12px;
    letter-spacing: .12em;
    text-transform: uppercase;
  }
  header a { color: var(--dim); text-decoration: none; }
  header a:hover { color: var(--mark); }
  .summary {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: .75rem;
    margin: -.45rem .7rem .75rem;
    color: var(--dim);
    font-size: 12px;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .see-site {
    padding: .25rem .5rem;
    background: var(--mark);
    color: var(--paper);
    font-weight: 700;
    letter-spacing: .08em;
    text-decoration: none;
    text-transform: uppercase;
  }
  .see-site:hover { color: var(--paper); filter: brightness(1.15); }
  .list { display: flex; flex-direction: column; }
  a.row {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 1rem;
    padding: .45rem .7rem;
    color: var(--ink);
    text-decoration: none;
    border-left: 3px solid transparent;
  }
  a.row:hover { background: var(--paper); border-left-color: var(--mark); }
  .name { color: var(--ok); }
  .meta {
    color: var(--dim);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{DirEnt, SiteEnt};
    use axum::body::to_bytes;
    use axum::http::{HeaderValue, StatusCode};

    fn accept(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, HeaderValue::from_static(value));
        headers
    }

    async fn body_text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn plain_site_sizes_align_ones_places_and_units() {
        let list = SiteList {
            files: 11,
            bytes: 4_194_304,
            entries: vec![
                SiteEnt {
                    name: "hello".to_string(),
                    files: 8,
                    bytes: 3_774_464,
                },
                SiteEnt {
                    name: "notes".to_string(),
                    files: 3,
                    bytes: 419_840,
                },
            ],
        };
        assert_eq!(
            render_sites_plain(&list),
            concat!(
                "hello/        8 files     3.60 MiB\n",
                "notes/        3 files   410    KiB\n",
                "             11 files     4.00 MiB total\n",
            )
        );
    }

    #[test]
    fn plain_directory_sizes_align_files_and_directories() {
        let list = DirList {
            files: 8,
            bytes: 3_774_464,
            entries: vec![
                DirEnt {
                    kind: EntryKind::Directory,
                    name: "assets".to_string(),
                    files: 5,
                    bytes: 3_565_158,
                },
                DirEnt {
                    kind: EntryKind::Directory,
                    name: "css".to_string(),
                    files: 2,
                    bytes: 188_743,
                },
                DirEnt {
                    kind: EntryKind::File,
                    name: "index.html".to_string(),
                    files: 1,
                    bytes: 20_563,
                },
            ],
        };
        assert_eq!(
            render_plain("hello", "", &list),
            "hello/        8 files     3.60 MiB\n\
             assets/       5 files     3.40 MiB\n\
             css/          2 files   184    KiB\n\
             index.html               20.1  KiB\n"
        );
    }

    #[tokio::test]
    async fn files_responses_negotiate_json_and_html() {
        let list = SiteList {
            files: 1,
            bytes: 512,
            entries: vec![SiteEnt {
                name: "hello".to_string(),
                files: 1,
                bytes: 512,
            }],
        };
        let response = sites(&accept("application/json"), &list);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(
            body_text(response).await,
            r#"{"path":"/","files":1,"bytes":512,"entries":[{"kind":"site","name":"hello","files":1,"bytes":512}]}"#
        );

        let response = sites(&accept("text/html"), &list);
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_text(response).await;
        assert!(body.contains("1 files · 512 B"));
        assert!(body.contains(r#"href="/hello/FILES/""#));
        assert!(body.contains("font-variant-numeric: tabular-nums"));
    }

    #[test]
    fn json_accept_detection_is_explicit() {
        assert!(wants_json(&accept("application/json; charset=utf-8")));
        assert!(!wants_json(&accept("*/*")));
    }

    #[test]
    fn files_navigation_keeps_directory_links_in_files_view() {
        assert_eq!(parent_href("hello", "", true).as_deref(), Some("/FILES"));
        assert_eq!(
            parent_href("hello", "assets/css", true).as_deref(),
            Some("/hello/FILES/assets/")
        );
        assert_eq!(
            entry_href("hello", "assets", "css", EntryKind::Directory, true),
            "/hello/FILES/assets/css/"
        );
    }

    #[test]
    fn files_html_links_to_site_when_directory_has_an_index() {
        let list = DirList {
            files: 1,
            bytes: 512,
            entries: vec![DirEnt {
                kind: EntryKind::File,
                name: "index.html".to_string(),
                files: 1,
                bytes: 512,
            }],
        };
        let body = render_html("hello", "docs", &list, true);
        assert!(body.contains(r#"class="see-site" href="/hello/docs/""#));
        assert!(body.contains(">see site</a>"));

        let body = render_html("hello", "docs", &list, false);
        assert!(!body.contains(r#"class="see-site""#));
    }
}
