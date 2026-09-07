#!/usr/bin/env python3
"""Expand Lucide strokes to filled paths for GTK 4.14+ symbolic rendering.

Development-only tool: pip install fonttools==4.64.0 skia-pathops==0.9.2
Usage: python normalize.py original.svg bundled.svg
Normal app builds consume the checked-in SVGs and do not need this tool.
"""

import sys
import xml.etree.ElementTree as ET
from pathlib import Path

import pathops
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.svgLib import SVGPath


def normalize(source: Path, destination: Path) -> None:
    root = ET.parse(source).getroot()
    assert root.attrib.get("viewBox") == "0 0 24 24"
    assert root.attrib.get("stroke-width") == "2"
    assert root.attrib.get("stroke-linecap") == "round"
    assert root.attrib.get("stroke-linejoin") == "round"
    combined = pathops.Path()
    for element in root:
        assert element.tag.rsplit("}", 1)[-1] in {"path", "rect", "circle", "ellipse", "line"}
        geometry = pathops.Path()
        fragment = ET.Element("svg", xmlns="http://www.w3.org/2000/svg")
        fragment.append(element)
        SVGPath.fromstring(ET.tostring(fragment)).draw(geometry.getPen())
        outline = pathops.Path(geometry)
        outline.stroke(2, pathops.LineCap.ROUND_CAP, pathops.LineJoin.ROUND_JOIN, 4)
        outline.convertConicsToQuads(0.001)
        if element.attrib.get("fill", root.attrib.get("fill")) != "none":
            outline = pathops.op(outline, geometry, pathops.PathOp.UNION)
        combined = pathops.op(combined, outline, pathops.PathOp.UNION)
    combined.convertConicsToQuads(0.001)
    pen = SVGPathPen(None, ntos=lambda number: f"{number:.4f}".rstrip("0").rstrip("."))
    combined.draw(pen)
    destination.write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">\n'
        '  <!-- Lucide 1.42.0; rounded 2 px strokes expanded for GTK 4.14+. See LICENSE. -->\n'
        f'  <path fill="#000000" d="{pen.getCommands()}"/>\n'
        '</svg>\n'
    )


if __name__ == "__main__":
    normalize(Path(sys.argv[1]), Path(sys.argv[2]))
