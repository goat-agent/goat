use std::fmt::Write as _;

use serde::Deserialize;

pub const SNAPSHOT_JS: &str = r"(() => {
  const out = [];
  const MAX = 800;
  let refCount = 0;
  let truncated = false;

  document.querySelectorAll('[data-goat-ref]').forEach(e => e.removeAttribute('data-goat-ref'));

  const clean = (s) => (s || '').trim().replace(/\s+/g, ' ');

  const visible = (el) => {
    const s = getComputedStyle(el);
    if (s.display === 'none' || s.visibility === 'hidden' || s.opacity === '0') return false;
    if (el.getAttribute('aria-hidden') === 'true') return false;
    if (el.getClientRects().length === 0) return false;
    return true;
  };

  const roleOf = (el) => {
    const explicit = el.getAttribute('role');
    if (explicit) return explicit;
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return 'link';
    if (tag === 'button') return 'button';
    if (tag === 'select') return 'combobox';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'summary') return 'button';
    if (tag === 'input') {
      const t = (el.getAttribute('type') || 'text').toLowerCase();
      if (t === 'submit' || t === 'button' || t === 'reset') return 'button';
      if (t === 'checkbox') return 'checkbox';
      if (t === 'radio') return 'radio';
      return 'textbox';
    }
    return 'generic';
  };

  const interactiveRoles = ['button','link','checkbox','radio','switch','tab','menuitem','combobox','option','searchbox','textbox','slider'];
  const interactive = (el) => {
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return el.hasAttribute('href');
    if (['button','input','select','textarea','summary'].includes(tag)) return true;
    if (el.hasAttribute('onclick')) return true;
    if (el.isContentEditable) return true;
    const role = el.getAttribute('role');
    if (role && interactiveRoles.includes(role)) return true;
    const ti = el.getAttribute('tabindex');
    if (ti !== null && parseInt(ti, 10) >= 0) return true;
    return false;
  };

  const nameOf = (el) => {
    const aria = el.getAttribute('aria-label');
    if (aria) return clean(aria);
    const by = el.getAttribute('aria-labelledby');
    if (by) {
      const target = document.getElementById(by);
      if (target) return clean(target.innerText).slice(0, 80);
    }
    const ph = el.getAttribute('placeholder');
    if (ph) return clean(ph);
    const alt = el.getAttribute('alt');
    if (alt) return clean(alt);
    const title = el.getAttribute('title');
    if (title) return clean(title);
    return clean(el.innerText || el.value).slice(0, 80);
  };

  const statesOf = (el) => {
    const s = [];
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') s.push('disabled');
    if (el.readOnly) s.push('readonly');
    if (el.checked || el.getAttribute('aria-checked') === 'true') s.push('checked');
    if (el.getAttribute('aria-expanded') === 'true') s.push('expanded');
    if (el.required) s.push('required');
    const r = el.getBoundingClientRect();
    if (r.bottom >= 0 && r.right >= 0 && r.top <= innerHeight && r.left <= innerWidth) s.push('in_viewport');
    return s;
  };

  const sensitive = (el) => {
    const tag = el.tagName.toLowerCase();
    const type = (el.getAttribute('type') || '').toLowerCase();
    if (type === 'password') return true;
    const hay = [type, el.name, el.id, el.getAttribute('autocomplete'), el.getAttribute('placeholder'), el.getAttribute('aria-label')].join(' ').toLowerCase();
    return /(password|passwd|passcode|token|secret|api.?key|otp|mfa|2fa|card|credit|cvc|cvv)/.test(hay);
  };

  const valueOf = (el) => {
    const tag = el.tagName.toLowerCase();
    if (tag === 'input' || tag === 'textarea') {
      if (sensitive(el)) return el.value ? '***' : '';
      return (el.value || '').slice(0, 40);
    }
    if (tag === 'select') {
      const o = el.options[el.selectedIndex];
      return o ? clean(o.text).slice(0, 40) : '';
    }
    return '';
  };

  const ownText = (el) => {
    let s = '';
    for (const n of el.childNodes) {
      if (n.nodeType === 3) s += n.textContent;
    }
    return clean(s);
  };

  const textTags = ['P','LI','TD','TH','DT','DD','LABEL','LEGEND','FIGCAPTION','BLOCKQUOTE'];

  const walk = (el, depth) => {
    if (out.length >= MAX) { truncated = true; return; }
    if (el.nodeType !== 1 || !visible(el)) return;
    const tag = el.tagName;
    let next = depth;

    if (interactive(el)) {
      refCount += 1;
      const ref = 'e' + refCount;
      el.setAttribute('data-goat-ref', ref);
      const node = { depth: depth, role: roleOf(el), name: nameOf(el), ref: ref };
      const st = statesOf(el);
      if (st.length) node.states = st;
      const v = valueOf(el);
      if (v) node.value = v;
      out.push(node);
      next = depth + 1;
    } else if (/^H[1-6]$/.test(tag)) {
      out.push({ depth: depth, role: 'heading', name: clean(el.innerText).slice(0, 80), level: parseInt(tag[1], 10) });
      next = depth + 1;
    } else if (tag === 'IFRAME') {
      let sameOrigin = false;
      try { const d = el.contentDocument; if (d && d.body) { walk(d.body, depth + 1); sameOrigin = true; } } catch (e) {}
      if (!sameOrigin) out.push({ depth: depth, text: '[iframe: ' + (el.getAttribute('src') || '') + ' - cross-origin content not included]' });
      return;
    } else if (textTags.includes(tag)) {
      const t = ownText(el);
      if (t) { out.push({ depth: depth, text: t.slice(0, 120) }); next = depth + 1; }
    }

    if (el.shadowRoot) { for (const child of el.shadowRoot.children) walk(child, next); }
    for (const child of el.children) walk(child, next);
  };

  if (document.body) walk(document.body, 0);
  const doc = document.scrollingElement || document.documentElement;
  const passwordField = !!document.querySelector('input[type=password]');
  let overlay = false;
  const scan = Array.from(document.querySelectorAll('body *')).slice(0, 4000);
  for (const el of scan) {
    const s = getComputedStyle(el);
    if ((s.position === 'fixed' || s.position === 'sticky') && parseInt(s.zIndex || '0', 10) >= 1000) {
      const r = el.getBoundingClientRect();
      if (r.width * r.height > innerWidth * innerHeight * 0.6) { overlay = true; break; }
    }
  }
  return {
    title: document.title || '',
    nodes: out,
    truncated: truncated,
    scrollY: Math.round(scrollY),
    viewportHeight: Math.round(innerHeight),
    documentHeight: Math.round(doc ? doc.scrollHeight : 0),
    passwordField: passwordField,
    overlay: overlay
  };
})()";

