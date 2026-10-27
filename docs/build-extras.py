#!/usr/bin/env python3
"""Generate sitemap.xml and llms-full.txt for the mdBook site.

Run after `mdbook build docs`:
    python3 docs/build-extras.py
"""
import datetime
import os
import re
import sys

BASE_URL = os.environ.get("SITE_URL", "https://mingye-lu.github.io/AgenticCrawler/")
DOCS = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(DOCS, "src")
OUT = os.path.join(DOCS, "book")


def chapters():
    """Markdown files in SUMMARY.md order."""
    with open(os.path.join(SRC, "SUMMARY.md"), encoding="utf-8") as f:
        return re.findall(r"\]\(([^)]+\.md)\)", f.read())


def main():
    if not os.path.isdir(OUT):
        sys.exit(f"{OUT} not found; run `mdbook build docs` first")
    files = chapters()
    today = datetime.date.today().isoformat()

    # The first chapter is served as index.html, so list it once, at the root.
    urls = [BASE_URL] + [BASE_URL + p[:-3] + ".html" for p in files[1:]]
    with open(os.path.join(OUT, "sitemap.xml"), "w", encoding="utf-8") as f:
        f.write('<?xml version="1.0" encoding="UTF-8"?>\n')
        f.write('<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n')
        for u in dict.fromkeys(urls):
            f.write(f"  <url><loc>{u}</loc><lastmod>{today}</lastmod></url>\n")
        f.write("</urlset>\n")

    parts = []
    for p in files:
        with open(os.path.join(SRC, p), encoding="utf-8") as f:
            text = f.read()
        # Drop embedded HTML blocks (video, JSON-LD) that mean nothing as text.
        text = re.sub(r"<video.*?</video>\s*", "", text, flags=re.S)
        text = re.sub(r"<script.*?</script>\s*", "", text, flags=re.S)
        parts.append(f"<!-- {BASE_URL}{p[:-3]}.html -->\n\n{text.strip()}\n")
    with open(os.path.join(OUT, "llms-full.txt"), "w", encoding="utf-8") as f:
        f.write("\n\n---\n\n".join(parts))

    print(f"sitemap.xml: {len(dict.fromkeys(urls))} urls; llms-full.txt: {len(files)} pages")


if __name__ == "__main__":
    main()
