#!/usr/bin/env python3
"""Builds the Omarchy Studio ROSP project report — one personalised copy per
team member — as .docx (editable) and .pdf (via LibreOffice).

    python3 build_report.py            # all four copies
    python3 build_report.py 231453    # one copy, by roll number

The layout follows the department's report format (cover, certificate,
approval, declaration, acknowledgement, student information, abstract, table
of contents, then numbered chapters with "Page N" footers). Page numbers in
the table of contents are measured from a first rendering and filled in on a
second pass, so they are always exact.
"""
import os
import re
import subprocess
import sys
import tempfile

from docx import Document
from docx.enum.section import WD_SECTION
from docx.enum.table import WD_TABLE_ALIGNMENT, WD_CELL_VERTICAL_ALIGNMENT
from docx.enum.text import WD_ALIGN_PARAGRAPH, WD_BREAK
from docx.oxml import OxmlElement
from docx.oxml.ns import qn
from docx.shared import Pt, Inches, RGBColor
from pypdf import PdfReader

HERE = os.path.dirname(os.path.abspath(__file__))
ASSETS = os.path.join(HERE, "assets")
OUT = os.path.join(HERE, "out")

# ── project facts ────────────────────────────────────────────────────────────
TITLE = "OMARCHY STUDIO"
SUBTITLE = "A Unified, Undo-Safe Desktop Configuration Studio for Omarchy Linux"
COLLEGE = "M. H. SABOO SIDDIK COLLEGE OF ENGINEERING"
ADDRESS = "8, Saboo Siddik Polytechnic Road, Byculla, Mumbai, Maharashtra 400008"
DEPT = "DEPARTMENT OF INFORMATION TECHNOLOGY"
COURSE = "ROSP"
COURSE_LONG = "Final Year Project"
SEMESTER = "7th"
ACAD_YEAR = "Academic Year (2026 -27)"
GUIDE = "Prof. Fardeen"
GUIDE_CAPS = "PROF. FARDEEN"
HOD = "Dr. Zainab Mirza"
HOD_CAPS = "DR. ZAINAB MIRZA"
TEAM = [  # in the order given by the team
    ("Aariz Sheikh", "231453"),
    ("Yusuf Mohd Aslam", "231460"),
    ("Samia Loladia", "231463"),
    ("Naved Rar", "231433"),
]
FONT = "Times New Roman"
BODY_PT = 13


# ── low-level helpers ────────────────────────────────────────────────────────
def set_cell_bg(cell, hex_fill):
    tcPr = cell._tc.get_or_add_tcPr()
    shd = OxmlElement("w:shd")
    shd.set(qn("w:val"), "clear")
    shd.set(qn("w:color"), "auto")
    shd.set(qn("w:fill"), hex_fill)
    tcPr.append(shd)


def set_cell_margins(table, top=80, bottom=80, left=100, right=100):
    tblPr = table._tbl.tblPr
    mar = OxmlElement("w:tblCellMar")
    for side, v in (("top", top), ("left", left), ("bottom", bottom), ("right", right)):
        el = OxmlElement(f"w:{side}")
        el.set(qn("w:w"), str(v))
        el.set(qn("w:type"), "dxa")
        mar.append(el)
    tblPr.append(mar)


def add_field(par, instr, style):
    for kind, text in (("begin", None), ("instr", instr), ("separate", None), ("text", "1"), ("end", None)):
        run = par.add_run()
        style(run)
        if kind == "instr":
            el = OxmlElement("w:instrText")
            el.set(qn("xml:space"), "preserve")
            el.text = text
        elif kind == "text":
            el = OxmlElement("w:t")
            el.text = text
        else:
            el = OxmlElement("w:fldChar")
            el.set(qn("w:fldCharType"), kind)
        run._r.append(el)


def keep_with_next(p):
    p.paragraph_format.keep_with_next = True


class Report:
    def __init__(self, student, toc_pages=None):
        self.student = student
        self.toc_pages = toc_pages or {}
        self.doc = Document()
        self.fig = 0
        self.tab = 0
        sec = self.doc.sections[0]
        sec.page_width, sec.page_height = Inches(8.5), Inches(11)
        for s in (sec,):
            s.left_margin = s.right_margin = Inches(1)
            s.top_margin = Inches(0.9)
            s.bottom_margin = Inches(0.9)
        st = self.doc.styles["Normal"]
        st.font.name = FONT
        st.font.size = Pt(BODY_PT)
        st.element.rPr.rFonts.set(qn("w:eastAsia"), FONT)
        st.paragraph_format.space_after = Pt(0)
        st.paragraph_format.line_spacing = 1.15

    # text primitives ---------------------------------------------------------
    def runs(self, p, text, size=None, bold=False, italic=False, color=None):
        """Adds text with **bold**, *italic* and `code` spans."""
        for part in re.split(r"(\*\*.+?\*\*|(?<![\w*])\*[^*\s][^*]*?\*(?![\w*])|`[^`]+`)", text):
            if not part:
                continue
            b, it, code = bold, italic, False
            if part.startswith("**") and part.endswith("**") and len(part) > 4:
                part, b = part[2:-2], True
            elif part.startswith("`") and part.endswith("`") and len(part) > 1:
                part, code = part[1:-1], True
            elif part.startswith("*") and part.endswith("*") and len(part) > 2:
                part, it = part[1:-1], True
            r = p.add_run(part)
            r.bold, r.italic = b, it
            r.font.name = "Courier New" if code else FONT
            if code:
                r.font.size = Pt((size or BODY_PT) - 1.5)
            elif size:
                r.font.size = Pt(size)
            if color:
                r.font.color.rgb = RGBColor.from_string(color)
        return p

    def para(self, text="", size=None, bold=False, italic=False, align="justify",
             before=0, after=8, spacing=1.4, indent=None, first=None, color=None):
        p = self.doc.add_paragraph()
        p.alignment = {"justify": WD_ALIGN_PARAGRAPH.JUSTIFY, "center": WD_ALIGN_PARAGRAPH.CENTER,
                       "left": WD_ALIGN_PARAGRAPH.LEFT, "right": WD_ALIGN_PARAGRAPH.RIGHT}[align]
        pf = p.paragraph_format
        pf.space_before, pf.space_after, pf.line_spacing = Pt(before), Pt(after), spacing
        if indent is not None:
            pf.left_indent = Inches(indent)
        if first is not None:
            pf.first_line_indent = Inches(first)
        self.runs(p, text, size, bold, italic, color)
        return p

    def chapter(self, text):
        """A chapter title on a fresh page, like the reference report."""
        self.page_break()
        p = self.para(text, size=22, bold=True, align="center", after=14, spacing=1.0)
        keep_with_next(p)
        return p

    def title(self, text, size=22, after=16, before=0):
        p = self.para(text, size=size, bold=True, align="center", after=after, before=before, spacing=1.0)
        keep_with_next(p)
        return p

    def sub(self, text, size=15, before=8):
        p = self.para(text, size=size, bold=True, align="left", before=before, after=6, spacing=1.15)
        keep_with_next(p)
        return p

    def bullets(self, items, numbered=False, after=4):
        for i, it in enumerate(items, 1):
            mark = f"{i}." if numbered else "•"
            p = self.para(f"{mark}\t{it}", align="justify", after=after, spacing=1.35, indent=0.35, first=-0.25)
            p.paragraph_format.tab_stops.add_tab_stop(Inches(0.35))

    def page_break(self):
        p = self.doc.add_paragraph()
        p.add_run().add_break(WD_BREAK.PAGE)
        p.paragraph_format.space_after = Pt(0)

    def image(self, path, width_in, after=4):
        p = self.doc.add_paragraph()
        p.alignment = WD_ALIGN_PARAGRAPH.CENTER
        p.paragraph_format.space_after = Pt(after)
        p.add_run().add_picture(path, width=Inches(width_in))
        keep_with_next(p)
        return p

    def figure(self, path, width_in, caption):
        self.fig += 1
        self.image(path, width_in)
        return self.para(f"Figure {self.fig}: {caption}", size=11.5, italic=True, align="center", after=12, spacing=1.1)

    def table(self, header, rows, widths, size=11, header_fill="EAEAEA", align=None, caption=None,
              fills=None, bold_first_col=False, center_all=False):
        if caption:
            self.tab += 1
            cp = self.para(f"Table {self.tab}: {caption}", size=11.5, italic=True, align="center", after=4, spacing=1.1)
            keep_with_next(cp)
        t = self.doc.add_table(rows=1 + len(rows), cols=len(header))
        t.style = "Table Grid"
        t.alignment = WD_TABLE_ALIGNMENT.CENTER
        set_cell_margins(t)
        for r_i, row in enumerate([header] + rows):
            for c_i, val in enumerate(row):
                cell = t.cell(r_i, c_i)
                cell.width = Inches(widths[c_i])
                cell.vertical_alignment = WD_CELL_VERTICAL_ALIGNMENT.CENTER
                p = cell.paragraphs[0]
                p.paragraph_format.space_after = Pt(0)
                p.paragraph_format.line_spacing = 1.1
                if center_all or (align and align[c_i] == "c"):
                    p.alignment = WD_ALIGN_PARAGRAPH.CENTER
                bold = r_i == 0 or (bold_first_col and c_i == 0)
                self.runs(p, str(val), size=size, bold=bold)
                if r_i == 0:
                    set_cell_bg(cell, header_fill)
                elif fills and fills(r_i - 1, c_i, val):
                    set_cell_bg(cell, fills(r_i - 1, c_i, val))
        for row in t.rows:  # keep rows from splitting across pages
            trPr = row._tr.get_or_add_trPr()
            cs = OxmlElement("w:cantSplit")
            trPr.append(cs)
        self.para("", after=6)
        return t

    # page furniture ----------------------------------------------------------
    def start_numbered_section(self):
        sec = self.doc.add_section(WD_SECTION.NEW_PAGE)
        sec.footer.is_linked_to_previous = False
        p = sec.footer.paragraphs[0]
        p.alignment = WD_ALIGN_PARAGRAPH.RIGHT
        r = p.add_run("Page ")
        r.bold, r.font.name, r.font.size = True, FONT, Pt(12)
        add_field(p, "PAGE", lambda run: (setattr(run, "bold", True), setattr(run.font, "name", FONT),
                                          setattr(run.font, "size", Pt(12))))
        pg = OxmlElement("w:pgNumType")
        pg.set(qn("w:start"), "1")
        sec._sectPr.append(pg)
        # the first section (front matter) carries an empty footer
        self.doc.sections[0].footer.is_linked_to_previous = False
        return sec

    def save(self, path):
        self.doc.save(path)


