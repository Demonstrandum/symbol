use std::sync::OnceLock;

use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;
use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::http_cache::{self, Representation};

const SOURCE: &str = static_asset!("docs.md");
const HOST_PLACEHOLDER: &str = concat!("{", "host", "}");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Html,
    Plain,
    Man,
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
    PAGE.get_or_init(|| parse(SOURCE).expect("static/docs.md"))
}

struct Templates {
    html: String,
    plain: String,
    man: String,
}

fn templates() -> &'static Templates {
    static T: OnceLock<Templates> = OnceLock::new();
    T.get_or_init(|| {
        let page = source_page();
        Templates {
            html: render_html(page),
            plain: render_plain_template(page, false),
            man: render_plain_template(page, true),
        }
    })
}

pub fn negotiate(headers: &HeaderMap) -> Flavor {
    accept_flavor(header_str(headers, header::ACCEPT)).unwrap_or_else(|| {
        let ua = header_str(headers, header::USER_AGENT);
        if looks_like_browser(ua) {
            Flavor::Html
        } else if looks_like_cli(ua) {
            Flavor::Man
        } else {
            Flavor::Plain
        }
    })
}

pub fn render(headers: &HeaderMap, host: &str, flavor: Flavor) -> Response {
    match flavor {
        Flavor::Html => html_response(headers, host),
        Flavor::Plain => plain_response(headers, host, false),
        Flavor::Man => plain_response(headers, host, true),
    }
}

fn html_response(headers: &HeaderMap, host: &str) -> Response {
    let mut representation = Representation::new(fill_html(host), "text/html; charset=utf-8");
    representation.vary = Some(HeaderValue::from_static("Accept, User-Agent"));
    representation.link = Some(HeaderValue::from_static(
        "</>; rel=\"alternate\"; type=\"text/plain\"",
    ));
    http_cache::respond(headers, representation)
}

fn plain_response(headers: &HeaderMap, host: &str, man: bool) -> Response {
    let body = if man {
        fill_man(host)
    } else {
        render_plain(host)
    };
    let mut representation = Representation::new(body, "text/plain; charset=utf-8");
    representation.vary = Some(HeaderValue::from_static("Accept, User-Agent"));
    representation.link = Some(HeaderValue::from_static(
        "</>; rel=\"alternate\"; type=\"text/html\"",
    ));
    http_cache::respond(headers, representation)
}

pub fn render_plain(host: &str) -> String {
    templates().plain.replace(HOST_PLACEHOLDER, host)
}

fn fill_man(host: &str) -> String {
    templates().man.replace(HOST_PLACEHOLDER, host)
}

fn fill_html(host: &str) -> String {
    templates()
        .html
        .replace(HOST_PLACEHOLDER, &html! { (host) }.into_string())
}

fn render_plain_template(page: &Page, tty: bool) -> String {
    let (name, description) = intro(&page.title, &page.lead);
    let mut out = String::new();
    out.push_str(&banner(MAN_MID, tty));
    out.push_str("\n\n");
    push_heading(&mut out, "NAME", tty);
    push_name_line(&mut out, &name, tty);
    if !description.is_empty() {
        out.push('\n');
        push_heading(&mut out, "DESCRIPTION", tty);
        for para in &description {
            out.push('\n');
            push_wrapped(&mut out, &inline_plain(para, tty), INDENT);
        }
    }
    for section in &page.sections {
        out.push('\n');
        push_heading(
            &mut out,
            &inline_plain(&section.heading, false).to_uppercase(),
            tty,
        );
        for block in &section.blocks {
            match block {
                Block::Prose(text) => {
                    out.push('\n');
                    push_wrapped(&mut out, &inline_plain(text, tty), INDENT);
                }
                Block::Example { caption, commands } => {
                    if !caption.is_empty() {
                        out.push('\n');
                        push_wrapped(&mut out, &inline_plain(caption, tty), INDENT);
                    }
                    out.push('\n');
                    for line in commands.lines() {
                        out.extend(std::iter::repeat_n(' ', EXDENT));
                        out.push_str(line);
                        out.push('\n');
                    }
                }
            }
        }
    }
    out.push('\n');
    out.push_str(&banner(MAN_MID, tty));
    out.push('\n');
    out
}

