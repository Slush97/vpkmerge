use super::{replace_resource_block, resource_block};
use anyhow::{bail, Context, Result};
use morphic::kv3::Value;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::io::BufRead;

pub(super) fn decompile_panorama_layout_xml(resource: &[u8]) -> Result<String> {
    let laco = resource_block(resource, *b"LaCo")?;
    let root = morphic::kv3::decode(laco).context("decoding LaCo KV3")?;
    print_panorama_layout_xml(&root)
}

pub(super) fn rebuild_panorama_layout_xml_resource(raw: &[u8], xml: &[u8]) -> Result<Vec<u8>> {
    let old_laco = resource_block(raw, *b"LaCo")?;
    let format = morphic::kv3::Format::from_payload(old_laco).context("reading LaCo KV3 format")?;

    // Faithfulness gate. Our XML<->LaCo codec is canonicalizing, not byte-exact:
    // it models the node types real Deadlock layouts use but does NOT yet
    // reproduce every compiled detail (it normalizes single-child `child`
    // containers to `vecChildren`, flattens reference-typed attribute values to
    // plain values, and omits the `sourceLineColumn` source map). Rather than
    // silently ship a layout whose structure we altered, we only rebuild a file
    // we can prove we reproduce: decompile the ORIGINAL, recompile it, and
    // require structural equality (ignoring the debug-only `sourceLineColumn`).
    // If that fails, bail so the caller's --allow-stale-raw path keeps the
    // original compiled bytes untouched. Verified against shipped HUD layouts:
    // simple ones pass; ones using reference attributes / single-child
    // containers correctly fall back instead of being silently rewritten.
    let original = morphic::kv3::decode(old_laco).context("decoding original LaCo KV3")?;
    if !layout_codec_reproduces(&original)? {
        bail!(
            "vpkmerge cannot losslessly rebuild this Panorama layout: its compiled form uses \
             constructs the XML codec does not yet round-trip (reference-typed attributes or \
             single-child containers). Re-pack with --allow-stale-raw to keep the original \
             compiled layout."
        );
    }

    let layout = compile_panorama_layout_xml(xml)?;
    let new_laco = morphic::kv3::encode(&layout, &format);
    replace_resource_block(raw, *b"LaCo", &new_laco)
}

/// Whether decompiling `original` and recompiling the result reproduces the same
/// layout AST, ignoring the debug-only `sourceLineColumn` source map (which the
/// engine does not render from and which XML cannot carry). This is the proof
/// that our codec fully models a given file's structure before we trust it to
/// apply an edit.
fn layout_codec_reproduces(original: &Value) -> Result<bool> {
    let xml = print_panorama_layout_xml(original)?;
    let recompiled = compile_panorama_layout_xml(xml.as_bytes())?;
    Ok(strip_kv3_key(original, "sourceLineColumn")
        == strip_kv3_key(&recompiled, "sourceLineColumn"))
}

/// Clone `value` with every object entry named `drop` removed, recursively.
fn strip_kv3_key(value: &Value, drop: &str) -> Value {
    match value {
        Value::Object(pairs) => Value::Object(
            pairs
                .iter()
                .filter(|(key, _)| key != drop)
                .map(|(key, child)| (key.clone(), strip_kv3_key(child, drop)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|child| strip_kv3_key(child, drop))
                .collect(),
        ),
        other => other.clone(),
    }
}

pub(super) fn compile_panorama_layout_xml(xml: &[u8]) -> Result<Value> {
    let mut reader = Reader::from_reader(std::io::Cursor::new(xml));
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(start) => {
                let root = parse_layout_element(&mut reader, &start)?;
                return Ok(Value::Object(vec![(
                    "m_AST".to_owned(),
                    Value::Object(vec![("m_pRoot".to_owned(), root)]),
                )]));
            }
            Event::Empty(start) => {
                let root = build_layout_node(&reader, &start, Vec::new())?;
                return Ok(Value::Object(vec![(
                    "m_AST".to_owned(),
                    Value::Object(vec![("m_pRoot".to_owned(), root)]),
                )]));
            }
            Event::Decl(_) | Event::Comment(_) | Event::Text(_) | Event::CData(_) => {}
            Event::Eof => bail!("layout XML has no root element"),
            event => bail!("unsupported XML event before root: {event:?}"),
        }
        buf.clear();
    }
}

