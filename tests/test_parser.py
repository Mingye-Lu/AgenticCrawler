from agentic_crawler.parser.html_parser import parse_html, page_content_to_text
from agentic_crawler.parser.readability import extract_main_content


def test_parse_html(sample_html: str) -> None:
    content = parse_html(sample_html, url="https://example.com")

    assert content.title == "Test Page"
    assert content.url == "https://example.com"
    assert "Welcome to Test Page" in content.main_text
    assert len(content.links) == 2
    assert content.links[0].text == "About"
    assert content.links[0].href == "/about"
    assert len(content.forms) == 1
    assert content.forms[0].action == "/search"
    assert len(content.forms[0].fields) >= 1
    assert len(content.tables) == 1
    assert content.tables[0][0] == ["Name", "Price"]


def test_page_content_to_text(sample_html: str) -> None:
    content = parse_html(sample_html, url="https://example.com")
    text = page_content_to_text(content)

    assert "Test Page" in text
    assert "About" in text
    assert "/search" in text


def test_extract_main_content(sample_html: str) -> None:
    text = extract_main_content(sample_html)
    assert "Welcome to Test Page" in text
    assert "Widget A" in text