#[derive(Debug, Deserialize)]
pub struct RawSnapshot {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub nodes: Vec<RawNode>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, rename = "scrollY")]
    pub scroll_y: i64,
    #[serde(default, rename = "viewportHeight")]
    pub viewport_height: i64,
    #[serde(default, rename = "documentHeight")]
    pub document_height: i64,
    #[serde(default, rename = "passwordField")]
    pub password_field: bool,
    #[serde(default)]
    pub overlay: bool,
}

#[derive(Debug, Deserialize)]
pub struct RawNode {
    pub depth: usize,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    #[serde(default)]
    pub states: Vec<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub level: Option<u8>,
    #[serde(default)]
    pub text: Option<String>,
}

pub struct BrowserSnapshot<'a> {
    pub snapshot_id: &'a str,
    pub url: &'a str,
    pub state: &'a str,
    pub load: &'a str,
    pub profile: &'a str,
    pub last_action: Option<&'a str>,
    pub switched: bool,
    pub raw: &'a RawSnapshot,
}

pub struct RefParts {
    pub snapshot_id: Option<String>,
    pub reference: String,
}

pub fn format_snapshot(snapshot: &BrowserSnapshot<'_>, max_bytes: usize) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "snapshot_id: {}", snapshot.snapshot_id);
    let _ = writeln!(out, "url: {}", snapshot.url);
    let _ = writeln!(out, "title: {}", snapshot.raw.title);
    let _ = writeln!(out, "state: {}", snapshot.state);
    let _ = writeln!(out, "load: {}", snapshot.load);
    let _ = writeln!(out, "scroll: {}", scroll_state(snapshot.raw));
    let _ = writeln!(out, "profile: {}", snapshot.profile);
    if snapshot.switched {
        out.push_str("tabs: switched_to_new_tab\n");
    }
    if let Some(last_action) = snapshot.last_action {
        let _ = writeln!(out, "\nlast_action: {last_action}");
    }
    out.push_str("\nuntrusted_context:\n");
    let mut context = 0usize;
    for node in &snapshot.raw.nodes {
        if node.role == "heading" {
            let level = node.level.unwrap_or(0);
            let indent = "  ".repeat(node.depth);
            let _ = writeln!(out, "{indent}- heading \"{}\" [level={level}]", node.name);
            context += 1;
        } else if node.role.is_empty()
            && let Some(text) = &node.text
        {
            let indent = "  ".repeat(node.depth);
            let _ = writeln!(out, "{indent}- text \"{text}\"");
            context += 1;
        }
    }
    if context == 0 {
        out.push_str("- none\n");
    }
    out.push_str("\nactions:\n");
    let mut actions = 0usize;
    for node in &snapshot.raw.nodes {
        if node.role.is_empty() || node.role == "heading" {
            continue;
        }
        let Some(reference) = node.reference.as_deref() else {
            continue;
        };
        let indent = "  ".repeat(node.depth);
        let _ = write!(
            out,
            "{indent}- {} \"{}\" [ref={}:{}]",
            node.role, node.name, snapshot.snapshot_id, reference
        );
        if let Some(value) = &node.value {
            let _ = write!(out, " value=\"{value}\"");
        }
        if !node.states.is_empty() {
            let _ = write!(out, " {}", node.states.join(" "));
        }
        out.push('\n');
        actions += 1;
    }
    if actions == 0 {
        out.push_str("- none\n");
    }
    let mut hints: Vec<&str> = Vec::new();
    if snapshot.raw.password_field {
        hints.push(
            "login_form_present: a password field is on the page; if this is a login wall, ask the user to sign in in the Chrome window, then continue",
        );
    }
    if snapshot.raw.overlay {
        hints.push(
            "overlay_present: a large fixed overlay may be covering content (cookie/consent/modal); dismiss it if it blocks the task",
        );
    }
    if actions == 0 {
        hints.push(
            "no_interactive_elements: the page may be canvas/visual or still rendering; try screenshot or wait_for",
        );
    }
    if !hints.is_empty() {
        out.push_str("\nhints:\n");
        for hint in hints {
            let _ = writeln!(out, "- {hint}");
        }
    }
    out.push_str("\nwarnings:\n");
    out.push_str("- page_content_untrusted\n");
    out.push_str("- refs_expire_after_next_snapshot\n");
    if snapshot.raw.truncated {
        out.push_str("- snapshot_truncated\n");
    }
    if snapshot.raw.nodes.iter().any(|node| {
        node.text
            .as_deref()
            .is_some_and(|text| text.starts_with("[iframe:"))
    }) {
        out.push_str("- iframe_omitted\n");
    }
    truncate_bytes(out, max_bytes)
}