fn parse_layout_element<R: BufRead>(
    reader: &mut Reader<R>,
    start: &BytesStart<'_>,
) -> Result<Value> {
    let tag = xml_name(start.name().as_ref())?;
    if tag == "script" {
        return parse_script_body(reader);
    }

    let mut children = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(child) => children.push(parse_layout_element(reader, &child)?),
            Event::Empty(child) => children.push(build_layout_node(reader, &child, Vec::new())?),
            Event::End(end) => {
                let end_tag = xml_name(end.name().as_ref())?;
                if end_tag != tag {
                    bail!("mismatched XML end tag </{end_tag}> for <{tag}>");
                }
                break;
            }
            Event::Text(text) => {
                if !text.decode()?.trim().is_empty() {
                    bail!("unexpected text inside <{tag}>");
                }
            }
            Event::CData(text) => {
                if !text.decode()?.trim().is_empty() {
                    bail!("unexpected CDATA inside <{tag}>");
                }
            }
            Event::Comment(_) | Event::Decl(_) => {}
            Event::Eof => bail!("unexpected EOF inside <{tag}>"),
            event => bail!("unsupported XML event inside <{tag}>: {event:?}"),
        }
        buf.clear();
    }

    build_layout_node(reader, start, children)
}

fn parse_script_body<R: BufRead>(reader: &mut Reader<R>) -> Result<Value> {
    let mut content = String::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Text(text) => content.push_str(&text.decode()?),
            Event::CData(text) => content.push_str(&text.decode()?),
            Event::End(end) => {
                let end_tag = xml_name(end.name().as_ref())?;
                if end_tag != "script" {
                    bail!("mismatched XML end tag </{end_tag}> for <script>");
                }
                break;
            }
            Event::Comment(_) => {}
            Event::Eof => bail!("unexpected EOF inside <script>"),
            event => bail!("unsupported XML event inside <script>: {event:?}"),
        }
        buf.clear();
    }
    Ok(Value::Object(vec![
        ("eType".to_owned(), Value::String("SCRIPT_BODY".to_owned())),
        ("name".to_owned(), Value::String(content)),
    ]))
}

fn build_layout_node<R: BufRead>(
    reader: &Reader<R>,
    start: &BytesStart<'_>,
    children: Vec<Value>,
) -> Result<Value> {
    let tag = xml_name(start.name().as_ref())?;
    match tag.as_str() {
        "root" => Ok(layout_container(
            "ROOT",
            None,
            attributes(reader, start)?,
            children,
        )),
        "styles" => Ok(layout_container(
            "STYLES",
            None,
            attributes(reader, start)?,
            children,
        )),
        "scripts" => Ok(layout_container(
            "SCRIPTS",
            None,
            attributes(reader, start)?,
            children,
        )),
        "snippets" => Ok(layout_container(
            "SNIPPETS",
            None,
            attributes(reader, start)?,
            children,
        )),
        "snippet" => {
            let mut attrs = attributes(reader, start)?;
            let name = take_required_attr(&mut attrs, "name")?;
            Ok(layout_container("SNIPPET", Some(name), attrs, children))
        }
        "include" => {
            let mut attrs = attributes(reader, start)?;
            let src = take_required_attr(&mut attrs, "src")?;
            if !attrs.is_empty() {
                bail!("<include> has unsupported attributes");
            }
            if !children.is_empty() {
                bail!("<include> cannot have children");
            }
            Ok(Value::Object(vec![
                ("eType".to_owned(), Value::String("INCLUDE".to_owned())),
                ("child".to_owned(), layout_reference_value_node(&src)?),
            ]))
        }
        "script" => bail!("internal parser error: script should be handled separately"),
        panel_name => Ok(layout_container(
            "PANEL",
            Some(panel_name.to_owned()),
            attributes(reader, start)?,
            children,
        )),
    }
}