# ── front matter ─────────────────────────────────────────────────────────────
def cover(R):
    name, roll = R.student
    R.image(os.path.join(ASSETS, "logos", "college_logo.png"), 1.55, after=8)
    R.para("Anjuman-I-Islam's", size=12, bold=True, italic=True, align="center", after=1, spacing=1.0)
    R.para(COLLEGE, size=15.5, bold=True, align="center", after=2, spacing=1.0)
    R.para(ADDRESS, size=9.5, bold=True, align="center", after=16, spacing=1.0)
    R.para(DEPT, size=15, bold=True, align="center", after=18, spacing=1.0)
    R.para(COURSE, size=15, bold=True, align="center", after=3, spacing=1.0)
    R.para(f"{COURSE_LONG} (Semester VII)", size=14, bold=True, align="center", after=18, spacing=1.0)
    R.para("Title of the Project", size=14, bold=True, align="center", after=4, spacing=1.0)
    R.para(f"“{TITLE}”", size=25, bold=True, align="center", after=3, spacing=1.0)
    R.para(SUBTITLE, size=12.5, italic=True, align="center", after=14, spacing=1.0)
    R.para("Guide", size=14, bold=True, align="center", after=2, spacing=1.0)
    R.para(f"“{GUIDE} ”", size=17, bold=True, align="center", after=12, spacing=1.0)
    R.para("Submitted by", size=13, bold=True, align="center", after=2, spacing=1.0)
    R.para(f"{name} ({roll})", size=15, bold=True, align="center", after=12, spacing=1.0)
    R.para("REV - 2019 ‘C’ Scheme", size=14, bold=True, align="center", after=16, spacing=1.0)
    R.image(os.path.join(ASSETS, "logos", "mu_logo.png"), 1.35, after=10)
    R.para("University of Mumbai", size=14, bold=True, align="center", after=8, spacing=1.0)
    R.para(ACAD_YEAR, size=14, bold=True, align="center", after=0, spacing=1.0)


def team_inline(sep_last=", "):
    parts = [f"“{n}” ({r})" for n, r in TEAM]
    return ", ".join(parts)


def certificate(R):
    R.page_break()
    R.title("CERTIFICATE", size=24, after=22, before=10)
    names = ", ".join(f"**“{n}”** ({r})" for n, r in TEAM)
    R.para(f"This is to certify that the project entitled **“Omarchy Studio”** is a bonafide work of "
           f"{names} submitted to the **University of Mumbai** in partial fulfillment of the requirement for the "
           f"**{COURSE} – {COURSE_LONG}** of the **{SEMESTER.upper()} Semester** (Final Year, Bachelor of "
           f"Engineering) in the **Department of Information Technology**.", size=14.5, spacing=1.6, after=40)
    for _ in range(2):
        R.para("", after=10)
    R.para(f"({GUIDE_CAPS})", size=11.5, bold=True, align="left", after=14)
    R.para("SUPERVISOR/GUIDE", size=11.5, bold=True, align="left", after=40)
    for _ in range(2):
        R.para("", after=10)
    t = R.doc.add_table(rows=2, cols=2)
    vals = [[f"({HOD_CAPS})", f"({GUIDE_CAPS})"], ["HEAD OF DEPARTMENT", "ROSP CO-ORDINATOR"]]
    for i in range(2):
        for j in range(2):
            p = t.cell(i, j).paragraphs[0]
            p.alignment = WD_ALIGN_PARAGRAPH.LEFT if j == 0 else WD_ALIGN_PARAGRAPH.RIGHT
            p.paragraph_format.space_after = Pt(26 if i == 0 else 0)
            R.runs(p, vals[i][j], size=11.5, bold=True)
    R.para("", after=40)
    R.para("Place : Mumbai", size=13, align="left", after=8)
    R.para("Date :", size=13, align="left")


def approval(R):
    R.page_break()
    R.title("Project Report Approval", size=24, after=20, before=10)
    names = ", ".join(n for n, _ in TEAM[:-1]) + " & " + TEAM[-1][0]
    R.para(f"This project report entitled **“Omarchy Studio”** by **{names}** is approved for the "
           f"**{COURSE} {COURSE_LONG}** of the {SEMESTER} Semester in the Department of Information Technology.",
           size=13, spacing=1.6, after=40)
    R.para("Examiners:", align="right", after=10)
    R.para("--------------------------------------------  (Internal Examiner)", align="right", after=30)
    R.para("-------------------------------------------- (External Examiner)", align="right", after=50)
    R.para("Date:", align="left", after=30)
    R.para("Place: Mumbai", align="left")


def declaration(R):
    name, roll = R.student
    R.page_break()
    R.title("Declaration", size=24, after=20, before=6)
    R.para("I declare that this written submission represents my ideas in my own words and where others' ideas "
           "or words have been included, I have adequately cited and referenced the original sources. I also "
           "declare that I have adhered to all principles of academic honesty and integrity and have not "
           "misrepresented or fabricated or falsified any idea/data/fact/source in my submission. I understand "
           "that any violation of the above will be cause for disciplinary action by the Institute and can also "
           "evoke penal action from the sources which have thus not been properly cited or from whom proper "
           "permission has not been taken when needed.", size=13, bold=True, spacing=1.6, after=50)
    R.para("----------------------------------------", bold=True, align="left", after=14)
    R.para("(Signature)", bold=True, align="left", after=6)
    R.para(f"{name.upper()} {roll}", bold=True, align="left", after=40)
    R.para("Project team:", bold=True, align="left", after=8)
    R.table(["Name", "Roll No.", "Signature"], [[n.upper(), r, ""] for n, r in TEAM], [2.9, 1.3, 2.2],
            size=12, align="lcl", bold_first_col=True)
    R.para("Date:", bold=True, align="left", before=10)


