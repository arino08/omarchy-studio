"""Generates usecase.html (a hand-laid UML use-case diagram as inline SVG)."""
W, H = 1600, 1240
A = ["Browse & apply themes", "Craft theme from wallpaper", "Rebind keys", "Tune look & feel",
     "Apply animation preset", "Edit bar layout", "Configure notifications / OSD", "Set idle & lock",
     "Arrange monitors", "Manage power & battery", "Remove apps safely", "Undo / restore snapshot",
     "Export / import rice bundle", "Run doctor"]
B = ["Install community theme", "Search wallhaven", "Self-update", "Preview change live", "Apply change (pipeline)",
     "Record snapshot", "Verify reload", "Roll back automatically"]
ax, bx = 620, 1030
ay = {n: 120 + i * 77 for i, n in enumerate(A)}
by = {n: 150 + i * 136 for i, n in enumerate(B)}
rx, ry = 150, 30
out = []
def actor(x, y, label):
    out.append(f'<g stroke="#1f2937" stroke-width="3" fill="none"><circle cx="{x}" cy="{y-48}" r="17" fill="#cfe0ff"/>'
               f'<line x1="{x}" y1="{y-31}" x2="{x}" y2="{y+15}"/><line x1="{x-28}" y1="{y-14}" x2="{x+28}" y2="{y-14}"/>'
               f'<line x1="{x}" y1="{y+15}" x2="{x-22}" y2="{y+48}"/><line x1="{x}" y1="{y+15}" x2="{x+22}" y2="{y+48}"/></g>')
    for i, l in enumerate(label.split("\n")):
        out.append(f'<text x="{x}" y="{y+76+i*21}" text-anchor="middle" font-size="18" font-weight="700">{l}</text>')
def line(x1, y1, x2, y2):
    out.append(f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="#4b5563" stroke-width="1.6"/>')
def dashed(x1, y1, x2, y2, label):
    out.append(f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="#7c4ddb" stroke-width="1.6" stroke-dasharray="7 5" marker-end="url(#arr)"/>')
    mx, my = (x1 + x2) / 2, (y1 + y2) / 2
    out.append(f'<text x="{mx}" y="{my-5}" text-anchor="middle" font-size="14" fill="#5b2fb8" style="paint-order:stroke" stroke="#fff" stroke-width="5">«{label}»</text>')
def ellipse(x, y, text, fill):
    out.append(f'<ellipse cx="{x}" cy="{y}" rx="{rx}" ry="{ry}" fill="{fill}" stroke="#b8860b" stroke-width="2"/>')
    out.append(f'<text x="{x}" y="{y+6}" text-anchor="middle" font-size="16.5">{text.replace("&","&amp;")}</text>')
# boundary
out.append(f'<rect x="420" y="60" width="830" height="{H-90}" rx="14" fill="#f3f4f6" stroke="#6b7280" stroke-width="2.5"/>')
out.append('<text x="835" y="92" text-anchor="middle" font-size="21" font-weight="700">Omarchy Studio</text>')
# actors
ux, uy = 150, 520
actor(ux, uy, "Desktop User\n(Ricer)")
tx, ty = 150, 1010
actor(tx, ty, "Theme Author")
ox, oy = 1460, 900
actor(ox, oy, "Omarchy +\nHyprland")
ex, ey = 1460, 300
actor(ex, ey, "wallhaven /\nGitHub")
for n in A:
    line(ux + 30, uy - 14, ax - rx, ay[n])
line(tx + 30, ty - 14, ax - rx + 6, ay["Export / import rice bundle"] + 8)
# generalisation: a Theme Author is a Desktop User (hollow triangle on the parent)
out.append(f'<line x1="{tx}" y1="{ty-70}" x2="{ux}" y2="{uy+110}" stroke="#1f2937" stroke-width="2" marker-end="url(#gen)"/>')
out.append(f'<text x="{tx+12}" y="{(ty+uy)/2+20}" font-size="14" fill="#374151">generalises</text>')
for n in ["Preview change live", "Verify reload"]:
    line(ox - 30, oy - 14, bx + rx, by[n])
for n in ["Install community theme", "Search wallhaven", "Self-update"]:
    line(ex - 30, ey - 14, bx + rx, by[n])
inc = [("Browse & apply themes", "Install community theme", "extend"),
       ("Craft theme from wallpaper", "Search wallhaven", "extend"),
       ("Tune look & feel", "Preview change live", "include"),
       ("Rebind keys", "Apply change (pipeline)", "include"),
       ("Edit bar layout", "Apply change (pipeline)", "include"),
       ("Arrange monitors", "Apply change (pipeline)", "include"),
       ("Undo / restore snapshot", "Record snapshot", "include")]
for a, b, kind in inc:
    dashed(ax + rx, ay[a], bx - rx - 4, by[b], kind)
# within B column
def vdash(a, b, kind, off=0):
    x = bx + off
    out.append(f'<line x1="{x}" y1="{by[a]+ry}" x2="{x}" y2="{by[b]-ry-4}" stroke="#7c4ddb" stroke-width="1.6" stroke-dasharray="7 5" marker-end="url(#arr)"/>')
    out.append(f'<text x="{x+8}" y="{(by[a]+by[b])/2+5}" font-size="14" fill="#5b2fb8">«{kind}»</text>')
vdash("Apply change (pipeline)", "Record snapshot", "include")
vdash("Record snapshot", "Verify reload", "include")
out.append(f'<line x1="{bx}" y1="{by["Roll back automatically"]-ry}" x2="{bx}" y2="{by["Verify reload"]+ry+4}" stroke="#7c4ddb" stroke-width="1.6" stroke-dasharray="7 5" marker-end="url(#arr)"/>')
out.append(f'<text x="{bx+8}" y="{(by["Roll back automatically"]+by["Verify reload"])/2+5}" font-size="14" fill="#5b2fb8">«extend»</text>')
for n in A:
    ellipse(ax, ay[n], n, "#fff3b0")
for n in B:
    ellipse(bx, by[n], n, "#d9f0ff")
svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" font-family="DejaVu Sans, Arial, sans-serif" fill="#111827">'
       '<defs><marker id="arr" markerWidth="10" markerHeight="8" refX="9" refY="4" orient="auto"><path d="M0,0 L10,4 L0,8 z" fill="#7c4ddb"/></marker><marker id="gen" markerWidth="18" markerHeight="16" refX="17" refY="8" orient="auto"><path d="M1,1 L17,8 L1,15 z" fill="#fff" stroke="#1f2937" stroke-width="1.5"/></marker></defs>'
       + "".join(out) + '</svg>')
open("usecase.html", "w").write('<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="common.css"></head><body>'
    '<div id="diagram"><div class="title">Omarchy Studio — Use Case Diagram</div>' + svg + '</div></body></html>')