fn render_html(page: &Page) -> String {
    let (name, description) = intro(&page.title, &page.lead);
    html! {
        (DOCTYPE)
        meta charset="utf-8";
        meta name="viewport" content="width=device-width, initial-scale=1";
        title { (&page.title) }
        style {
            (PreEscaped(BASE_STYLE))
            (PreEscaped(STYLE))
        }
        main {
            header {
                span { (MAN) }
                span { (MAN_MID) }
                span { (MAN) }
            }
            h2 { "NAME" }
            p { (inline_html(&name)) }
            @if !description.is_empty() {
                h2 { "DESCRIPTION" }
                @for paragraph in &description {
                    p { (inline_html(paragraph)) }
                }
            }
            @for section in &page.sections {
                h2 { (&section.heading) }
                @for block in &section.blocks {
                    @match block {
                        Block::Prose(text) => {
                            p { (inline_html(text)) }
                        }
                        Block::Example { caption, commands } => {
                            .row {
                                @if !caption.is_empty() {
                                    p.cap { (inline_html(caption)) }
                                }
                                pre { (highlight_shell(commands)) }
                            }
                        }
                    }
                }
            }
            footer {
                span { (MAN) }
                span { "click a command to copy" }
                span { (MAN) }
            }
        }
        .toast #toast { "copied" }
        script { (PreEscaped(SCRIPT)) }
    }
    .into_string()
}

const MAN: &str = "SYMBOL(1)";
const MAN_MID: &str = "Tailnet static hosting";
const COLS: usize = 78;
const INDENT: usize = 7;
const EXDENT: usize = 14;

fn banner(mid: &str, tty: bool) -> String {
    let ends = MAN.len() * 2;
    let line = if COLS <= ends + mid.len() {
        format!("{MAN} {mid} {MAN}")
    } else {
        let gap = COLS - ends;
        let left_pad = gap.saturating_sub(mid.len()) / 2;
        let right_pad = gap - mid.len() - left_pad;
        let mut s = String::with_capacity(COLS);
        s.push_str(MAN);
        s.extend(std::iter::repeat_n(' ', left_pad));
        s.push_str(mid);
        s.extend(std::iter::repeat_n(' ', right_pad));
        s.push_str(MAN);
        s
    };
    if !tty {
        return line;
    }
    let inner = &line[MAN.len()..line.len() - MAN.len()];
    let mut out = overstrike(MAN);
    out.push_str(inner);
    out.push_str(&overstrike(MAN));
    out
}

fn overstrike(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for c in s.chars() {
        out.push(c);
        out.push('\u{8}');
        out.push(c);
    }
    out
}

fn vis(s: &str) -> usize {
    let mut n = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{8}' {
            continue;
        }
        n += 1;
        if chars.peek() == Some(&'\u{8}') {
            chars.next();
            chars.next();
        }
    }
    n
}

fn push_heading(out: &mut String, s: &str, tty: bool) {
    if tty {
        out.push_str(&overstrike(s));
    } else {
        out.push_str(s);
    }
    out.push('\n');
}

fn push_name_line(out: &mut String, name: &str, tty: bool) {
    let (title, rest) = name.split_once(" - ").unwrap_or((name, ""));
    let rest = inline_plain(rest, tty);
    out.extend(std::iter::repeat_n(' ', INDENT));
    if tty {
        out.push_str(&overstrike(title));
    } else {
        out.push_str(title);
    }
    if rest.is_empty() {
        out.push('\n');
        return;
    }
    out.push_str(" - ");
    let first_fill = COLS
        .saturating_sub(INDENT + title.chars().count() + 3)
        .max(1);
    let fill = COLS.saturating_sub(INDENT).max(1);
    let mut line = String::new();
    let mut first = true;
    for word in rest.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
            continue;
        }
        let limit = if first { first_fill } else { fill };
        if vis(&line) + 1 + vis(word) > limit {
            out.push_str(&line);
            out.push('\n');
            out.extend(std::iter::repeat_n(' ', INDENT));
            line.clear();
            line.push_str(word);
            first = false;
        } else {
            line.push(' ');
            line.push_str(word);
        }
    }
    out.push_str(&line);
    out.push('\n');
}

fn push_wrapped(out: &mut String, text: &str, indent: usize) {
    let fill = COLS.saturating_sub(indent).max(1);
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
            continue;
        }
        if vis(&line) + 1 + vis(word) > fill {
            out.extend(std::iter::repeat_n(' ', indent));
            out.push_str(&line);
            out.push('\n');
            line.clear();
        } else {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.extend(std::iter::repeat_n(' ', indent));
        out.push_str(&line);
        out.push('\n');
    }
}

fn flow(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn first_sentence(s: &str) -> (&str, &str) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'.' && bytes[i + 1].is_ascii_whitespace() {
            return (&s[..=i], s[i + 1..].trim_start());
        }
        i += 1;
    }
    (s, "")
}