def acknowledgement(R):
    R.page_break()
    R.title("ACKNOWLEDGEMENT", size=22, after=24, before=10)
    names = ", ".join(n for n, _ in TEAM[:-1]) + " and " + TEAM[-1][0]
    body = [
        f"We, the project team — **{names}** — would like to express our heartfelt gratitude to everyone "
        f"who supported and guided us throughout the development of our **{COURSE} {COURSE_LONG}, “Omarchy Studio”**.",
        f"We are especially thankful to our project guide, **{GUIDE} Sir**, for his continuous support, expert "
        "guidance and valuable feedback at every stage of the project. His mentorship played a crucial role in "
        "shaping our ideas, in insisting on careful engineering and testing, and in helping us overcome the "
        "challenges we met while building a tool that edits a live desktop safely.",
        f"We are sincerely grateful to **{HOD} Ma'am**, Head of the Department of Information Technology, for "
        "her encouragement and for providing the facilities and environment that made this work possible.",
        "We also extend our sincere thanks to the faculty and staff of the **Information Technology Department at "
        "M.H. Saboo Siddik College of Engineering** for providing us with the necessary resources and a "
        "productive learning environment, and to the open-source communities behind Omarchy, Hyprland, Rust and "
        "Ratatui, whose documentation and code we learned from.",
        "Our sincere appreciation goes to our friends and classmates for their inputs, testing and motivation. "
        "Last but not least, we are grateful to our families for their patience, encouragement and constant "
        "support during this journey.",
        "This project was a great learning experience for all of us, and we are truly thankful to everyone who "
        "contributed directly or indirectly to its success.",
    ]
    for b in body:
        R.para(b, size=13.5, spacing=1.45, after=12)


def student_info(R):
    R.page_break()
    R.title("Student Information", size=24, after=24, before=10)
    R.table(["Name", "Roll No."], [[n.upper(), r] for n, r in TEAM], [3.2, 3.2], size=14,
            align="cc", bold_first_col=True)
    R.para("", after=6)
    R.table(["Role", "Name"], [["Project Guide", f"{GUIDE} Sir"], ["ROSP Co-ordinator", f"{GUIDE} Sir"],
                               ["Head of Department", f"{HOD} Ma'am"]],
            [3.2, 3.2], size=13, align="cc")


def abstract(R):
    R.page_break()
    R.title("Abstract", size=24, after=14, before=4)
    for b in [
        "**Omarchy Studio** is a keyboard-driven desktop configuration studio for **Omarchy**, an opinionated "
        "Arch Linux distribution built on the **Hyprland** Wayland compositor. Omarchy lets a user pick a theme from "
        "its menu, but every other customisation — keybindings, window gaps and blur, animations, the status "
        "bar, notifications, the lock screen, idle timers and displays — still means hand-editing files in five "
        "different configuration dialects spread across four directory trees, with no discoverability and no undo. "
        "One typo in a compositor file can leave the user with a session that will not reload.",
        "Studio replaces that workflow with a single terminal user interface (TUI) and a matching command-line "
        "interface (CLI). It covers more than twenty modules: themes with live preview and a 114-theme community "
        "browser, a wallpaper browser with wallhaven.cc search, a wizard that extracts a complete colour theme from "
        "any image using median-cut quantisation, keybinds with live chord capture and conflict detection, 56 "
        "Hyprland look-and-feel settings with instant preview, animation presets, bar, notification, OSD and idle "
        "editors, monitor arrangement, power and battery thresholds, safe application removal with a dependency "
        "cascade preview, and a health-checking Doctor.",
        "The system is engineered around trust. Every change passes through one **apply pipeline** — plan, "
        "drift check, pre-snapshot, hash-guarded atomic write, reload, verification and post-snapshot — and "
        "is **rolled back automatically** if the compositor reports configuration errors or a restarted component "
        "dies. Configuration files are edited with **lossless, comment-preserving parsers**, so the user's own "
        "edits survive byte-for-byte, and Omarchy's vendored files are never touched, so system updates cannot "
        "clobber the user's work. Every change is committed to a local **git-backed snapshot store** that can be "
        "browsed as a timeline with coloured diffs and restored to any point.",
        "Studio is written in **Rust** as a two-crate workspace (an engine library and a TUI/CLI binary built on "
        "Ratatui and clap) of roughly 43,700 lines. It is verified by 477 automated tests, terminal end-to-end "
        "journeys run in CI, and has been released publicly as open-source software (MIT licence), supporting both "
        "Omarchy 3.8 and the Lua-configured Omarchy 4 “Quattro”.",
    ]:
        R.para(b, size=12.5, spacing=1.32, after=6)


CHAPTERS = ["Introduction", "Literature Survey", "Proposed System Architecture", "Technology Stack",
            "Diagrams", "Results and Screenshots", "Comparative Analysis", "Conclusion", "Future Scope",
            "References"]


def toc(R):
    R.page_break()
    R.title("Table of Contents", size=24, after=20, before=4)
    rows = [[str(i), c, str(R.toc_pages.get(c, ""))] for i, c in enumerate(CHAPTERS, 1)]
    R.table(["Sr. No.", "Section", "Page no."], rows, [1.4, 3.4, 1.4], size=14, header_fill="EEF2F7",
            center_all=True, bold_first_col=False)
    for row in R.doc.tables[-1].rows[1:]:
        for c in row.cells:
            for r in c.paragraphs[0].runs:
                r.bold = True


# ── chapters ─────────────────────────────────────────────────────────────────
def ch_introduction(R):
    R.chapter("1. Introduction")
    for b in [
        "Linux desktop customisation — popularly called “ricing” — has become one of the main "
        "reasons people adopt tiling Wayland compositors such as Hyprland. **Omarchy**, an opinionated Arch Linux "
        "setup published by David Heinemeier Hansson, packages Hyprland with a curated set of applications and "
        "beautiful themes, and has attracted a large community of new Linux users. Omarchy's menu lets a user switch "
        "the theme in one keystroke, but everything beyond theme selection drops the user into a text editor.",
        "A typical customisation session therefore requires knowledge of **five configuration dialects** — "
        "Hyprland's hyprlang (or Lua on Omarchy 4) for keybinds and window behaviour, JSONC and GTK-CSS for the "
        "Waybar status bar, INI for mako notifications and TOML for colour palettes — spread across four "
        "directory trees. Some of those files are owned by Omarchy and are silently replaced by `omarchy-update`; "
        "others are the user's to edit. There is no list of the settings that exist, no indication of which file a "
        "keybinding came from, no preview of a change before it is saved, and no undo. A single typo in "
        "`hyprland.conf` can break the compositor reload.",
        "**Omarchy Studio** addresses these problems with a single keyboard-driven terminal application and a "
        "matching CLI that together form a “control centre” for the whole desktop. Every editable knob "
        "is presented as a plain-language field with a live preview; the generated configuration line is shown "
        "alongside so the user learns the underlying syntax passively. Every change is written through one "
        "verified apply pipeline that snapshots the files to a local git repository first and rolls the change back "
        "automatically if the desktop rejects it. Studio never writes to Omarchy's own files, so system updates and "
        "Studio's changes cannot fight each other.",
    ]:
        R.para(b)
    R.sub("1.1 Problem Statement")
    R.bullets([
        "**The configuration cliff** — beyond theme selection, every change requires hand-editing files in "
        "dialects most users do not know.",
        "**No discoverability** — which bar modules exist, which animation curves are possible, which of six "
        "sourced binding files owns a shortcut: today the answer is “read the source”.",
        "**No safety net** — a typo breaks the compositor reload, edits to Omarchy-managed files are clobbered "
        "on update, and nothing can be undone.",
        "**Fragmented tooling** — colours are solved by half a dozen tools, monitors by two, while keybinds, "
        "animations, bar layout and notification behaviour have no tool at all.",
    ])
    R.sub("1.2 Objectives")
    R.bullets([
        "Provide zero-jargon editing of every user-facing Omarchy surface through forms, pickers and live previews.",
        "Make every change reversible: snapshot before and after, one-key undo, and a browsable history.",
        "Verify each change against the running desktop and roll back automatically on failure.",
        "Preserve the user's hand-edits byte-for-byte using lossless, comment-preserving parsers.",
        "Integrate natively with Omarchy (menu entry, lifecycle hooks) and survive `omarchy-update`.",
        "Offer a scriptable CLI twin for every screen, driven by the same engine.",
    ], numbered=True)
    R.sub("1.3 Scope")
    R.para("The project targets Omarchy 3.8 (hyprlang configuration) and Omarchy 4.0 “Quattro” (Lua "
           "configuration, Quickshell-based shell) on Hyprland 0.55–0.56. It is a configuration tool for the "
           "user's own desktop; it is not a general dotfile manager, an image editor, a package manager or a daemon "
           "— Studio runs, applies and exits, and lifecycle hooks handle the rest.")


