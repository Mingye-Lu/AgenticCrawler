from __future__ import annotations

from bs4 import BeautifulSoup


# Tags likely to contain main content
_CONTENT_TAGS = {"article", "main", "section", "div"}
_NOISE_TAGS = {"nav", "header", "footer", "aside", "script", "style", "noscript", "svg", "form"}


def extract_main_content(html: str, max_length: int = 6000) -> str:
    """Extract the main readable content from an HTML page, stripping boilerplate."""
    soup = BeautifulSoup(html, "lxml")

    # Remove noise
    for tag in soup.find_all(_NOISE_TAGS):
        tag.decompose()

    # Try to find the main content container
    main = soup.find("main") or soup.find("article")
    if main:
        text = main.get_text(separator="\n", strip=True)
    else:
        # Fallback: find the largest text block
        body = soup.find("body")
        if body:
            text = body.get_text(separator="\n", strip=True)
        else:
            text = soup.get_text(separator="\n", strip=True)

    # Collapse blank lines
    lines = [line for line in text.splitlines() if line.strip()]
    text = "\n".join(lines)

    if len(text) > max_length:
        text = text[:max_length] + "\n... [truncated]"

    return text
