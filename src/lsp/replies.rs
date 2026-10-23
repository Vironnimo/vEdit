//! Convert server replies into bounded editor values and reference previews.
use super::*;

fn parse_range(value: &Value) -> Option<TextRange> {
    let range: TextRange = serde_json::from_value(value.clone()).ok()?;
    ((range.start.line, range.start.character) <= (range.end.line, range.end.character))
        .then_some(range)
}
fn parse_edit(value: &Value) -> Option<TextEdit> {
    Some(TextEdit {
        range: parse_range(value.get("range").or_else(|| value.get("replace"))?)?,
        new_text: value.get("newText")?.as_str()?.to_owned(),
    })
}

pub(super) fn parse_diagnostics(params: &Value) -> Option<(PathBuf, Option<i32>, Vec<Diagnostic>)> {
    let path = uri_to_path(params.get("uri")?.as_str()?)?;
    let version = params
        .get("version")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok());
    let diagnostics = params
        .get("diagnostics")?
        .as_array()?
        .iter()
        .take(2000)
        .filter_map(|value| {
            Some(Diagnostic {
                range: parse_range(value.get("range")?)?,
                severity: value
                    .get("severity")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .clamp(1, 4) as u8,
                message: value.get("message")?.as_str()?.to_owned(),
                source: value
                    .get("source")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect();
    Some((path, version, diagnostics))
}

pub(super) fn parse_completions(result: &Value) -> Vec<CompletionItem> {
    let Some(items) = result
        .as_array()
        .or_else(|| result.get("items").and_then(Value::as_array))
    else {
        return Vec::new();
    };
    let mut items = items
        .iter()
        .filter_map(|value| {
            let label = value.get("label")?.as_str()?.to_owned();
            let mut text_edit = value.get("textEdit").and_then(parse_edit);
            let mut insert_text = text_edit
                .as_ref()
                .map(|edit| edit.new_text.clone())
                .or_else(|| {
                    value
                        .get("insertText")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| label.clone());
            if value.get("insertTextFormat").and_then(Value::as_u64) == Some(2) {
                insert_text = plain_snippet(&insert_text);
                if let Some(edit) = &mut text_edit {
                    edit.new_text = plain_snippet(&edit.new_text);
                }
            }
            let additional_text_edits = value
                .get("additionalTextEdits")
                .and_then(Value::as_array)
                .map(|edits| edits.iter().take(100).filter_map(parse_edit).collect())
                .unwrap_or_default();
            let detail = value
                .get("detail")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let sort = value
                .get("sortText")
                .and_then(Value::as_str)
                .unwrap_or(&label)
                .to_owned();
            Some((
                sort,
                CompletionItem {
                    label,
                    detail,
                    insert_text,
                    text_edit,
                    additional_text_edits,
                },
            ))
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items.into_iter().take(200).map(|(_, item)| item).collect()
}

pub(super) fn parse_definition(result: &Value) -> Option<(PathBuf, Position)> {
    let value = result
        .as_array()
        .and_then(|items| items.first())
        .unwrap_or(result);
    let uri = value
        .get("uri")
        .or_else(|| value.get("targetUri"))?
        .as_str()?;
    let range = value
        .get("targetSelectionRange")
        .or_else(|| value.get("range"))
        .or_else(|| value.get("targetRange"))?;
    Some((uri_to_path(uri)?, parse_range(range)?.start))
}

/// MarkupContent, a MarkedString or an array of MarkedStrings. Language-tagged
/// strings become fenced code; a deprecated plain string is Markdown.
pub(super) fn parse_hover(result: &Value) -> Option<HoverContents> {
    fn marked(value: &Value) -> Option<String> {
        if let Some(text) = value.as_str() {
            return Some(text.to_owned());
        }
        let code = value.get("value")?.as_str()?;
        let language = value.get("language")?.as_str()?;
        Some(format!("```{language}\n{code}\n```"))
    }
    let contents = result.get("contents")?;
    let (markdown, text) = if let Some(kind) = contents.get("kind").and_then(Value::as_str) {
        (
            kind == "markdown",
            contents.get("value")?.as_str()?.to_owned(),
        )
    } else if let Some(items) = contents.as_array() {
        (
            true,
            items
                .iter()
                .filter_map(marked)
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"),
        )
    } else {
        (true, marked(contents)?)
    };
    let mut text = text.trim().to_owned();
    if text.is_empty() {
        return None;
    }
    if text.len() > HOVER_LIMIT {
        let mut end = HOVER_LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    Some(HoverContents { markdown, text })
}

/// Location[] or LocationLink[]; null means none. Duplicates are removed.
pub(super) fn parse_locations(result: &Value) -> (Vec<(PathBuf, TextRange)>, bool) {
    let mut locations = Vec::new();
    let mut truncated = false;
    for value in result.as_array().map(Vec::as_slice).unwrap_or_default() {
        let Some(uri) = value
            .get("uri")
            .or_else(|| value.get("targetUri"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(range) = value
            .get("range")
            .or_else(|| value.get("targetSelectionRange"))
            .and_then(parse_range)
        else {
            continue;
        };
        let Some(path) = uri_to_path(uri) else {
            continue;
        };
        if locations.contains(&(path.clone(), range)) {
            continue;
        }
        if locations.len() == REFERENCE_LIMIT {
            truncated = true;
            break;
        }
        locations.push((path, range));
    }
    (locations, truncated)
}

/// The previewed source, at most the editor's document limit.
pub(super) fn read_preview_source(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

pub(super) const PREVIEW_CHARS: usize = 160;

/// Attach one-line previews. `open` holds the text the server analyzed for
/// documents it has open; others come from `read`. Paths become workspace keys.
pub(super) fn build_references(
    locations: Vec<(PathBuf, TextRange)>,
    open: &HashMap<PathBuf, String>,
    read: impl Fn(&Path) -> Option<String>,
) -> Vec<Reference> {
    let mut references = vec![None; locations.len()];
    let mut files: HashMap<PathBuf, Vec<(usize, TextRange)>> = HashMap::new();
    for (index, (path, range)) in locations.into_iter().enumerate() {
        files.entry(path).or_default().push((index, range));
    }
    for (path, mut ranges) in files {
        let key = canonical_path(&path);
        let disk = if open.contains_key(&path) {
            None
        } else {
            read(&path)
        };
        let text = open.get(&path).or(disk.as_ref());
        let mut lines = text
            .into_iter()
            .flat_map(|text| text.split('\n'))
            .enumerate();
        let mut current = lines.next();
        // Traverse each source once, but keep the server's original result
        // order (including multiple references on the same line).
        ranges.sort_unstable_by_key(|(_, range)| range.start.line);
        for (index, range) in ranges {
            let target = range.start.line as usize;
            while current.is_some_and(|(line, _)| line < target) {
                current = lines.next();
            }
            let line_text = current
                .filter(|(line, _)| *line == target)
                .map(|(_, text)| text.strip_suffix('\r').unwrap_or(text));
            let (column, preview, highlight) = match line_text {
                Some(line) => line_preview(line, range),
                None => (range.start.character as usize, String::new(), 0..0),
            };
            references[index] = Some(Reference {
                path: key.clone(),
                position: range.start,
                line: range.start.line as usize + 1,
                column: column + 1,
                preview,
                highlight,
            });
        }
    }
    references.into_iter().flatten().collect()
}

/// (zero-based scalar column, trimmed preview, highlighted scalar range in it).
pub(super) fn line_preview(
    line: &str,
    range: TextRange,
) -> (usize, String, std::ops::Range<usize>) {
    let column_at = |character: u32| position_to_char(line, Position { line: 0, character });
    let chars: Vec<char> = line.chars().collect();
    let start = column_at(range.start.character);
    let end = if range.end.line == range.start.line {
        column_at(range.end.character).max(start)
    } else {
        chars.len()
    };
    let indent = chars.iter().take_while(|ch| ch.is_whitespace()).count();
    // Keep the match visible in long lines: start shortly before it.
    let mut first = indent.min(start);
    if start - first > PREVIEW_CHARS / 2 {
        first = start - PREVIEW_CHARS / 4;
    }
    let last = chars.len().min(first + PREVIEW_CHARS);
    let mut preview: String = chars[first..last].iter().collect();
    let mut offset = 0;
    if first > indent {
        preview.insert(0, '…');
        offset = 1;
    }
    let trimmed = preview.trim_end().chars().count();
    preview.truncate(preview.trim_end().len());
    if last < chars.len() {
        preview.push('…');
    }
    let highlight =
        (start - first + offset).min(trimmed)..(end.min(last) - first + offset).min(trimmed);
    (start, preview, highlight)
}

/// DocumentSymbol[] (hierarchical) or SymbolInformation[] / WorkspaceSymbol[].
/// Document symbols keep document order and their nesting; `with_paths` keeps
/// each symbol's file for workspace results.
pub(super) fn parse_symbols(result: &Value, with_paths: bool) -> Vec<Symbol> {
    fn nested(value: &Value, container: Option<&str>, depth: usize, symbols: &mut Vec<Symbol>) {
        if symbols.len() >= SYMBOL_LIMIT || depth > 64 {
            return;
        }
        let Some(name) = value.get("name").and_then(Value::as_str) else {
            return;
        };
        let Some(range) = value
            .get("selectionRange")
            .or_else(|| value.get("range"))
            .and_then(parse_range)
        else {
            return;
        };
        symbols.push(Symbol {
            name: name.to_owned(),
            detail: value
                .get("detail")
                .and_then(Value::as_str)
                .filter(|detail| !detail.trim().is_empty())
                .map(str::to_owned),
            kind: value.get("kind").and_then(Value::as_u64).unwrap_or(0) as u32,
            container: container.map(str::to_owned),
            path: None,
            position: range.start,
            depth,
        });
        for child in value
            .get("children")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            nested(child, Some(name), depth + 1, symbols);
        }
    }
    let items = result.as_array().map(Vec::as_slice).unwrap_or_default();
    let mut symbols = Vec::new();
    for value in items {
        if symbols.len() >= SYMBOL_LIMIT {
            break;
        }
        let Some(location) = value.get("location") else {
            nested(value, None, 0, &mut symbols);
            continue;
        };
        let Some(name) = value.get("name").and_then(Value::as_str) else {
            continue;
        };
        let path = location
            .get("uri")
            .and_then(Value::as_str)
            .and_then(uri_to_path);
        if with_paths && path.is_none() {
            continue;
        }
        symbols.push(Symbol {
            name: name.to_owned(),
            detail: None,
            kind: value.get("kind").and_then(Value::as_u64).unwrap_or(0) as u32,
            container: value
                .get("containerName")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_owned),
            path: path.filter(|_| with_paths),
            // A WorkspaceSymbol may omit the range until resolved.
            position: location
                .get("range")
                .and_then(parse_range)
                .map(|range| range.start)
                .unwrap_or_default(),
            depth: 0,
        });
    }
    if !with_paths
        && items
            .first()
            .is_some_and(|value| value.get("location").is_some())
    {
        symbols.sort_by_key(|symbol| (symbol.position.line, symbol.position.character));
    }
    symbols
}

/// A calm lowercase name for an LSP SymbolKind.
pub fn symbol_kind_label(kind: u32) -> &'static str {
    match kind {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "boolean",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enum member",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type parameter",
        _ => "symbol",
    }
}

pub(super) fn plain_snippet(snippet: &str) -> String {
    let mut output = String::new();
    let mut chars = snippet.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\'
            && chars
                .peek()
                .is_some_and(|next| matches!(next, '$' | '}' | '\\'))
        {
            output.push(chars.next().unwrap());
        } else if ch == '$' && chars.peek().is_some_and(char::is_ascii_digit) {
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
            }
        } else if ch == '$' && chars.peek() == Some(&'{') {
            chars.next();
            let mut placeholder = String::new();
            let mut depth = 1;
            while let Some(next) = chars.next() {
                if next == '\\'
                    && chars
                        .peek()
                        .is_some_and(|escaped| matches!(escaped, '$' | '}' | '\\'))
                {
                    // Preserve escapes for the recursive fallback conversion;
                    // an escaped brace must not end this placeholder.
                    placeholder.push(next);
                    placeholder.push(chars.next().unwrap());
                    continue;
                }
                if next == '{' {
                    depth += 1;
                }
                if next == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                placeholder.push(next);
            }
            if let Some((_, fallback)) = placeholder.split_once(':') {
                output.push_str(&plain_snippet(fallback));
            } else if let Some((_, choices)) = placeholder.split_once('|') {
                output.push_str(
                    choices
                        .split(',')
                        .next()
                        .unwrap_or("")
                        .trim_end_matches('|'),
                );
            }
        } else {
            output.push(ch);
        }
    }
    output
}