fn intro(title: &str, lead: &str) -> (String, Vec<String>) {
    let (head, tail) = first_sentence(lead.trim());
    let name = format!("{} - {}", title, flow(head));
    let description = tail
        .split("\n\n")
        .map(flow)
        .filter(|p| !p.is_empty())
        .collect();
    (name, description)
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
    const fn class(self) -> Option<&'static str> {
        match self {
            Self::Cmd => Some("cmd"),
            Self::Flag => Some("flag"),
            Self::Str => Some("str"),
            Self::Cmt => Some("cmt"),
            Self::Url => Some("url"),
            Self::Punct => Some("punct"),
            Self::Text => None,
        }
    }
}

fn highlight_shell(src: &str) -> Markup {
    let mut out = Vec::new();
    let mut continued = false;
    for line in src.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        highlight_line(body, !continued, &mut out);
        if line.ends_with('\n') {
            out.push(html! { "\n" });
        }
        continued = body.trim_end().ends_with('\\');
    }
    html! {
        @for part in out {
            (part)
        }
    }
}

fn highlight_line(line: &str, mut expect_cmd: bool, out: &mut Vec<Markup>) {
    let mut rest = line;
    while !rest.is_empty() {
        if rest.starts_with(|ch: char| ch.is_whitespace()) {
            let n = rest
                .find(|ch: char| !ch.is_whitespace())
                .unwrap_or(rest.len());
            out.push(html! { (&rest[..n]) });
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

fn emit(out: &mut Vec<Markup>, kind: Hl, text: &str) {
    match kind.class() {
        Some(class) => out.push(html! { span class=(class) { (text) } }),
        None => out.push(html! { (text) }),
    }
}

fn take_link(s: &str) -> Option<(&str, &str, &str)> {
    let rest = s.strip_prefix('[')?;
    let close = rest.find(']')?;
    let label = &rest[..close];
    let rest = rest[close + 1..].strip_prefix('(')?;
    let end = rest.find(')')?;
    Some((label, &rest[..end], &rest[end + 1..]))
}

fn inline_plain(s: &str, tty: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        if let Some((label, href, after)) = take_link(&rest[i..]) {
            out.push_str(&rest[..i]);
            out.push_str(label);
            if label != href {
                out.push_str(" (");
                out.push_str(href);
                out.push(')');
            }
            rest = after;
        } else {
            out.push_str(&rest[..=i]);
            rest = &rest[i + 1..];
        }
    }
    out.push_str(rest);
    ticks(&out, tty)
}

fn ticks(s: &str, tty: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('`') {
        out.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        if let Some(end) = rest.find('`') {
            let code = &rest[..end];
            if tty && !code.contains("{host}") {
                out.push_str(&overstrike(code));
            } else {
                out.push_str(code);
            }
            rest = &rest[end + 1..];
        } else {
            out.push('`');
            break;
        }
    }
    out.push_str(rest);
    out
}

fn inline_html(s: &str) -> Markup {
    let mut out = Vec::new();
    let mut rest = s;
    loop {
        let tick = rest.find('`');
        let brack = rest.find('[');
        let next = match (tick, brack) {
            (None, None) => {
                out.push(html! { (rest) });
                return html! {
                    @for part in out {
                        (part)
                    }
                };
            }
            (Some(t), None) => t,
            (None, Some(b)) => b,
            (Some(t), Some(b)) => t.min(b),
        };
        out.push(html! { (&rest[..next]) });
        rest = &rest[next..];
        if rest.starts_with('`') {
            rest = &rest[1..];
            if let Some(end) = rest.find('`') {
                out.push(html! { code { (&rest[..end]) } });
                rest = &rest[end + 1..];
            } else {
                out.push(html! { "`" (rest) });
                return html! {
                    @for part in out {
                        (part)
                    }
                };
            }
        } else if let Some((label, href, after)) = take_link(rest) {
            out.push(html! { a href=(href) { (codes_html(label)) } });
            rest = after;
        } else {
            out.push(html! { "[" });
            rest = &rest[1..];
        }
    }
}

fn codes_html(s: &str) -> Markup {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find('`') {
        out.push(html! { (&rest[..start]) });
        rest = &rest[start + 1..];
        if let Some(end) = rest.find('`') {
            out.push(html! { code { (&rest[..end]) } });
            rest = &rest[end + 1..];
        } else {
            out.push(html! { "`" });
            break;
        }
    }
    out.push(html! { (rest) });
    html! {
        @for part in out {
            (part)
        }
    }
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> &str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

fn looks_like_browser(ua: &str) -> bool {
    let ua = ua.to_ascii_lowercase();
    ua.contains("mozilla") || ua.contains("browser")
}

fn looks_like_cli(ua: &str) -> bool {
    let ua = ua.to_ascii_lowercase();
    ua.contains("curl/") || ua.contains("wget") || ua.contains("httpie")
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

    let flush_pending =
        |pending: &mut Option<String>, lead: &mut String, sections: &mut Vec<Section>| {
            let Some(text) = pending.take() else {
                return;
            };
            if sections.is_empty() {
                if lead.is_empty() {
                    *lead = text;
                } else {
                    lead.push_str("\n\n");
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

const BASE_STYLE: &str = static_asset!("base.css");
const STYLE: &str = static_asset!("docs.css");
const SCRIPT: &str = static_asset!("docs.js");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_docs_support_conditional_requests() {
        let response = render(&HeaderMap::new(), "https://symbol.test", Flavor::Html);
        let etag = response.headers()[header::ETAG].clone();
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag);

        let response = render(&headers, "https://symbol.test", Flavor::Html);
        assert_eq!(response.status(), axum::http::StatusCode::NOT_MODIFIED);
    }

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
            other @ Block::Prose(_) => panic!("{other:?}"),
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
        assert!(looks_like_cli("curl/8.5.0"));
        assert!(!looks_like_cli(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/120.0.0.0"
        ));
        assert!(looks_like_browser(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/120.0.0.0"
        ));
    }

    #[test]
    fn host_is_filled_in_plain() {
        let text = render_plain("http://symbol");
        assert!(text.starts_with("SYMBOL(1)"));
        assert!(text.contains("NAME\n"));
        assert!(text.contains("symbol - tailnet hosting of static sites on http://symbol."));
        assert!(text.contains("DESCRIPTION\n"));
        assert!(text.contains("curl -T index.html http://symbol/hello"));
        assert!(text.contains("e.g. http://symbol/k7qm/"));
        assert!(!text.contains("[http://symbol/k7qm/]"));
        assert!(!text.contains("{host}"));
        assert!(!text.contains('`'));
        assert!(text.trim_end().ends_with("SYMBOL(1)"));
        assert!(!text.contains('\u{8}'));
    }

    #[test]
    fn man_overstrike_for_less() {
        let text = fill_man("http://symbol");
        assert!(text.contains('\u{8}'));
        assert!(text.contains("http://symbol/hello"));
        assert!(!text.contains("{host}"));
        assert!(text.contains(&overstrike("NAME")));
        assert!(text.contains(&overstrike("symbol")));
        assert!(!render_plain("http://symbol").contains('\u{8}'));
    }

    #[test]
    fn html_is_prebuilt_and_host_is_filled() {
        let html = fill_html("http://symbol");
        assert!(html.contains("class=\"url\">http://symbol/hello</span>"));
        assert!(html.contains("class=\"cmd\">curl</span>"));
        assert!(html.contains("<a href=\"http://symbol/k7qm/\">http://symbol/k7qm/</a>"));
        assert!(html.contains("<h2>NAME</h2>"));
        assert!(!html.contains("{host}"));
        assert!(templates().html.contains("{host}"));
        assert!(templates().html.contains("class=\"cmd\">curl</span>"));
    }

    #[test]
    fn markdown_links() {
        assert_eq!(
            inline_plain("see [a](b) and [c](c)", false),
            "see a (b) and c"
        );
        assert_eq!(
            inline_html("see [`x`]({host}/x)").into_string(),
            "see <a href=\"{host}/x\"><code>x</code></a>"
        );
        assert_eq!(inline_html("not [a link").into_string(), "not [a link");
    }

    #[test]
    fn highlights_shell_in_html() {
        let html = highlight_shell("curl -T index.html {host}/hello  # put\n'quoted' | sh \\\n")
            .into_string();
        assert!(html.contains("class=\"cmd\">curl</span>"));
        assert!(html.contains("class=\"flag\">-T</span>"));
        assert!(html.contains("class=\"url\">{host}/hello</span>"));
        assert!(html.contains("class=\"cmt\"># put</span>"));
        assert!(html.contains("class=\"str\">'quoted'</span>"));
        assert!(html.contains("class=\"cmd\">sh</span>"));
        assert!(html.contains("class=\"punct\">|</span>"));
        assert!(html.contains("class=\"punct\">\\</span>"));
        assert!(!html.contains("<script>"));
        let html = highlight_shell("echo '<b>'").into_string();
        assert!(html.contains("&lt;b&gt;"));
        let html = highlight_shell("ls \u{00a0}# nbsp").into_string();
        assert!(html.contains("class=\"cmt\"># nbsp</span>"));
    }
}