def ch_literature(R):
    R.chapter("2. Literature Survey")
    for b in [
        "Configuration errors are one of the dominant causes of failures in real systems. In [1], Xu and Zhou survey "
        "systems approaches to tackling configuration errors and organise the field into prevention, testing, "
        "diagnosis and troubleshooting. They argue that configuration should be treated as a first-class input that "
        "is validated before use and that recovery mechanisms such as checkpointing and rollback are essential "
        "because many misconfigurations can only be detected at run time. Studio follows exactly this model: every "
        "write is validated by its dialect parser before it reaches disk, verified against the running compositor "
        "after reload, and rolled back from a snapshot on failure.",
        "Yin et al. [2] studied hundreds of real configuration errors from commercial and open-source systems and "
        "found that a large fraction were simple parameter mistakes made by users, and that systems frequently gave "
        "poor or misleading feedback when a setting was wrong. Their recommendation — pinpoint the offending "
        "parameter and explain it in the user's terms — motivated Studio's error model, in which every failure "
        "is rendered as “what happened → why → what to do next” and mapped back to the exact field "
        "on screen.",
        "In [3], Xu et al. show that system software is frequently over-configured: most users touch only a small "
        "subset of the available knobs, and the sheer number of options is itself a source of errors. Studio "
        "responds by curating settings into grouped, documented schemas (56 Hyprland settings in eight categories, "
        "twelve mako behaviour settings) with sensible bounds, each carrying a one-line human description, instead "
        "of exposing every raw key.",
        "Shneiderman's guidelines for interface design [4] include “permit easy reversal of actions” and "
        "“offer informative feedback”; direct manipulation with immediate, visible results lowers the "
        "anxiety of exploration. Studio applies these principles through live preview (every Look & Feel adjustment "
        "is pushed to the compositor with `hyprctl keyword` before it is saved), a ten-second confirm-or-revert "
        "countdown for risky changes, and one-key undo backed by a git history.",
        "For theme generation, Heckbert's median-cut algorithm [5] recursively splits the colour space along its "
        "longest axis to find a small representative palette of an image. Studio's palette extraction ports the "
        "median-cut approach (with Normal, Muted and Material modes inspired by the MIT-licensed Aether project [15]) "
        "and assigns the extracted colours to Omarchy's 21-slot `colors.toml`. Contrast between foreground, accent "
        "and background is checked against the WCAG 2.1 contrast-ratio formula [6] so that generated themes remain "
        "readable.",
    ]:
        R.para(b)
    R.sub("2.1 Existing Tools in the Omarchy Ecosystem")
    R.para("The Omarchy community has produced several tools, each solving one slice of the problem. Table 1 "
           "summarises them; a detailed comparison with Studio appears in Chapter 7.")
    R.table(["Tool", "Approach", "Limitation"], [
        ["Omarchy menu [7]", "Bash + Walker menus; theme pick/install, then “open this file in nvim”",
         "No structured editing, preview or undo beyond themes"],
        ["Aether [15]", "Go + Wails GUI; wallpaper → palette extraction, wallhaven browser, presets",
         "Colours only — no keybinds, animations, bar or notifications"],
        ["Omarchist [17]", "Rust + GPUI visual theme designer", "Early stage; no keybinds, animations or notifications"],
        ["hyprmon [18]", "Go TUI for monitor layout", "Monitors only"],
        ["A La Carchy [14]", "Single 10.8k-line bash script: debloat + many toggles",
         "Edits some vendored files; tar.gz backup instead of verified rollback; no tests"],
    ], [1.5, 2.6, 2.4], size=11, caption="Existing customisation tools for Omarchy", bold_first_col=True)
    R.para("None of these tools combines the visual half of ricing (colours, wallpapers) with the behavioural half "
           "(keybinds, animations, bar, notifications, displays), and none offers verified apply with automatic "
           "rollback. That gap defines Omarchy Studio.")


