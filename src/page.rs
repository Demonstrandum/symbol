use std::sync::OnceLock;

use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};

const SOURCE: &str = include_str!("../ops/docs.md");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Html,
    Plain,
}

#[derive(Debug)]
pub struct Page {
    pub title: String,
    pub lead: String,
    pub sections: Vec<Section>,
}

#[derive(Debug)]
pub struct Section {
    pub heading: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug)]
pub enum Block {
    Prose(String),
    Example { caption: String, commands: String },
}

fn source_page() -> &'static Page {
    static PAGE: OnceLock<Page> = OnceLock::new();
    PAGE.get_or_init(|| parse(SOURCE).expect("ops/docs.md"))
}

struct Templates {
    html: String,
    plain: String,
}

fn templates() -> &'static Templates {
    static T: OnceLock<Templates> = OnceLock::new();
    T.get_or_init(|| {
        let page = source_page();
        Templates {
            html: render_html(page),
            plain: render_plain_template(page),
        }
    })
}

pub fn negotiate(headers: &HeaderMap) -> Flavor {
    match accept_flavor(header_str(headers, header::ACCEPT)) {
        Some(flavor) => flavor,
        None => {
            if looks_like_browser(header_str(headers, header::USER_AGENT)) {
                Flavor::Html
            } else {
                Flavor::Plain
            }
        }
    }
}

pub fn render(host: &str, flavor: Flavor) -> Response {
    match flavor {
        Flavor::Html => html_response(host),
        Flavor::Plain => plain_response(host),
    }
}

fn html_response(host: &str) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            (header::VARY, HeaderValue::from_static("Accept, User-Agent")),
            (
                header::LINK,
                HeaderValue::from_static("</>; rel=\"alternate\"; type=\"text/plain\""),
            ),
        ],
        fill_html(host),
    )
        .into_response()
}

fn plain_response(host: &str) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            (header::VARY, HeaderValue::from_static("Accept, User-Agent")),
            (
                header::LINK,
                HeaderValue::from_static("</>; rel=\"alternate\"; type=\"text/html\""),
            ),
        ],
        render_plain(host),
    )
        .into_response()
}

pub fn render_plain(host: &str) -> String {
    templates().plain.replace("{host}", host)
}

fn fill_html(host: &str) -> String {
    templates().html.replace("{host}", &escape(host))
}

fn render_plain_template(page: &Page) -> String {
    let mut out = String::new();
    out.push_str(&page.title);
    out.push_str("\n\n");
    out.push_str(&page.lead);
    out.push('\n');
    for section in &page.sections {
        out.push('\n');
        out.push_str(&section.heading.to_uppercase());
        out.push('\n');
        for block in &section.blocks {
            match block {
                Block::Prose(text) => {
                    out.push('\n');
                    out.push_str(text);
                    out.push('\n');
                }
                Block::Example { caption, commands } => {
                    if !caption.is_empty() {
                        out.push('\n');
                        out.push_str(caption);
                        out.push('\n');
                    }
                    out.push('\n');
                    for line in commands.lines() {
                        out.push_str("    ");
                        out.push_str(line);
                        out.push('\n');
                    }
                }
            }
        }
    }
    out.push('\n');
    out
}

fn render_html(page: &Page) -> String {
    let mut body = String::new();
    body.push_str("<!DOCTYPE html>\n<meta charset=\"utf-8\">\n");
    body.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    body.push_str("<title>");
    body.push_str(&escape(&page.title));
    body.push_str("</title>\n<style>\n");
    body.push_str(STYLE);
    body.push_str("\n</style>\n<main>\n  <header>\n    <span>SYMBOL(1)</span>\n    <span>Tailnet static hosting</span>\n    <span>SYMBOL(1)</span>\n  </header>\n  <h1>");
    body.push_str(&heading_html(&page.title));
    body.push_str("</h1>\n  <p class=\"lead\">");
    body.push_str(&inline_html(&page.lead));
    body.push_str("</p>\n");
    for section in &page.sections {
        body.push_str("  <h2>");
        body.push_str(&escape(&section.heading));
        body.push_str("</h2>\n");
        for block in &section.blocks {
            match block {
                Block::Prose(text) => {
                    body.push_str("  <p>");
                    body.push_str(&inline_html(text));
                    body.push_str("</p>\n");
                }
                Block::Example { caption, commands } => {
                    body.push_str("  <div class=\"row\">\n");
                    if !caption.is_empty() {
                        body.push_str("    <p class=\"cap\">");
                        body.push_str(&inline_html(caption));
                        body.push_str("</p>\n");
                    }
                    body.push_str("    <pre>");
                    body.push_str(&highlight_shell(commands));
                    body.push_str("</pre>\n  </div>\n");
                }
            }
        }
    }
    body.push_str("  <footer>click a command to copy · {host}</footer>\n</main>\n<div class=\"toast\" id=\"toast\">copied</div>\n");
    body.push_str(SCRIPT);
    body
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(Clone, Copy)]
enum Hl {
    Cmd,
    Flag,
    Str,
    Cmt,
    Url,
    Punct,
    Text,
}

