use std::fmt::Write as _;

use pulldown_cmark::{Options, Parser, html};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Manual {
    Index,
    JavaScript,
    Python,
    Shell,
    Protocol,
}

impl Manual {
    pub const ALL: [Self; 5] = [
        Self::Index,
        Self::JavaScript,
        Self::Python,
        Self::Shell,
        Self::Protocol,
    ];

    pub const fn marker(self) -> &'static str {
        match self {
            Self::Index => "INDEX",
            Self::JavaScript => "JS",
            Self::Python => "PYTHON",
            Self::Shell => "SHELL",
            Self::Protocol => "PROTOCOL",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::JavaScript => "js",
            Self::Python => "python",
            Self::Shell => "shell",
            Self::Protocol => "protocol",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Index => "Symbol API",
            Self::JavaScript => "Symbol JavaScript and TypeScript API",
            Self::Python => "Symbol Python API",
            Self::Shell => "Symbol shell client",
            Self::Protocol => "Symbol HTTP protocol",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct CompiledManual {
    pub manual: Manual,
    pub markdown: String,
    pub html: String,
}

#[derive(Debug, Error)]
pub enum DocumentationError {
    #[error("API.md is missing marker `{0}`")]
    MissingMarker(String),
    #[error("API.md repeats marker `{0}`")]
    RepeatedMarker(String),
    #[error("API.md section `{0}` is empty")]
    EmptySection(&'static str),
}

pub fn compile(source: &str) -> Result<Vec<CompiledManual>, DocumentationError> {
    Manual::ALL
        .into_iter()
        .map(|manual| {
            let markdown = section(source, manual)?;
            let html = render_html(manual.title(), &markdown);
            Ok(CompiledManual {
                manual,
                markdown,
                html,
            })
        })
        .collect()
}

fn section(source: &str, manual: Manual) -> Result<String, DocumentationError> {
    let start = format!("<!-- API:{}:START -->", manual.marker());
    let end = format!("<!-- API:{}:END -->", manual.marker());
    let (_, after_start) = source
        .split_once(&start)
        .ok_or_else(|| DocumentationError::MissingMarker(start.clone()))?;
    if after_start.contains(&start) {
        return Err(DocumentationError::RepeatedMarker(start));
    }
    let (body, after_end) = after_start
        .split_once(&end)
        .ok_or_else(|| DocumentationError::MissingMarker(end.clone()))?;
    if after_end.contains(&end) {
        return Err(DocumentationError::RepeatedMarker(end));
    }
    let body = body.trim();
    if body.is_empty() {
        return Err(DocumentationError::EmptySection(manual.marker()));
    }
    Ok(format!("{body}\n"))
}

fn render_html(title: &str, markdown: &str) -> String {
    let mut rendered = String::new();
    html::push_html(&mut rendered, Parser::new_ext(markdown, Options::all()));
    let mut document = String::new();
    write!(
        document,
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title><style>\
         :root{{color-scheme:light dark}}body{{font:16px/1.55 system-ui,sans-serif;\
         max-width:76rem;margin:0 auto;padding:2rem}}pre{{overflow:auto;padding:1rem;\
         background:#8882}}code{{font-family:ui-monospace,monospace}}\
         a{{color:inherit}}table{{border-collapse:collapse}}td,th{{padding:.35rem .7rem;\
         border:1px solid #8888}}</style></head><body><main>{rendered}</main></body></html>"
    )
    .expect("writing to a String cannot fail");
    document
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_every_section_and_renders_real_markdown() {
        let source = Manual::ALL
            .into_iter()
            .map(|manual| {
                format!(
                    "<!-- API:{}:START -->\n# {}\n\n```sh\necho ok\n```\n\
                     <!-- API:{}:END -->\n",
                    manual.marker(),
                    manual.title(),
                    manual.marker()
                )
            })
            .collect::<String>();
        let compiled = compile(&source).unwrap();
        assert_eq!(compiled.len(), Manual::ALL.len());
        assert!(compiled[0].html.contains("<h1>"));
        assert!(
            compiled[0]
                .html
                .contains("<pre><code class=\"language-sh\">")
        );
    }

    #[test]
    fn rejects_missing_repeated_and_empty_sections() {
        let error = compile("").unwrap_err();
        assert!(matches!(error, DocumentationError::MissingMarker(_)));

        let repeated = "<!-- API:INDEX:START -->x<!-- API:INDEX:START -->\
                        <!-- API:INDEX:END -->";
        let error = compile(repeated).unwrap_err();
        assert!(matches!(error, DocumentationError::RepeatedMarker(_)));

        let empty = "<!-- API:INDEX:START --><!-- API:INDEX:END -->";
        let error = compile(empty).unwrap_err();
        assert!(matches!(error, DocumentationError::EmptySection("INDEX")));
    }
}