def ch_architecture(R):
    R.chapter("3. Proposed System Architecture")
    R.figure(os.path.join(ASSETS, "diagrams", "architecture.png"), 6.5, "Layered architecture of Omarchy Studio")
    R.sub("It Includes", size=18, before=6)
    R.bullets(["Presentation Layer (TUI Cockpit, CLI Twin, Omarchy integration)",
               "Core Engine (studio-core: apply pipeline, domain modules, palette lab)",
               "Config Engine (lossless parsers and writers)",
               "Omarchy Adapter & System Interfaces",
               "Persistence Layer (snapshot store and configuration files)"], numbered=True, after=2)

    R.sub("Presentation Layer (Frontends) - Detailed Analysis", before=14)
    R.para("The Presentation Layer is the only part of Studio that talks to the user. It contains two frontends "
           "over the same engine, and the rule that keeps them honest is that **studio-core never prints, never "
           "draws and never prompts** — it returns typed results, and the frontends render them. This is also "
           "what keeps a future graphical frontend cheap to build.")
    R.sub("TUI Cockpit", size=14)
    R.para("The TUI is a full-screen Ratatui application with a module rail on the left, the active screen on the "
           "right and a key bar at the bottom. Studio themes itself from the active Omarchy theme, so its panels "
           "and highlights re-tint with every theme switch. Images (theme wallpapers, wallhaven thumbnails) are drawn "
           "inside the terminal with the kitty or sixel graphics protocols, falling back to half-block characters.")
    R.sub("User Workflow:", size=13.5, before=4)
    R.bullets([
        "**Launch**: from the Omarchy menu (Style → Studio), a keybinding or a terminal; Studio opens as a "
        "floating window like Omarchy's own TUIs.",
        "**Onboarding**: on first run a welcome card offers three optional steps — track every change, wire into "
        "Omarchy (menu entry and update-survival hooks) and theme terminal tools.",
        "**Navigate**: `Tab`/`Shift+Tab`, `1`–`9`, arrow keys or a mouse click moves between modules; `/` opens "
        "a fuzzy command palette; `?` shows context help generated from each screen's key map.",
        "**Edit with live preview**: values are adjusted with `h`/`l`; compositor settings are pushed live, themes "
        "preview their wallpaper and palette, keybinds are captured by pressing the chord itself.",
        "**Save**: `s` sends the pending edits through the apply pipeline; the result is confirmed with "
        "“Applied — u to undo”.",
        "**Undo / restore**: `u` undoes the last change; the Snapshots screen shows every change with a coloured "
        "diff and restores any point in time.",
    ], numbered=True)
    R.sub("CLI Twin", size=14)
    R.para("Everything the TUI does is scriptable through a clap-based command tree with 30 verb groups (`theme`, "
           "`wallpaper`, `keybind`, `looknfeel`, `animations`, `waybar`, `monitor`, `snapshot`, `rice`, `doctor`, "
           "…). Each mutating command prints its undo hint, e.g. `looknfeel general.gaps_in=8 · undo with "
           "omarchy-studio snapshot undo`. Because both frontends call the same core functions, a change made in the "
           "CLI is undoable from the TUI and vice versa.")

    R.title("Core Engine", size=20, before=10, after=10)
    R.para("The Core Engine (`studio-core`) contains all domain logic — 31 module files covering themes, "
           "wallpapers, keybinds, look & feel, animations, Waybar, mako, SwayOSD, lock & idle, monitors, power, "
           "battery, apps, tweaks, rice bundles, theme sync, the Omarchy 4 shell and the self-updater — and the "
           "machinery they all share.")
    R.para("**Settings schema.** Every editable field is a `Setting` with an identifier, a target (file, dialect "
           "and key path), a widget (stepper, toggle, colour, select, chord or text), a one-line human explanation, "
           "an optional live-apply action such as `HyprctlKeyword(\"decoration:blur:size\")`, dependency gates and "
           "version gates. The TUI renders forms from the schema and the CLI resolves `get`/`set` paths against it, "
           "so adding a setting means adding data, not UI code.")
    R.para("**The apply pipeline.** Every mutation is described by an `ApplyPlan` containing the file edits, the "
           "ordered reload steps (`hyprctl reload`, `omarchy-theme-refresh`, restart a component, reload mako), the "
           "verifications to run afterwards and a risk level. The pipeline executes "
           "**plan → drift check → snapshot (pre) → atomic write → reload → verify → "
           "commit or rollback**. Verifications include `hyprctl configerrors` being empty, a process (such as "
           "Waybar) still being alive after a grace period, and arbitrary commands exiting successfully. Risky "
           "changes arm a ten-second confirm-or-revert countdown, the same pattern operating systems use for display "
           "settings.")
    R.para("**Error model.** Errors form a typed taxonomy — missing dependency, parse failure with line and "
           "hint, verification failure (with whether rollback succeeded), Omarchy version mismatch, I/O and external "
           "command failures. The frontend contract is that every error renders as *what happened → why → "
           "what to do next*; a missing dependency is never an error at all but a visible, disabled feature carrying "
           "the exact install command.")
    R.para("**Concurrency.** The core is single-threaded; worker threads are used only for wallhaven HTTP requests "
           "and thumbnail decoding, image previews, palette extraction and command execution with timeouts, "
           "communicating with the UI over `std::sync::mpsc` channels. No async runtime is needed because every "
           "system command Studio runs is a fast local process.")

    R.title("Config Engine (Lossless Parsers)", size=20, before=10, after=10)
    R.para("The Config Engine (`configfs`) is the trust core of the system. Its guiding principle is to parse "
           "fully only what Studio must understand and, for everything else, to own a clearly marked region and treat "
           "the rest of the file as opaque text. Table 2 lists the strategy per dialect.")
    R.table(["Dialect", "Files", "Strategy"], [
        ["hyprlang", "~/.config/hypr/*.conf (Omarchy 3)", "Line-level CST; byte-identical round-trip"],
        ["Lua", "~/.config/hypr/*.lua (Omarchy 4)", "Emit-only managed block of hl.* calls; values read back via hyprctl"],
        ["TOML", "colors.toml, swayosd config, Studio config", "Lossless document edit with toml_edit"],
        ["JSONC", "waybar/config.jsonc, omarchy-menu.jsonc", "Span-splice editor preserving comments and order"],
        ["INI", "themed/mako.ini.tpl", "Studio-owned file, simple line model"],
        ["CSS", "waybar/style.css, swayosd/style.css", "Managed blocks only — never a full CSS parse"],
    ], [1.0, 2.6, 2.9], size=11, caption="Configuration dialects and editing strategies", bold_first_col=True)
    R.para("**hyprlang line CST.** A Hyprland file is parsed into a lossless list of lines — blank, comment, "
           "assignment (with category path and trailing comment), category open/close, `source`, variable or unknown "
           "— where unknown lines are never dropped or touched. The invariant `parse(s).to_string() == s` is "
           "tested against real configuration files.")
    R.para("**Lua emission for Omarchy 4.** Omarchy 4 moved Hyprland's configuration to Lua. Studio emits a "
           "deterministic managed block of `hl.config{}`, `hl.monitor{}`, `hl.curve`/`hl.animation` and `o.bind` "
           "calls at the end of the user's file (later calls deep-merge over earlier ones), never parses arbitrary "
           "Lua, and reads effective values back through `hyprctl -j --batch getoption`. Every emitted block is "
           "syntax-checked by the real `luac` in the test suite.")
    R.para("**Managed blocks and atomic writes.** Shared files receive a marked block such as "
           "`/* >>> omarchy-studio:<section> … >>> */` that is fully regenerated on each apply, while everything "
           "outside it is untouchable. Writes are hash-guarded (the write is refused if the file changed since it was "
           "parsed), go to a temporary file, are flushed with `fsync` and then atomically `rename`d into place. "
           "Symlinked theme files are never written through.")

    R.title("Safety & Verification Layer", size=20, before=10, after=10)
    R.para("Safety in Studio is not a single component but a set of guarantees enforced at the boundaries of every "
           "layer. Together they ensure that an experiment can never leave the user with a broken desktop.")
    R.bullets([
        "**Verified apply with automatic rollback** — after every reload the pipeline checks "
        "`hyprctl configerrors`; on failure it restores the pre-snapshot files, deletes files the apply created and "
        "re-runs the reload steps.",
        "**Waybar crash watchdog** — a bar configuration is applied, Waybar is restarted and, if the process "
        "dies within the grace period, the previous configuration is restored automatically.",
        "**Drift detection** — if a managed file was hand-edited since Studio's last snapshot, a `drift` "
        "snapshot is recorded first, so the user's own edit is preserved in history too.",
        "**Never fight Omarchy** — Studio writes only user-owned files and sanctioned extension surfaces "
        "(`themed/*.tpl`, hooks, menu extensions); a CI check fails the build if any code outside the Omarchy "
        "adapter names Omarchy's vendored path.",
        "**Version awareness** — the adapter probes the Omarchy and Hyprland versions and the configuration "
        "dialect; untested versions produce a warning but never a refusal.",
        "**Privileged writes are previewed** — the AC/battery power rule and persistent battery thresholds show "
        "the exact root write for approval before anything runs; app removal shows the full `pacman -Rs` cascade "
        "and never bypasses pacman's refusal to break dependencies.",
    ])

    R.title("Persistence Layer — Snapshot Store", size=20, before=10, after=10)
    R.para("The snapshot store is a plain git repository at `~/.local/state/omarchy-studio/history/`. It mirrors "
           "every managed file under `files/` at its absolute path and keeps a `manifest.toml` recording which "
           "module claimed each file. Each apply produces a `pre:` commit before the write and a `post:` commit "
           "after successful verification, with the change summary as the subject (for example `looknfeel "
           "general.gaps_in=8`) and optional `key: value` trailers. Hand-edits are captured as `drift:` commits, "
           "the first state as `genesis:` and every undo as a `restore:` commit — history is append-only, so "
           "an undo can itself be undone.")
    R.para("Using the system `git` binary rather than a library keeps the executable small, and git is always "
           "present on Omarchy. The same store powers three user-facing features: one-key undo, the Snapshots "
           "timeline (with per-commit coloured diffs and whole-tree or per-file restore), and **rice bundles** "
           "— `rice export` archives every file Studio has written, grouped by module, and `rice import` "
           "replays each file through the full apply pipeline on another machine instead of untarring over a live "
           "desktop.")

    R.title("Theme & Palette Pipeline", size=20, before=10, after=10)
    R.para("The theme-from-wallpaper wizard turns any image into a complete, standard Omarchy theme. The image is "
           "sampled and quantised with median cut into a representative palette; the selected mode (Normal, Muted or "
           "Material) and light/dark bias shape the result; an accent is chosen and checked for WCAG contrast against "
           "the background; and the colours are written to all 21 keys of `colors.toml` together with the wallpaper "
           "and a generated `preview.png`. The wizard is reachable from any wallpaper (`t`), from wallhaven search "
           "results and from the CLI (`theme new --from-image`, `theme new --from-current-wallpaper`). The community "
           "browser complements it with 114 published themes whose wallpaper and palette are previewed *before* "
           "anything is installed.")

    R.title("Omarchy 4 “Quattro” Adapter", size=20, before=10, after=10)
    R.para("Omarchy 4 changed the platform underneath Studio: Hyprland configuration moved from hyprlang to Lua, "
           "the menu moved from a bash script to a Quickshell plugin reading a JSONC file, and Waybar, mako, SwayOSD "
           "and hypridle were replaced by the Quickshell-based `omarchy-shell`. Studio handles both generations from "
           "one binary: the dialect is probed from the running compositor (`hyprctl systeminfo` → "
           "`configProvider`), every Hyprland writer renders hyprlang or Lua accordingly, retired components are "
           "detected and their screens explain why they are unavailable, and a new shell module edits the Omarchy 4 "
           "bar layout, plugins, idle timers and appearance by wrapping Omarchy's own `omarchy bar` and "
           "`omarchy plugin` commands.")