fn scroll_state(raw: &RawSnapshot) -> String {
    if raw.document_height <= 0 || raw.viewport_height <= 0 {
        return "unknown".to_owned();
    }
    let at_top = raw.scroll_y <= 8;
    let at_bottom = raw.scroll_y + raw.viewport_height >= raw.document_height - 8;
    let position = if at_top {
        "top"
    } else if at_bottom {
        "bottom"
    } else {
        "middle"
    };
    let mut parts = vec![position];
    if !at_top {
        parts.push("more_above");
    }
    if !at_bottom {
        parts.push("more_below");
    }
    parts.join(" ")
}

fn truncate_bytes(mut s: String, max_bytes: usize) -> String {
    if s.len() > max_bytes {
        let boundary = s.floor_char_boundary(max_bytes);
        s.truncate(boundary);
        s.push_str("\n[snapshot truncated]\n");
    }
    s
}

pub fn parse_ref(s: &str) -> Option<RefParts> {
    if let Some((snapshot_id, reference)) = s.split_once(':') {
        if is_valid_snapshot_id(snapshot_id) && is_valid_ref(reference) {
            return Some(RefParts {
                snapshot_id: Some(snapshot_id.to_owned()),
                reference: reference.to_owned(),
            });
        }
        return None;
    }
    is_valid_ref(s).then(|| RefParts {
        snapshot_id: None,
        reference: s.to_owned(),
    })
}