fn layout_container(
    node_type: &str,
    name: Option<String>,
    attrs: Vec<(String, String)>,
    children: Vec<Value>,
) -> Value {
    let mut pairs = vec![("eType".to_owned(), Value::String(node_type.to_owned()))];
    if let Some(name) = name {
        pairs.push(("name".to_owned(), Value::String(name)));
    }
    let mut sub_nodes = attrs
        .into_iter()
        .map(|(key, value)| layout_attribute_node(&key, &value))
        .collect::<Vec<_>>();
    sub_nodes.extend(children);
    if !sub_nodes.is_empty() {
        pairs.push(("vecChildren".to_owned(), Value::Array(sub_nodes)));
    }
    Value::Object(pairs)
}

fn layout_attribute_node(name: &str, value: &str) -> Value {
    Value::Object(vec![
        (
            "eType".to_owned(),
            Value::String("PANEL_ATTRIBUTE".to_owned()),
        ),
        ("name".to_owned(), Value::String(name.to_owned())),
        (
            "child".to_owned(),
            Value::Object(vec![
                (
                    "eType".to_owned(),
                    Value::String("PANEL_ATTRIBUTE_VALUE".to_owned()),
                ),
                ("name".to_owned(), Value::String(value.to_owned())),
            ]),
        ),
    ])
}

fn layout_reference_value_node(src: &str) -> Result<Value> {
    let (node_type, name) = if let Some(path) = src.strip_prefix("s2r://") {
        ("REFERENCE_COMPILED", path)
    } else if let Some(path) = src.strip_prefix("file://") {
        ("REFERENCE_PASSTHROUGH", path)
    } else {
        bail!("unsupported Panorama reference {src:?}");
    };
    Ok(Value::Object(vec![
        ("eType".to_owned(), Value::String(node_type.to_owned())),
        ("name".to_owned(), Value::String(name.to_owned())),
    ]))
}

fn attributes<R: BufRead>(
    reader: &Reader<R>,
    start: &BytesStart<'_>,
) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for attr in start.attributes() {
        let attr = attr?;
        let key = xml_name(attr.key.as_ref())?;
        let value = attr
            .decode_and_unescape_value(reader.decoder())
            .with_context(|| format!("decoding attribute {key:?}"))?
            .into_owned();
        out.push((key, value));
    }
    Ok(out)
}

fn take_required_attr(attrs: &mut Vec<(String, String)>, key: &str) -> Result<String> {
    let index = attrs
        .iter()
        .position(|(name, _)| name == key)
        .with_context(|| format!("missing required attribute {key:?}"))?;
    Ok(attrs.remove(index).1)
}

fn xml_name(bytes: &[u8]) -> Result<String> {
    std::str::from_utf8(bytes)
        .context("XML name is not UTF-8")
        .map(str::to_owned)
}

pub(super) fn print_panorama_layout_xml(layout_root: &Value) -> Result<String> {
    let root = layout_root
        .get("m_AST")
        .and_then(|ast| ast.get("m_pRoot"))
        .context("unknown LaCo format: missing m_AST.m_pRoot")?;
    let mut out =
        "<!-- xml reconstructed by vpkmerge from Source 2 Panorama LaCo -->\n".to_string();
    print_layout_node(root, 0, &mut out)?;
    Ok(out)
}

fn print_layout_node(node: &Value, indent: usize, out: &mut String) -> Result<()> {
    let node_type = object_string(node, "eType")?;
    match node_type {
        "ROOT" => print_panel_base("root", node, indent, out),
        "STYLES" => print_panel_base("styles", node, indent, out),
        "SCRIPTS" => print_panel_base("scripts", node, indent, out),
        "SNIPPETS" => print_panel_base("snippets", node, indent, out),
        "INCLUDE" => print_include(node, indent, out),
        "PANEL" => {
            let name = object_string_or_empty(node, "name");
            print_panel_base(name, node, indent, out)
        }
        "SCRIPT_BODY" => {
            print_script_body(node, indent, out);
            Ok(())
        }
        "SNIPPET" => print_snippet(node, indent, out),
        _ => bail!("unknown Panorama layout node type {node_type:?}"),
    }
}

