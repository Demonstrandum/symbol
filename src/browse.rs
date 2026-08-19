use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};

use crate::page;
use crate::store::DirEnt;

pub fn sites(headers: &HeaderMap, names: &[String]) -> Response {
    let flavor = page::negotiate(headers);
    match flavor {
        page::Flavor::Plain => {
            let body = if names.is_empty() {
                String::new()
            } else {
                format!("{}\n", names.join("\n"))
            };
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
            for name in names {
                rows.push_str(&format!(
                    "    <a class=\"row\" href=\"/{name}/\"><span class=\"name\">{esc}/</span><span class=\"meta\">site</span></a>\n",
                    name = name,
                    esc = html_escape(name),
                ));
            }
            let body = format!(
                r#"<!DOCTYPE html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>sites</title>
<style>
{style}
</style>
<main>
  <header>
    <span>FILES</span>
    <span>sites</span>
    <span><a href="/">docs</a></span>
  </header>
  <div class="list">
{rows}  </div>
</main>
"#,
                style = STYLE,
                rows = rows,
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
    entries: &[DirEnt],
    files_view: bool,
) -> Response {
    let flavor = page::negotiate(headers);
    match flavor {
        page::Flavor::Plain => {
            let body = render_plain(site, rel, entries);
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
            let body = render_html(site, rel, entries, files_view);
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

fn render_plain(site: &str, rel: &str, entries: &[DirEnt]) -> String {
    let mut out = String::new();
    if rel.is_empty() {
        out.push_str(site);
        out.push_str("/\n");
    } else {
        out.push_str(site);
        out.push('/');
        out.push_str(rel);
        if !rel.ends_with('/') {
            out.push('/');
        }
        out.push('\n');
    }
    if !rel.is_empty() {
        out.push_str("../\n");
    }
    for ent in entries {
        if ent.is_dir {
            out.push_str(&ent.name);
            out.push_str("/\n");
        } else {
            out.push_str(&format!("{:<32} {}\n", ent.name, size_label(ent.size)));
        }
    }
    out
}

fn render_html(site: &str, rel: &str, entries: &[DirEnt], files_view: bool) -> String {
    let display = if rel.is_empty() {
        format!("{site}/")
    } else {
        format!("{site}/{rel}/")
    };
    let parent = parent_href(site, rel, files_view);
    let mut rows = String::new();
    if let Some(href) = parent {
        rows.push_str(&format!(
            "    <a class=\"row\" href=\"{href}\"><span class=\"name\">..</span><span class=\"meta\">dir</span></a>\n"
        ));
    }
    for ent in entries {
        let href = entry_href(site, rel, &ent.name, ent.is_dir, files_view);
        let meta = if ent.is_dir {
            "dir".to_string()
        } else {
            size_label(ent.size)
        };
        let name = if ent.is_dir {
            format!("{}/", html_escape(&ent.name))
        } else {
            html_escape(&ent.name)
        };
        rows.push_str(&format!(
            "    <a class=\"row\" href=\"{href}\"><span class=\"name\">{name}</span><span class=\"meta\">{meta}</span></a>\n"
        ));
    }
    let files_href = format!("/{site}/FILES/");
    let site_href = format!("/{site}/");
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
  <div class="list">
{rows}  </div>
</main>
"#,
        title = html_escape(&display),
        style = STYLE,
        rows = rows,
        site_href = site_href,
        files_href = files_href,
    )
}

fn parent_href(site: &str, rel: &str, files_view: bool) -> Option<String> {
    if rel.is_empty() {
        return if files_view {
            Some(format!("/{site}/"))
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

fn entry_href(site: &str, rel: &str, name: &str, is_dir: bool, files_view: bool) -> String {
    let path = if rel.is_empty() {
        name.to_string()
    } else {
        format!("{rel}/{name}")
    };
    if is_dir {
        dir_href(site, &path, files_view)
    } else {
        format!("/{site}/{path}")
    }
}

fn size_label(n: u64) -> String {
    if n < 1024 {
        format!("{n}B")
    } else if n < 1024 * 1024 {
        format!("{}K", (n + 512) / 1024)
    } else {
        format!("{:.1}M", n as f64 / 1024.0 / 1024.0)
    }
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
  .meta { color: var(--dim); font-size: 12px; }"#;