impl Hl {
    fn class(self) -> Option<&'static str> {
        match self {
            Hl::Cmd => Some("cmd"),
            Hl::Flag => Some("flag"),
            Hl::Str => Some("str"),
            Hl::Cmt => Some("cmt"),
            Hl::Url => Some("url"),
            Hl::Punct => Some("punct"),
            Hl::Text => None,
        }
    }
}

fn highlight_shell(src: &str) -> String {
    let mut out = String::with_capacity(src.len() * 2);
    let mut continued = false;
    for line in src.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        highlight_line(body, !continued, &mut out);
        if line.ends_with('\n') {
            out.push('\n');
        }
        continued = body.trim_end().ends_with('\\');
    }
    out
}

fn highlight_line(line: &str, mut expect_cmd: bool, out: &mut String) {
    let mut rest = line;
    while !rest.is_empty() {
        if rest.starts_with(|ch: char| ch.is_whitespace()) {
            let n = rest.find(|ch: char| !ch.is_whitespace()).unwrap_or(rest.len());
            out.push_str(&rest[..n]);
            rest = &rest[n..];
            continue;
        }
        let c = rest.as_bytes()[0];
        if c == b'#' {
            emit(out, Hl::Cmt, rest);
            return;
        }
        if c == b'\\' {
            emit(out, Hl::Punct, "\\");
            rest = &rest[1..];
            continue;
        }
        if c == b'|' {
            emit(out, Hl::Punct, "|");
            rest = &rest[1..];
            expect_cmd = true;
            continue;
        }
        if c == b'\'' || c == b'"' {
            let (tok, next) = take_string(rest);
            emit(out, Hl::Str, tok);
            rest = next;
            expect_cmd = false;
            continue;
        }
        let n = rest
            .find(|ch: char| ch.is_whitespace() || ch == '|')
            .unwrap_or(rest.len())
            .max(1);
        let tok = &rest[..n];
        rest = &rest[n..];
        emit(out, classify(tok, expect_cmd), tok);
        expect_cmd = false;
    }
}

fn take_string(s: &str) -> (&str, &str) {
    let quote = s.as_bytes()[0];
    let mut i = 1;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == quote {
            return (&s[..=i], &s[i + 1..]);
        }
        i += 1;
    }
    (s, "")
}

fn classify(tok: &str, expect_cmd: bool) -> Hl {
    if expect_cmd && matches!(tok, "curl" | "symbol" | "tar" | "sh") {
        return Hl::Cmd;
    }
    if tok.starts_with('-') {
        return Hl::Flag;
    }
    if tok.contains("://") || tok.contains("{host}") {
        return Hl::Url;
    }
    Hl::Text
}

fn emit(out: &mut String, kind: Hl, text: &str) {
    match kind.class() {
        Some(class) => {
            out.push_str("<span class=\"");
            out.push_str(class);
            out.push_str("\">");
            out.push_str(&escape(text));
            out.push_str("</span>");
        }
        None => out.push_str(&escape(text)),
    }
}