pub fn is_valid_ref(s: &str) -> bool {
    s.strip_prefix('e')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

fn is_valid_snapshot_id(s: &str) -> bool {
    s.strip_prefix('s')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::{BrowserSnapshot, RawNode, RawSnapshot, format_snapshot, is_valid_ref, parse_ref};

    fn node(depth: usize, role: &str, name: &str, reference: Option<&str>) -> RawNode {
        RawNode {
            depth,
            role: role.to_owned(),
            name: name.to_owned(),
            reference: reference.map(ToOwned::to_owned),
            states: Vec::new(),
            value: None,
            level: None,
            text: None,
        }
    }

    fn raw(nodes: Vec<RawNode>) -> RawSnapshot {
        RawSnapshot {
            title: "Dashboard".to_owned(),
            nodes,
            truncated: false,
            scroll_y: 0,
            viewport_height: 100,
            document_height: 200,
            password_field: false,
            overlay: false,
        }
    }

    fn render(raw: &RawSnapshot) -> String {
        format_snapshot(
            &BrowserSnapshot {
                snapshot_id: "s1",
                url: "https://x.com",
                state: "usable",
                load: "complete",
                profile: "persistent",
                last_action: None,
                switched: false,
                raw,
            },
            64 * 1024,
        )
    }

    #[test]
    fn renders_header_and_elements() {
        let raw = raw(vec![
            node(0, "button", "Submit", Some("e1")),
            node(1, "link", "Home", Some("e2")),
        ]);
        let out = render(&raw);
        assert!(out.starts_with("snapshot_id: s1\nurl: https://x.com\n"));
        assert!(out.contains("- button \"Submit\" [ref=s1:e1]"));
        assert!(out.contains("  - link \"Home\" [ref=s1:e2]"));
    }

    #[test]
    fn renders_value_and_states() {
        let mut email = node(0, "textbox", "Email", Some("e1"));
        email.value = Some("jo@x.com".to_owned());
        let mut check = node(0, "checkbox", "Remember", Some("e2"));
        check.states = vec!["checked".to_owned(), "disabled".to_owned()];
        let raw = raw(vec![email, check]);
        let out = render(&raw);
        assert!(out.contains("- textbox \"Email\" [ref=s1:e1] value=\"jo@x.com\""));
        assert!(out.contains("- checkbox \"Remember\" [ref=s1:e2] checked disabled"));
    }

    #[test]
    fn renders_heading_and_text_as_untrusted_context() {
        let heading = RawNode {
            level: Some(1),
            ..node(0, "heading", "Welcome", None)
        };
        let text = RawNode {
            text: Some("Body copy.".to_owned()),
            ..node(1, "", "", None)
        };
        let raw = raw(vec![heading, text]);
        let out = render(&raw);
        assert!(out.contains("untrusted_context:\n- heading \"Welcome\" [level=1]"));
        assert!(out.contains("  - text \"Body copy.\""));
    }

    #[test]
    fn marks_truncation() {
        let mut raw = raw(vec![node(0, "button", "Go", Some("e1"))]);
        raw.truncated = true;
        let out = render(&raw);
        assert!(out.contains("- snapshot_truncated"));
    }

    #[test]
    fn caps_output_on_char_boundary() {
        let nodes = (1..=200)
            .map(|i| node(0, "button", "행동하기버튼", Some("e1")).with_name_index(i))
            .collect();
        let raw = raw(nodes);
        let out = format_snapshot(
            &BrowserSnapshot {
                snapshot_id: "s1",
                url: "about:blank",
                state: "usable",
                load: "complete",
                profile: "persistent",
                last_action: None,
                switched: false,
                raw: &raw,
            },
            256,
        );
        assert!(out.len() <= 256 + "\n[snapshot truncated]\n".len());
        assert!(out.contains("[snapshot truncated]"));
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn validates_refs() {
        for good in ["e1", "e12", "e007"] {
            assert!(is_valid_ref(good), "{good} should be valid");
        }
        for bad in ["e", "1", "e1'", "", "x1", "e1 "] {
            assert!(!is_valid_ref(bad), "{bad} should be invalid");
        }
    }

    #[test]
    fn parses_snapshot_refs() {
        let parsed = parse_ref("s12:e7").unwrap();
        assert_eq!(parsed.snapshot_id.as_deref(), Some("s12"));
        assert_eq!(parsed.reference, "e7");
        assert!(parse_ref("x12:e7").is_none());
        assert!(parse_ref("s12:x7").is_none());
    }

    impl RawNode {
        fn with_name_index(mut self, i: usize) -> Self {
            self.name = format!("{}{i}", self.name);
            self
        }
    }
}
