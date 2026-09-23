#!/usr/bin/env python3
"""Build the course HTML from chapters.py.

The wrapper (nav, sidebar, prev/next) is generated so it cannot drift between
chapters; only <main> is written by hand. elyracode.com renders <main> alone,
so the wrapper matters for reading these files straight from the repository.
"""

import pathlib
import re

from chapters import CHAPTERS, COURSE_TITLE, COURSE_LEAD, COURSE_INTRO

HERE = pathlib.Path(__file__).parent


def head(title: str) -> str:
    return f"""<!doctype html>
<html lang="en">
    <head>
        <meta charset="UTF-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1.0" />
        <title>{title} &mdash; Elyra Sj&aacute;</title>
    </head>
    <body>
        <nav class="topnav">
            <div class="topnav__inner">
                <a href="index.html" class="topnav__logo">Elyra Sj&aacute;</a>
            </div>
        </nav>
        <div class="layout">
"""


def sidebar(current: int, sections: list[tuple[str, str]]) -> str:
    rows = "\n".join(
        f'                <a href="#{anchor}">{label}</a>' for anchor, label in sections
    )
    prev_next = []
    if current > 1:
        p = CHAPTERS[current - 2]
        prev_next.append(
            f'                    <a href="chapter-{p["n"]:02d}.html">&larr; {p["n"]}. {p["title"]}</a>'
        )
    if current < len(CHAPTERS):
        nx = CHAPTERS[current]
        prev_next.append(
            f'                    <a href="chapter-{nx["n"]:02d}.html">{nx["n"]}. {nx["title"]} &rarr;</a>'
        )
    links = "\n".join(prev_next)
    return f"""            <aside class="sidebar">
{rows}
                <div class="sidebar__group">
                    <div class="sidebar__group-label">Course</div>
                    <a href="index.html">Overview</a>
{links}
                </div>
            </aside>
"""


def anchors(body: str) -> list[tuple[str, str]]:
    return [
        (m.group(1), re.sub(r"<[^>]+>", "", m.group(2)).strip())
        for m in re.finditer(r'<h2 id="([^"]+)">(.*?)</h2>', body, re.S)
    ]


def chapter_page(ch: dict) -> str:
    body = ch["body"].rstrip()
    learned = "\n".join(
        f"                    <li>{item}</li>" for item in ch["learned"]
    )
    nxt = ""
    if ch["n"] < len(CHAPTERS):
        after = CHAPTERS[ch["n"]]
        nxt = f"""
                <div class="callout callout--success">
                    <strong>Next:</strong> in
                    <a href="chapter-{after['n']:02d}.html">Chapter {after['n']}</a>
                    {ch['next']}
                </div>"""
    else:
        nxt = f"""
                <div class="callout callout--success">
                    {ch['next']}
                </div>"""

    full = f"Chapter {ch['n']}: {ch['title']}"
    sections = anchors(body) + [("what-you-learned", "What you learned")]

    return (
        head(full)
        + sidebar(ch["n"], sections)
        + f"""            <main class="main">
                <h1>{full}</h1>
                <p class="lead">
                    {ch['lead']}
                </p>

{body}

                <h2 id="what-you-learned">What you learned</h2>
                <ul>
{learned}
                </ul>
{nxt}
            </main>
        </div>
    </body>
</html>
"""
    )


def index_page() -> str:
    rows = []
    for ch in CHAPTERS:
        rows.append(
            f"""                    <li>
                        <a href="chapter-{ch['n']:02d}.html"><strong>{ch['n']}. {ch['title']}</strong></a>
                        &mdash; {ch['summary']}
                    </li>"""
        )
    toc = "\n".join(rows)
    links = "\n".join(
        f'                <a href="chapter-{ch["n"]:02d}.html">{ch["n"]}. {ch["title"]}</a>'
        for ch in CHAPTERS
    )
    return (
        head(COURSE_TITLE)
        + f"""            <aside class="sidebar">
                <div class="sidebar__group">
                    <div class="sidebar__group-label">Chapters</div>
{links}
                </div>
            </aside>
            <main class="main">
                <h1>{COURSE_TITLE}</h1>
                <p class="lead">
                    {COURSE_LEAD}
                </p>

{COURSE_INTRO}

                <h2 id="chapters">The chapters</h2>
                <ol>
{toc}
                </ol>
            </main>
        </div>
    </body>
</html>
"""
    )


def main() -> None:
    for ch in CHAPTERS:
        (HERE / f"chapter-{ch['n']:02d}.html").write_text(chapter_page(ch))
    (HERE / "index.html").write_text(index_page())
    print(f"wrote {len(CHAPTERS)} chapters + index")


if __name__ == "__main__":
    main()