def ch_techstack(R):
    R.chapter("4. Technology Stack")
    R.para("The proposed system is a native, dependency-light application optimised for safety, speed and "
           "distribution as a single binary. The technology stack is divided into four layers.")
    R.sub("Language & Frontends")
    R.para("Studio is written in **Rust (edition 2021, toolchain 1.96)**, chosen for memory safety, strong typing "
           "of the configuration model and a single statically-linked-feeling binary that users download once. The TUI "
           "is built on **Ratatui 0.29** with its re-exported **crossterm** backend, and terminal image previews use "
           "**ratatui-image** (kitty, sixel and half-block protocols). The CLI uses **clap 4** for the command tree, "
           "help text and did-you-mean suggestions.")
    R.sub("Configuration & Data")
    R.para("Lossless TOML editing uses **toml_edit**; JSON from `hyprctl -j` and the wallhaven API is decoded with "
           "**serde / serde_json**. The hyprlang CST, the JSONC span-splice editor, the INI model and the Lua emitter "
           "are hand-written so that every byte of the user's files can be preserved. The **image** crate decodes "
           "PNG, JPEG, GIF and WebP for palette extraction and previews, and **ureq** (with rustls, no OpenSSL) makes "
           "blocking HTTP requests on worker threads.")
    R.sub("System Integration")
    R.para("Studio drives the desktop through its own tools: **hyprctl** (keywords, reload, configerrors, binds, "
           "monitors), Omarchy's **omarchy-theme-set / omarchy-theme-refresh / omarchy-restart-*** primitives, "
           "**makoctl**, **swayosd-client**, **pacman** and **powerprofilesctl**. Snapshots are stored with the "
           "system **git** binary.")
    R.sub("Quality, CI & Distribution")
    R.para("**GitHub Actions** runs rustfmt, clippy with warnings denied, the full test suite, an adapter-boundary "
           "check, a crates.io packaging check and tmux-driven end-to-end journeys on every push. Releases are built "
           "with link-time optimisation and distributed as a binary through a checksummed `curl | sh` installer, an "
           "**AUR** package and a built-in self-updater that defers to pacman for packaged installs.")
    R.table(["Layer", "Technology", "Purpose"], [
        ["Language", "Rust 2021 (toolchain 1.96)", "Memory-safe, typed engine and single binary"],
        ["TUI", "Ratatui 0.29 + crossterm", "Full-screen keyboard-driven interface"],
        ["Images", "ratatui-image 8, image 0.25", "In-terminal previews, palette extraction"],
        ["CLI", "clap 4", "Command tree, help, suggestions"],
        ["Config", "toml_edit 0.22, serde_json, hand-written CST", "Lossless, comment-preserving edits"],
        ["Network", "ureq 2 (rustls)", "wallhaven API, GitHub releases & themes"],
        ["History", "git", "Snapshot store, diffs, restore"],
        ["Desktop", "hyprctl, omarchy-* scripts, makoctl, pacman", "Live preview, reload, verification"],
        ["CI / Release", "GitHub Actions, tmux, AUR", "Lint, test, e2e, packaging"],
    ], [1.3, 2.6, 2.6], size=11, caption="Technology stack summary", bold_first_col=True)


def ch_diagrams(R):
    R.chapter("5. Diagrams")
    R.sub("Flow Diagram", size=20)
    R.figure(os.path.join(ASSETS, "diagrams", "flow.png"), 5.4, "User flow through Omarchy Studio")
    R.page_break()
    R.sub("Use Case Diagram", size=20)
    R.figure(os.path.join(ASSETS, "diagrams", "usecase.png"), 6.5, "Use case diagram")
    R.page_break()
    R.sub("Data Modelling Diagram", size=20)
    R.figure(os.path.join(ASSETS, "diagrams", "snapshot_model.png"), 6.5,
             "Data model of the git-backed snapshot store")
    R.page_break()
    R.sub("Data Flow Diagram", size=20)
    R.figure(os.path.join(ASSETS, "diagrams", "dfd.png"), 6.5, "Data flow diagram — Level 0 and Level 1")
    R.page_break()
    R.sub("Activity Diagram — Apply Pipeline", size=20)
    R.figure(os.path.join(ASSETS, "diagrams", "pipeline.png"), 3.0, "Activity diagram of the apply pipeline")


def ch_results(R):
    R.chapter("6. Results and Screenshots")
    R.sub("6.1 Test Results", size=14, before=0)
    R.para("Because Studio edits a live desktop, test effort was concentrated on the parts users must be able to "
           "trust. The full workspace suite was run for this report with `cargo test --workspace`:")
    R.table(["Test suite", "Tests", "Result"], [
        ["studio-core unit tests (parsers, pipeline, snapshots, modules)", "388", "387 passed, 1 ignored"],
        ["omarchy-studio unit tests (TUI screens, CLI)", "79", "79 passed"],
        ["Golden round-trip tests (real config files)", "10", "10 passed"],
        ["tmux end-to-end journeys (tools/e2e.sh)", "—", "All checks passed"],
        ["Total automated tests", "477", "0 failures"],
    ], [3.6, 0.9, 2.0], size=10.5, align="lcc", fills=lambda r, c, v: "E3F6EC" if c == 2 else None)
    R.para("The end-to-end journey launches the real release binary in a tmux pseudo-terminal against a fixture "
           "Omarchy installation and asserts on the captured screen: the first-run on-ramp, rail navigation, the "
           "panel following the selection, the help and search overlays and a clean exit. A separate rollback "
           "journey injects a fake `hyprctl configerrors` failure and proves that a rejected change — including a "
           "rejected *first-ever* write — leaves the user's files exactly as they were.")
    R.sub("6.2 Codebase and Build Metrics", size=14)
    R.table(["Metric", "Value"], [
        ["Rust source (both crates, incl. tests)", "≈ 43,700 lines"],
        ["studio-core engine / omarchy-studio binary", "≈ 25,500 / ≈ 17,900 lines"],
        ["Domain module files / TUI screen files", "31 / 23"],
        ["CLI verb groups", "30"],
        ["Release binary size (LTO, debuginfo stripped)", "≈ 7.3 MB (public release v0.9.3)"],
    ], [4.0, 2.5], size=11.5, align="lc")
    R.sub("6.3 Feature Coverage", size=14)
    R.bullets([
        "**56** Hyprland look-and-feel settings in 8 categories, each with live preview; **6** look-and-feel presets "
        "and **6** animation presets.",
        "**41** Hyprland dispatchers given plain-language names in the keybind editor, with source attribution "
        "across every sourced binding file.",
        "**114** community themes browsable with a pre-install wallpaper and palette preview.",
        "**30** curated packages and web apps in the safe-removal catalogue with a dependency cascade preview.",
        "Verified on **Omarchy 3.8 and 4.0** with **Hyprland 0.55 and 0.56**; both hyprlang and Lua configuration "
        "dialects are written from one binary.",
    ])
    R.page_break()
    R.sub("6.4 Screenshots", size=16, before=0)
    shots = [
        ("01_themes.png", "Themes Screen",
         "The home screen: every installed theme with its palette strip, light-mode markers and a live preview of the "
         "selected theme's wallpaper and a mock terminal drawn in its colours."),
        ("02_wallpapers.png", "Wallpaper Browser",
         "Wallpapers from all four sources Omarchy merges, with an in-terminal image preview; `w` opens wallhaven "
         "search and `t` crafts a theme from the selected image."),
        ("03_wizard.png", "Theme-from-Wallpaper Wizard",
         "Median-cut extraction turns the image into a full 21-colour palette; modes and light/dark bias can be "
         "switched with live re-extraction before the theme is saved."),
        ("04_keybinds.png", "Keybinds Screen",
         "The effective keymap read from the compositor, with human-readable actions and the file each binding "
         "comes from; pressing `/` captures a new chord and conflicts are reported before saving."),
        ("05_looknfeel.png", "Look & Feel Screen",
         "Hyprland settings grouped into tabs (Windows, Layout, Behavior, Corners & opacity, Blur, Shadow, Input, "
         "Touchpad); every adjustment is previewed live and `s` saves through the pipeline."),
        ("06_animations.png", "Animations Screen",
         "Six hand-tuned animation feels from instant to bouncy, applied as a managed block with a live reload."),
        ("07_snapshots.png", "Snapshots Timeline",
         "Every pre/post snapshot Studio has recorded, with the coloured diff of the selected change; any point "
         "can be restored, and the restore is itself recorded."),
        ("08_monitors.png", "Monitors Screen",
         "A proportional map of the connected displays with resolution, refresh rate, scale and effective size; "
         "placement mode arranges one screen against another and warns about overlaps."),
        ("09_apps.png", "Apps & Services",
         "Safe removal of bundled apps and web apps: a `pacman -Rs` cascade preview, service handling and a "
         "restore log, with no bulk-confirm bypass."),
        ("10_tweaks.png", "Quick Tweaks",
         "One-key reversible toggles; each tweak owns a uniquely named managed block and reports its live state."),
        ("11_integrations.png", "Integrations Screen",
         "Dependency health with the exact install command for anything missing, plus detection of companion tools "
         "such as Aether, Omarchist, matugen and hyprmon."),
        ("13_doctor.png", "Doctor",
         "One health view: system facts, capability probes, hook status and a drift report showing whether "
         "tracked files still match the last snapshot."),
        ("14_onboarding.png", "First-run On-ramp",
         "A fresh install opens on a welcome card with three optional, reversible set-up steps."),
        ("15_cli.png", "CLI Twin",
         "The same engine from the command line: monitor listing, a look-and-feel change that prints its undo hint, "
         "the snapshot log and the command list."),
    ]
    for i, (f, t, d) in enumerate(shots, 1):
        R.sub(f"{i}. {t}", size=14, before=6)
        R.image(os.path.join(ASSETS, "screens", f), 5.6, after=4)
        R.para(d, size=11.5, align="center", after=10, spacing=1.2)