fn print_panel_base(name: &str, node: &Value, indent: usize, out: &mut String) -> Result<()> {
    let children = layout_sub_nodes(node);
    let (attributes, child_nodes): (Vec<_>, Vec<_>) = children
        .iter()
        .copied()
        .partition(|child| object_string(child, "eType").ok() == Some("PANEL_ATTRIBUTE"));

    write_indent(out, indent);
    out.push('<');
    out.push_str(name);
    for attribute in attributes {
        let attr_name = object_string(attribute, "name")?;
        let value = attribute
            .get("child")
            .context("PANEL_ATTRIBUTE missing child value")?;
        out.push(' ');
        out.push_str(attr_name);
        out.push_str("=\"");
        out.push_str(&escape_xml_attribute(&layout_reference_value(value)?));
        out.push('"');
    }

    if child_nodes.is_empty() {
        out.push_str(" />\n");
        return Ok(());
    }

    out.push_str(">\n");
    for child in child_nodes {
        print_layout_node(child, indent + 1, out)?;
    }
    write_indent(out, indent);
    out.push_str("</");
    out.push_str(name);
    out.push_str(">\n");
    Ok(())
}

fn print_include(node: &Value, indent: usize, out: &mut String) -> Result<()> {
    let reference = node.get("child").context("INCLUDE missing child")?;
    write_indent(out, indent);
    out.push_str("<include src=\"");
    out.push_str(&escape_xml_attribute(&layout_reference_value(reference)?));
    out.push_str("\" />\n");
    Ok(())
}

fn print_script_body(node: &Value, indent: usize, out: &mut String) {
    let content = object_string_or_empty(node, "name");
    write_indent(out, indent);
    out.push_str("<script><![CDATA[");
    out.push_str(content);
    out.push_str("]]></script>\n");
}

fn print_snippet(node: &Value, indent: usize, out: &mut String) -> Result<()> {
    let name = object_string_or_empty(node, "name");
    write_indent(out, indent);
    out.push_str("<snippet name=\"");
    out.push_str(&escape_xml_attribute(name));
    out.push_str("\">\n");
    for child in layout_sub_nodes(node) {
        print_layout_node(child, indent + 1, out)?;
    }
    write_indent(out, indent);
    out.push_str("</snippet>\n");
    Ok(())
}

fn layout_reference_value(value: &Value) -> Result<String> {
    let name = object_string_or_empty(value, "name");
    let node_type = object_string(value, "eType")?;
    Ok(match node_type {
        "REFERENCE_COMPILED" => format!("s2r://{name}"),
        "REFERENCE_PASSTHROUGH" => format!("file://{name}"),
        "PANEL_ATTRIBUTE_VALUE" => name.to_string(),
        _ => bail!("unknown Panorama attribute/reference node type {node_type:?}"),
    })
}

fn layout_sub_nodes(node: &Value) -> Vec<&Value> {
    if let Some(children) = node.get("vecChildren").and_then(Value::as_array) {
        return children.iter().collect();
    }
    if let Some(child) = node.get("child") {
        return vec![child];
    }
    Vec::new()
}

fn object_string<'a>(node: &'a Value, key: &str) -> Result<&'a str> {
    node.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("layout node missing string property {key:?}"))
}

fn object_string_or_empty<'a>(node: &'a Value, key: &str) -> &'a str {
    node.get(key).and_then(Value::as_str).unwrap_or("")
}

fn write_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push('\t');
    }
}

