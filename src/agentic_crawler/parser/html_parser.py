from __future__ import annotations

from dataclasses import dataclass, field

from bs4 import BeautifulSoup, Tag


@dataclass
class FormField:
    name: str
    type: str
    selector: str
    value: str = ""
    placeholder: str = ""


@dataclass
class FormInfo:
    action: str
    method: str
    selector: str
    fields: list[FormField] = field(default_factory=list)


@dataclass
class LinkInfo:
    text: str
    href: str
    selector: str


@dataclass
class PageContent:
    title: str
    url: str
    main_text: str
    links: list[LinkInfo] = field(default_factory=list)
    forms: list[FormInfo] = field(default_factory=list)
    tables: list[list[list[str]]] = field(default_factory=list)
    meta_description: str = ""


def parse_html(html: str, url: str = "", max_text_length: int = 8000) -> PageContent:
    """Parse HTML into a structured PageContent for the agent."""
    soup = BeautifulSoup(html, "lxml")

    # Title
    title = soup.title.get_text(strip=True) if soup.title else ""

    # Meta description
    meta_desc = ""
    meta_tag = soup.find("meta", attrs={"name": "description"})
    if meta_tag and isinstance(meta_tag, Tag):
        meta_desc = meta_tag.get("content", "")
        if isinstance(meta_desc, list):
            meta_desc = meta_desc[0] if meta_desc else ""

    # Remove script/style noise
    for tag in soup(["script", "style", "noscript", "svg", "path"]):
        tag.decompose()

    # Main text
    main_text = soup.get_text(separator="\n", strip=True)
    if len(main_text) > max_text_length:
        main_text = main_text[:max_text_length] + "\n... [truncated]"

    # Links (deduplicated, top 50)
    links: list[LinkInfo] = []
    seen_hrefs: set[str] = set()
    for i, a_tag in enumerate(soup.find_all("a", href=True)):
        href = a_tag["href"]
        if isinstance(href, list):
            href = href[0]
        text = a_tag.get_text(strip=True)
        if href in seen_hrefs or not text:
            continue
        seen_hrefs.add(href)
        links.append(LinkInfo(text=text[:100], href=href, selector=f"a:nth-of-type({i + 1})"))
        if len(links) >= 50:
            break

    # Forms
    forms: list[FormInfo] = []
    for i, form_tag in enumerate(soup.find_all("form")):
        action = form_tag.get("action", "")
        if isinstance(action, list):
            action = action[0] if action else ""
        method = form_tag.get("method", "get")
        if isinstance(method, list):
            method = method[0] if method else "get"
        form_selector = f"form:nth-of-type({i + 1})"

        fields: list[FormField] = []
        for inp in form_tag.find_all(["input", "textarea", "select"]):
            name = inp.get("name", "")
            if isinstance(name, list):
                name = name[0] if name else ""
            if not name:
                continue
            inp_type = inp.get("type", "text")
            if isinstance(inp_type, list):
                inp_type = inp_type[0] if inp_type else "text"
            placeholder = inp.get("placeholder", "")
            if isinstance(placeholder, list):
                placeholder = placeholder[0] if placeholder else ""
            fields.append(
                FormField(
                    name=name,
                    type=inp_type,
                    selector=f'{form_selector} [name="{name}"]',
                    placeholder=placeholder,
                )
            )
        forms.append(
            FormInfo(action=action, method=method, selector=form_selector, fields=fields)
        )

    # Tables (top 5, simple extraction)
    tables: list[list[list[str]]] = []
    for table_tag in soup.find_all("table")[:5]:
        rows: list[list[str]] = []
        for tr in table_tag.find_all("tr"):
            cells = [td.get_text(strip=True) for td in tr.find_all(["td", "th"])]
            if cells:
                rows.append(cells)
        if rows:
            tables.append(rows)

    return PageContent(
        title=title,
        url=url,
        main_text=main_text,
        links=links,
        forms=forms,
        tables=tables,
        meta_description=meta_desc,
    )


def page_content_to_text(content: PageContent) -> str:
    """Convert PageContent to a compact text summary for the LLM."""
    parts = [f"# {content.title}", f"URL: {content.url}"]

    if content.meta_description:
        parts.append(f"Description: {content.meta_description}")

    parts.append(f"\n## Page Text\n{content.main_text}")

    if content.links:
        parts.append("\n## Links")
        for link in content.links[:30]:
            parts.append(f"- [{link.text}]({link.href}) — selector: `{link.selector}`")

    if content.forms:
        parts.append("\n## Forms")
        for form in content.forms:
            parts.append(f"- Form: action={form.action} method={form.method} selector=`{form.selector}`")
            for f in form.fields:
                parts.append(f"  - {f.name} (type={f.type}) selector=`{f.selector}`")

    if content.tables:
        parts.append(f"\n## Tables ({len(content.tables)} found)")
        for i, table in enumerate(content.tables[:3]):
            parts.append(f"Table {i + 1}: {len(table)} rows")
            for row in table[:5]:
                parts.append("  | " + " | ".join(row) + " |")
            if len(table) > 5:
                parts.append(f"  ... ({len(table) - 5} more rows)")

    return "\n".join(parts)