def ch_comparison(R):
    R.chapter("7. Comparative Analysis")
    R.sub("1. FEATURE COVERAGE MATRIX", size=13, before=0)
    Y, N, P = "Yes", "No", "Partial"
    rows = [
        ["Theme browse & apply", Y, Y, P, Y, N, Y],
        ["Theme from wallpaper", Y, Y, Y, N, N, N],
        ["Keybind editing", Y, Y, N, N, N, P],
        ["Hyprland look & feel", Y, Y, N, P, N, P],
        ["Animations", Y, N, N, N, N, P],
        ["Status bar layout", Y, P, N, P, N, N],
        ["Notifications / OSD / idle", Y, N, N, N, N, N],
        ["Monitors", Y, Y, N, N, Y, P],
        ["App removal", Y, Y, N, N, N, P],
        ["ASUS ROG hardware", N, Y, N, N, N, N],
    ]
    colors = {Y: "D8F3DC", N: "FADBD8", P: "FFF1C1"}
    R.table(["Capability", "Omarchy Studio", "A La Carchy", "Aether", "Omarchist", "hyprmon", "Omarchy menu"],
            rows, [1.65, 0.85, 0.8, 0.7, 0.8, 0.75, 0.85], size=10, align="lcccccc", bold_first_col=True,
            fills=lambda r, c, v: colors.get(v) if c > 0 else None,
            caption="Coverage of customisation areas (from each project's documentation)")
    R.para("“Partial” for the Omarchy menu means it opens the relevant file in a text editor; for A La "
           "Carchy's bar it means a few on/off toggles rather than a layout editor. A La Carchy is honestly the better "
           "tool for ASUS ROG laptops today.", size=11.5, spacing=1.25)
    R.page_break()
    R.sub("2. SAFETY & ENGINEERING COMPARISON", size=13, before=0)
    R.table(["Aspect", "Manual editing", "A La Carchy", "Omarchy Studio"], [
        ["Form", "Text editor + docs", "One 10.8k-line bash script", "Rust engine crate + TUI/CLI binary"],
        ["Preview", "None", "None", "Live (hyprctl keyword, image previews)"],
        ["Undo", "None (manual .bak)", "tar.gz backup of 8 dirs", "git timeline, diff, any-point restore"],
        ["Apply safety", "None", "Confirmation prompts", "Verify → automatic rollback, bar watchdog"],
        ["Hand-edits", "—", "Edited in place", "Preserved byte-for-byte"],
        ["Survives omarchy-update", "Only user files", "Mostly (edits some vendored files)", "Always"],
        ["Omarchy 4 support", "—", "Waiting for v4", "Lua dialect + shell adapter"],
        ["Automated tests", "—", "None", "477 tests + e2e in CI"],
    ], [1.45, 1.4, 1.75, 1.9], size=9.5, bold_first_col=True,
        fills=lambda r, c, v: "E3F6EC" if c == 3 else None, caption="Safety and engineering comparison")
    R.sub("3. QUALITATIVE ANALYSIS (STRENGTHS vs. WEAKNESSES)", size=13, before=6)
    R.table(["Tool", "Key Strengths", "Key Weaknesses"], [
        ["Omarchy Studio", "Covers visual and behavioural customisation in one place; live preview; verified "
         "apply with rollback; git-backed undo; CLI parity; Omarchy 3 and 4.",
         "Terminal-only (no GUI yet); Omarchy-specific; alpha status; no ROG hardware controls."],
        ["A La Carchy", "Very broad feature set incl. ASUS ROG; zero install (curl | bash).",
         "No verified rollback; edits some vendored files; no tests; paused for Omarchy 4."],
        ["Aether", "Excellent palette extraction, filters and presets in a polished GUI.",
         "Colour theming only; heavier GUI stack."],
        ["Omarchist", "Visual theme designer with native GPUI rendering.",
         "Early development; no keybinds, animations or notifications."],
        ["hyprmon", "Focused, capable monitor layout TUI.", "Single purpose."],
        ["Manual editing", "Unlimited flexibility; no extra tools.",
         "Five dialects, no discoverability, no preview, no undo, easy to break."],
    ], [1.3, 2.7, 2.5], size=10, bold_first_col=True)


def ch_conclusion(R):
    R.chapter("8. Conclusion")
    for b in [
        "The development of Omarchy Studio demonstrates that deep desktop customisation does not have to be "
        "risky or require fluency in a stack of configuration languages. By putting a schema-driven terminal "
        "interface and a matching CLI on top of a single verified apply pipeline, we turned the most error-prone "
        "activity on a Hyprland desktop — hand-editing configuration files — into a guided, previewable and "
        "fully reversible workflow.",
        "The project's central design decision was to treat trust as the product. Lossless, comment-preserving "
        "parsers ensure that the user's own edits survive byte-for-byte; the never-fight-Omarchy rule ensures "
        "that system updates and Studio's changes cannot clobber each other; verification after every reload with "
        "automatic rollback ensures that a bad change cannot strand the user; and a git-backed snapshot store makes "
        "every change in the system's history inspectable and undoable. These guarantees are backed by 477 "
        "automated tests and end-to-end terminal journeys that run on every commit.",
        "Studio also proved adaptable: when Omarchy 4 replaced the Hyprland configuration language, the menu system "
        "and four desktop components, the layered architecture allowed the change to be absorbed in the adapter and "
        "config engine while the modules, pipeline and snapshot store remained unchanged. The tool has been released "
        "publicly as open-source software, packaged for the AUR and used by members of the Omarchy community.",
        "Overall, the project shows how established ideas from configuration-error research, interface design and "
        "version control can be combined into a practical tool that makes a modern Linux desktop approachable for "
        "newcomers while remaining fast and scriptable for experienced users.",
    ]:
        R.para(b, spacing=1.5)