fn inline_html(s: &str) -> String {
    let escaped = escape(s);
    let mut out = String::with_capacity(escaped.len());
    let mut rest = escaped.as_str();
    while let Some(start) = rest.find('`') {
        out.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        match rest.find('`') {
            Some(end) => {
                out.push_str("<code>");
                out.push_str(&rest[..end]);
                out.push_str("</code>");
                rest = &rest[end + 1..];
            }
            None => {
                out.push('`');
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn heading_html(title: &str) -> String {
    let html = inline_html(title);
    if let Some(rest) = html.strip_prefix("symbol") {
        if rest.starts_with(":") {
            return format!("<span>symbol</span>{rest}");
        }
    }
    html
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> &str {
    headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("")
}

fn looks_like_browser(ua: &str) -> bool {
    let ua = ua.to_ascii_lowercase();
    ua.contains("mozilla") || ua.contains("browser")
}

fn accept_flavor(accept: &str) -> Option<Flavor> {
    if accept.is_empty() {
        return None;
    }
    let mut html_q: Option<f32> = None;
    let mut plain_q: Option<f32> = None;
    for part in accept.split(',') {
        let mut bits = part.split(';');
        let media = bits.next()?.trim();
        let mut q = 1.0f32;
        for param in bits {
            if let Some(v) = param.trim().strip_prefix("q=") {
                q = v.parse().unwrap_or(0.0);
            }
        }
        match media {
            "text/html" | "application/xhtml+xml" => html_q = Some(html_q.unwrap_or(0.0).max(q)),
            "text/plain" | "text/markdown" | "text/x-markdown" => {
                plain_q = Some(plain_q.unwrap_or(0.0).max(q));
            }
            _ => {}
        }
    }
    match (html_q, plain_q) {
        (Some(h), Some(p)) if p > h => Some(Flavor::Plain),
        (Some(_), _) => Some(Flavor::Html),
        (None, Some(_)) => Some(Flavor::Plain),
        (None, None) => None,
    }
}

fn parse(src: &str) -> Result<Page, String> {
    let mut title = String::new();
    let mut lead = String::new();
    let mut sections: Vec<Section> = Vec::new();
    let mut pending: Option<String> = None;
    let mut prev_blank = false;
    let mut in_fence = false;
    let mut fence_buf = String::new();

    let flush_pending = |pending: &mut Option<String>,
                         lead: &mut String,
                         sections: &mut Vec<Section>| {
        let Some(text) = pending.take() else {
            return;
        };
        if sections.is_empty() {
            if lead.is_empty() {
                *lead = text;
            } else {
                lead.push('\n');
                lead.push_str(&text);
            }
        } else if let Some(section) = sections.last_mut() {
            section.blocks.push(Block::Prose(text));
        }
    };

    for line in src.lines() {
        if in_fence {
            if line.starts_with("```") {
                in_fence = false;
                let commands = fence_buf.trim_end_matches('\n').to_string();
                fence_buf.clear();
                let caption = pending.take().unwrap_or_default();
                let section = sections
                    .last_mut()
                    .ok_or_else(|| "command block before any section".to_string())?;
                section.blocks.push(Block::Example { caption, commands });
                prev_blank = false;
            } else {
                fence_buf.push_str(line);
                fence_buf.push('\n');
            }
            continue;
        }
        if line.starts_with("```") {
            in_fence = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            flush_pending(&mut pending, &mut lead, &mut sections);
            title = rest.to_string();
            prev_blank = false;
            continue;
        }
        if let Some(rest) = line.strip_prefix("## ") {
            flush_pending(&mut pending, &mut lead, &mut sections);
            sections.push(Section {
                heading: rest.to_string(),
                blocks: Vec::new(),
            });
            prev_blank = false;
            continue;
        }
        if line.trim().is_empty() {
            prev_blank = true;
            continue;
        }
        if prev_blank {
            flush_pending(&mut pending, &mut lead, &mut sections);
        }
        match pending.as_mut() {
            Some(buf) => {
                buf.push('\n');
                buf.push_str(line);
            }
            None => pending = Some(line.to_string()),
        }
        prev_blank = false;
    }
    if in_fence {
        return Err("unclosed command block".into());
    }
    flush_pending(&mut pending, &mut lead, &mut sections);
    if title.is_empty() {
        return Err("missing title".into());
    }
    Ok(Page {
        title,
        lead,
        sections,
    })
}

const STYLE: &str = r#":root {
    --bg: #1c1916;
    --paper: #11100e;
    --ink: #d9d0c4;
    --dim: #8a8176;
    --rule: #3d3833;
    --mark: #e85d04;
    --ok: #c4d39d;
    --str: #e6c07b;
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
    margin-bottom: 1.6rem;
    color: var(--dim);
    font-size: 12px;
    letter-spacing: .12em;
    text-transform: uppercase;
  }
  h1 {
    font-size: 1.15rem;
    font-weight: 600;
    letter-spacing: .08em;
    margin: 0 0 1.1rem;
  }
  h1 span { color: var(--mark); }
  p { color: var(--ink); margin: 0 0 1rem; max-width: 40rem; }
  p.lead { color: var(--dim); }
  h2 {
    margin: 1.8rem 0 .7rem;
    font-size: 11px;
    letter-spacing: .16em;
    text-transform: uppercase;
    color: var(--mark);
    font-weight: 600;
  }
  pre {
    background: var(--paper);
    border-left: 3px solid var(--mark);
    padding: .85rem 1rem;
    overflow: auto;
    margin: 0 0 .7rem;
    color: var(--ok);
    cursor: pointer;
    white-space: pre-wrap;
  }
  pre:hover { outline: 1px solid var(--rule); }
  pre .cmd { color: var(--mark); }
  pre .flag { color: var(--ink); }
  pre .str { color: var(--str); }
  pre .cmt, pre .punct { color: var(--dim); }
  pre .url { color: var(--ok); text-decoration: underline; text-underline-offset: .15em; }
  .row { margin-bottom: 1.1rem; }
  .cap {
    color: var(--dim);
    font-size: 12px;
    margin: 0 0 .35rem;
  }
  code { font: inherit; }
  a { color: var(--ink); }
  a:hover { color: var(--mark); }
  footer {
    margin-top: 2.4rem;
    border-top: 1px solid var(--rule);
    padding-top: .8rem;
    color: var(--dim);
    font-size: 12px;
  }
  .toast {
    position: fixed;
    right: 1rem;
    bottom: 1rem;
    background: var(--mark);
    color: #1c1916;
    padding: .35rem .6rem;
    font-size: 12px;
    letter-spacing: .08em;
    text-transform: uppercase;
    opacity: 0;
    transition: opacity .15s;
    pointer-events: none;
  }
  .toast.on { opacity: 1; }"#;

const SCRIPT: &str = r#"<script>
  const toast = document.getElementById("toast");
  document.querySelectorAll("pre").forEach((el) => {
    el.addEventListener("click", async () => {
      await navigator.clipboard.writeText(el.innerText);
      toast.classList.add("on");
      setTimeout(() => toast.classList.remove("on"), 700);
    });
  });
</script>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_docs() {
        let page = parse(SOURCE).unwrap();
        assert!(page.title.contains("symbol"));
        assert!(!page.lead.is_empty());
        assert!(page.sections.iter().any(|s| s.heading == "curl"));
        assert!(page.sections.iter().any(|s| {
            s.blocks.iter().any(|b| matches!(b, Block::Example { commands, .. } if commands.contains("{host}/hello")))
        }));
    }

    #[test]
    fn caption_binds_to_following_commands() {
        let src = "# t\n\nlead\n\n## s\n\nhello there\n\n```\ncmd\n```\n";
        let page = parse(src).unwrap();
        match &page.sections[0].blocks[0] {
            Block::Example { caption, commands } => {
                assert_eq!(caption, "hello there");
                assert_eq!(commands, "cmd");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn accept_plain_beats_html_when_q_higher() {
        assert_eq!(
            accept_flavor("text/plain, text/html;q=0.1"),
            Some(Flavor::Plain)
        );
        assert_eq!(
            accept_flavor("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
            Some(Flavor::Html)
        );
        assert_eq!(accept_flavor("*/*"), None);
        assert_eq!(accept_flavor("text/plain"), Some(Flavor::Plain));
    }

    #[test]
    fn curl_is_not_a_browser() {
        assert!(!looks_like_browser("curl/8.5.0"));
        assert!(looks_like_browser(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/120.0.0.0"
        ));
    }

    #[test]
    fn host_is_filled_in_plain() {
        let text = render_plain("http://symbol");
        assert!(text.contains("curl -T index.html http://symbol/hello"));
        assert!(!text.contains("{host}"));
    }

    #[test]
    fn html_is_prebuilt_and_host_is_filled() {
        let html = fill_html("http://symbol");
        assert!(html.contains("class=\"url\">http://symbol/hello</span>"));
        assert!(html.contains("class=\"cmd\">curl</span>"));
        assert!(!html.contains("{host}"));
        assert!(templates().html.contains("{host}"));
        assert!(templates().html.contains("class=\"cmd\">curl</span>"));
    }

    #[test]
    fn highlights_shell_in_html() {
        let html = highlight_shell(
            "curl -T index.html {host}/hello  # put\n'quoted' | sh \\\n",
        );
        assert!(html.contains("class=\"cmd\">curl</span>"));
        assert!(html.contains("class=\"flag\">-T</span>"));
        assert!(html.contains("class=\"url\">{host}/hello</span>"));
        assert!(html.contains("class=\"cmt\"># put</span>"));
        assert!(html.contains("class=\"str\">'quoted'</span>"));
        assert!(html.contains("class=\"cmd\">sh</span>"));
        assert!(html.contains("class=\"punct\">|</span>"));
        assert!(html.contains("class=\"punct\">\\</span>"));
        assert!(!html.contains("<script>"));
        let html = highlight_shell("echo '<b>'");
        assert!(html.contains("&lt;b&gt;"));
        let html = highlight_shell("ls \u{00a0}# nbsp");
        assert!(html.contains("class=\"cmt\"># nbsp</span>"));
    }
}