fn escape_xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codec is a stable fixed point on the node types real layouts use:
    /// decompiling a compiled layout and recompiling it reproduces the same AST.
    /// This is the invariant `rebuild_panorama_layout_xml_resource` enforces.
    #[test]
    fn codec_round_trips_supported_node_types() {
        let xml = br#"<root>
	<styles>
		<include src="s2r://panorama/styles/example.vcss_c" />
	</styles>
	<snippets>
		<snippet name="Demo">
			<Panel id="PanelA" class="foo" hittest="false" />
		</snippet>
	</snippets>
	<Panel class="Bar">
		<Label text="hi &amp; bye" />
	</Panel>
</root>
"#;
        let layout = compile_panorama_layout_xml(xml).expect("compile");
        let printed = print_panorama_layout_xml(&layout).expect("print");
        let recompiled = compile_panorama_layout_xml(printed.as_bytes()).expect("recompile");
        let format = morphic::kv3::Format([1; 16]);
        assert_eq!(
            morphic::kv3::encode(&recompiled, &format),
            morphic::kv3::encode(&layout, &format),
            "compile->print->compile must be a fixed point"
        );
    }

    /// The faithfulness gate passes for a layout built from our own compiler
    /// (it carries no `sourceLineColumn` and only modeled node types).
    #[test]
    fn faithfulness_gate_accepts_codec_native_layout() {
        let xml =
            br#"<root><styles><include src="s2r://panorama/styles/a.vcss_c" /></styles></root>"#;
        let layout = compile_panorama_layout_xml(xml).expect("compile");
        assert!(layout_codec_reproduces(&layout).expect("gate"));
    }

    /// The gate rejects a layout whose compiled form uses a construct the codec
    /// canonicalizes away. Here a single-child container stored as `child` (what
    /// shipped HUD layouts do) recompiles to `vecChildren`, so the gate must fail
    /// and the rebuild fall back to raw rather than silently restructure it.
    #[test]
    fn faithfulness_gate_rejects_single_child_container() {
        let original = Value::Object(vec![(
            "m_AST".to_owned(),
            Value::Object(vec![(
                "m_pRoot".to_owned(),
                Value::Object(vec![
                    ("eType".to_owned(), Value::String("ROOT".to_owned())),
                    (
                        "child".to_owned(),
                        Value::Object(vec![(
                            "eType".to_owned(),
                            Value::String("STYLES".to_owned()),
                        )]),
                    ),
                ]),
            )]),
        )]);
        assert!(!layout_codec_reproduces(&original).expect("gate"));
    }

    /// Local fidelity audit against real compiled layouts. Gated on
    /// `VPKMERGE_VXML_DIR` (a directory of `.vxml_c`), mirroring the repo's other
    /// real-asset tests (`DEADLOCK_PAK`, `MORPHIC_MODEL_VPK`): a no-op in CI,
    /// proof against shipped HUD layouts locally. The invariant we assert is the
    /// safety one: a file is *either* faithfully reproducible (so a rebuild is
    /// exact) *or* the gate rejects it (so the build falls back to raw). It is
    /// never silently rewritten. Reports the faithful/fallback split.
    #[test]
    fn real_vxml_layouts_are_reproduced_or_rejected() {
        let Ok(dir) = std::env::var("VPKMERGE_VXML_DIR") else {
            return;
        };
        let (mut faithful, mut fallback) = (0u32, 0u32);
        for entry in std::fs::read_dir(&dir).expect("read VPKMERGE_VXML_DIR") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("vxml_c") {
                continue;
            }
            let bytes = std::fs::read(&path).expect("read vxml_c");
            // Skip files morphic cannot even decompile (already raw-only on dump).
            let Ok(original) =
                morphic::kv3::decode(resource_block(&bytes, *b"LaCo").expect("LaCo block"))
            else {
                continue;
            };
            if layout_codec_reproduces(&original).expect("gate") {
                faithful += 1;
            } else {
                fallback += 1;
            }
        }
        eprintln!("vxml faithfulness: {faithful} reproduced, {fallback} fall back to raw");
        assert!(
            faithful + fallback > 0,
            "no decodable .vxml_c found in {dir}"
        );
    }
}
