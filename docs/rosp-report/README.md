# Omarchy Studio — ROSP final-year project report

Four personalised copies of the report (one per team member), in the
department's report format. Every copy names the whole team; the cover
("Submitted by") and the declaration signature differ per student.

| Student | Roll No. | PDF | Editable |
|---|---|---|---|
| Aariz Sheikh | 231453 | [pdf](out/Omarchy_Studio_ROSP_Report_Aariz_Sheikh_231453.pdf) | [docx](out/Omarchy_Studio_ROSP_Report_Aariz_Sheikh_231453.docx) |
| Yusuf Mohd Aslam | 231460 | [pdf](out/Omarchy_Studio_ROSP_Report_Yusuf_Mohd_Aslam_231460.pdf) | [docx](out/Omarchy_Studio_ROSP_Report_Yusuf_Mohd_Aslam_231460.docx) |
| Samia Loladia | 231463 | [pdf](out/Omarchy_Studio_ROSP_Report_Samia_Loladia_231463.pdf) | [docx](out/Omarchy_Studio_ROSP_Report_Samia_Loladia_231463.docx) |
| Naved Rar | 231433 | [pdf](out/Omarchy_Studio_ROSP_Report_Naved_Rar_231433.pdf) | [docx](out/Omarchy_Studio_ROSP_Report_Naved_Rar_231433.docx) |

Guide and ROSP co-ordinator: Prof. Fardeen · Head of Department: Dr. Zainab Mirza.

## Regenerating

Names, guide, semester and academic year are constants at the top of
`build_report.py`. After editing them:

```bash
python3 build_report.py            # all four copies (needs python-docx, pypdf, LibreOffice)
python3 build_report.py 231453    # one copy
```

The table of contents is measured from a first rendering, so page numbers stay exact.

Diagrams are HTML/SVG/Mermaid sources in `diagrams/`, rendered with
`node diagrams/render.js` (Playwright + Chromium; set `MERMAID_JS` to a local
`mermaid.min.js`). Screenshots in `assets/screens/` are real captures of the
release binary (README media plus tmux captures against the e2e fixture).