def ch_future(R):
    R.chapter("9. Future Scope")
    for b in [
        "While the current system covers the full customisation journey on Omarchy, several avenues remain for "
        "future enhancement. The most significant is a **graphical frontend**: because `studio-core` never prints, "
        "draws or prompts, a GUI (or a Walker-native launcher flow) can be built over the same engine without "
        "duplicating any domain logic, widening the audience to users who prefer the mouse.",
        "On the platform side, a **generic-Hyprland mode** would decouple Studio from Omarchy's paths so that any "
        "Hyprland user could benefit from verified, undoable configuration. Deeper coverage of the Omarchy 4 "
        "Quickshell shell, per-module bar colours and AC/battery idle profiles are planned, together with an "
        "optional basic tier for **ASUS ROG** hardware (performance profiles, battery limits and keyboard "
        "brightness) behind capability gates.",
        "The theming engine can be extended with the remaining palette-extraction modes (monochromatic, analogous, "
        "pastel, colourful, bright), single-theme export and sharing, and **theme sync** for more terminal tools "
        "beyond fzf and lazygit. Finally, the v1.0 milestone includes a written quick-start and tutorial, a "
        "“which file did that touch?” guide and a documented manual test protocol run on real hardware "
        "before every release — turning Studio from a working alpha into a tool a stranger can install from the "
        "AUR and use from start to finish.",
    ]:
        R.para(b, spacing=1.5)


REFS = [
    "T. Xu and Y. Zhou, “Systems Approaches to Tackling Configuration Errors: A Survey,” ACM Computing "
    "Surveys, vol. 47, no. 4, Art. 70, 2015.",
    "Z. Yin, X. Ma, J. Zheng, Y. Zhou, L. N. Bairavasundaram and S. Pasupathy, “An Empirical Study on "
    "Configuration Errors in Commercial and Open Source Systems,” in Proc. 23rd ACM Symposium on Operating "
    "Systems Principles (SOSP), 2011, pp. 159–172.",
    "T. Xu, L. Jin, X. Fan, Y. Zhou, S. Pasupathy and R. Talwadker, “Hey, You Have Given Me Too Many Knobs!: "
    "Understanding and Dealing with Over-Designed Configuration in System Software,” in Proc. ESEC/FSE, 2015, "
    "pp. 307–319.",
    "B. Shneiderman, C. Plaisant, M. Cohen, S. Jacobs, N. Elmqvist and N. Diakopoulos, Designing the User "
    "Interface: Strategies for Effective Human-Computer Interaction, 6th ed. Pearson, 2016.",
    "P. Heckbert, “Color Image Quantization for Frame Buffer Display,” Computer Graphics (Proc. SIGGRAPH "
    "’82), vol. 16, no. 3, pp. 297–307, 1982.",
    "W3C, “Web Content Accessibility Guidelines (WCAG) 2.1,” W3C Recommendation, 2018. [Online]. "
    "Available: https://www.w3.org/TR/WCAG21/",
    "Omarchy. [Online]. Available: https://omarchy.org. [Accessed: Oct. 2026].",
    "Hyprland Wiki. [Online]. Available: https://wiki.hypr.land. [Accessed: Oct. 2026].",
    "Ratatui — Rust library for terminal user interfaces. [Online]. Available: https://ratatui.rs. "
    "[Accessed: Oct. 2026].",
    "S. Klabnik and C. Nichols, The Rust Programming Language. [Online]. Available: "
    "https://doc.rust-lang.org/book/. [Accessed: Oct. 2026].",
    "toml_edit — format-preserving TOML parser. [Online]. Available: https://docs.rs/toml_edit. "
    "[Accessed: Oct. 2026].",
    "S. Chacon and B. Straub, Pro Git, 2nd ed. Apress. [Online]. Available: https://git-scm.com/book. "
    "[Accessed: Oct. 2026].",
    "wallhaven API v1 documentation. [Online]. Available: https://wallhaven.cc/help/api. [Accessed: Oct. 2026].",
    "D. Coffey, “A La Carchy.” [Online]. Available: https://github.com/DanielCoffey1/a-la-carchy. "
    "[Accessed: Oct. 2026].",
    "bjarneo, “Aether.” [Online]. Available: https://github.com/bjarneo/aether. [Accessed: Oct. 2026].",
    "Omarchy Studio source repository. [Online]. Available: https://github.com/arino08/omarchy-studio. "
    "[Accessed: Oct. 2026].",
    "tahayvr, “Omarchist.” [Online]. Available: https://github.com/tahayvr/omarchist. [Accessed: Oct. 2026].",
    "erans, “hyprmon.” [Online]. Available: https://github.com/erans/hyprmon. [Accessed: Oct. 2026].",
]


def ch_references(R):
    R.chapter("10. References")
    for i, r in enumerate(REFS, 1):
        p = R.para(f"[{i}]\t{r}", size=12.5, align="left", after=10, spacing=1.3, indent=0.45, first=-0.45)
        p.paragraph_format.tab_stops.add_tab_stop(Inches(0.45))


# ── build ────────────────────────────────────────────────────────────────────
def build_docx(student, path, toc_pages=None):
    R = Report(student, toc_pages)
    cover(R)
    certificate(R)
    approval(R)
    declaration(R)
    acknowledgement(R)
    student_info(R)
    abstract(R)
    toc(R)
    R.start_numbered_section()
    ch_introduction(R)
    ch_literature(R)
    ch_architecture(R)
    ch_techstack(R)
    ch_diagrams(R)
    ch_results(R)
    ch_comparison(R)
    ch_conclusion(R)
    ch_future(R)
    ch_references(R)
    _drop_break_after_section(R.doc)
    R.save(path)


def _drop_break_after_section(doc):
    """The numbered section starts on a new page already; remove the page-break
    paragraph that chapter() put right after the section break."""
    body = doc.element.body
    for p in body.iterchildren(qn("w:p")):
        ppr = p.find(qn("w:pPr"))
        if ppr is not None and ppr.find(qn("w:sectPr")) is not None:
            nxt = p.getnext()
            if nxt is not None and nxt.tag == qn("w:p") and nxt.findall(".//" + qn("w:br")):
                body.remove(nxt)
            break


def to_pdf(docx_path, outdir):
    subprocess.run(["soffice", "--headless", "--convert-to", "pdf", "--outdir", outdir, docx_path],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return os.path.join(outdir, os.path.splitext(os.path.basename(docx_path))[0] + ".pdf")


def measure_toc(pdf_path):
    pages = {}
    reader = PdfReader(pdf_path)
    for page in reader.pages:
        text = page.extract_text() or ""
        m = re.search(r"Page\s+(\d+)\s*$", text.strip())
        if not m:
            continue
        head = text.strip().splitlines()[0].strip()
        for i, c in enumerate(CHAPTERS, 1):
            if c not in pages and re.match(rf"^{i}\.\s*{re.escape(c)}", head):
                pages[c] = int(m.group(1))
    return pages


def main():
    os.makedirs(OUT, exist_ok=True)
    wanted = sys.argv[1:]
    for name, roll in TEAM:
        if wanted and roll not in wanted:
            continue
        base = f"Omarchy_Studio_ROSP_Report_{name.replace(' ', '_')}_{roll}"
        docx_path = os.path.join(OUT, base + ".docx")
        with tempfile.TemporaryDirectory() as tmp:
            build_docx((name, roll), docx_path)
            pages = measure_toc(to_pdf(docx_path, tmp))
        missing = [c for c in CHAPTERS if c not in pages]
        if missing:
            print(f"warning: could not locate {missing} in {base}")
        build_docx((name, roll), docx_path, pages)
        pdf = to_pdf(docx_path, OUT)
        print(f"{base}: {len(PdfReader(pdf).pages)} pages, toc={pages}")


if __name__ == "__main__":
    main()
