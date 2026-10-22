#!/usr/bin/env python3
"""Render actual Ratatui TestBackend cell JSON as labeled PNG artifacts.

This is a deterministic Terminal-Test-Render, not a native terminal screenshot.
Usage: python3 tests/render_frames.py work/render-baseline [--output DIR]
Pillow can be installed locally under work/testdeps. Colors marked Reset use
an explicit dark terminal palette; actual terminal profile colors may differ.
"""
import argparse
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'work/testdeps'))
from PIL import Image, ImageDraw, ImageFont

PALETTE = {
    'Black': '#000000', 'Red': '#cd3131', 'Green': '#0dbc79',
    'Yellow': '#e5b95c', 'Blue': '#2472c8', 'Magenta': '#bc3fbc',
    'Cyan': '#11a8cd', 'Gray': '#d5d5d5', 'DarkGray': '#737373',
    'LightRed': '#f14c4c', 'LightGreen': '#23d18b', 'LightYellow': '#f5d76e',
    'LightBlue': '#3b8eea', 'LightMagenta': '#d670d6',
    'LightCyan': '#29b8db', 'White': '#f5f5f5',
}


def color(value, background=False):
    if value == 'Reset':
        return '#111111' if background else '#dddddd'
    if value in PALETTE:
        return PALETTE[value]
    rgb = re.fullmatch(r'Rgb\((\d+), (\d+), (\d+)\)', value)
    if rgb:
        return tuple(map(int, rgb.groups()))
    indexed = re.fullmatch(r'Indexed\((\d+)\)', value)
    if indexed:
        index = int(indexed[1])
        if index < 16:
            return list(PALETTE.values())[index]
        if index >= 232:
            return (8 + 10 * (index - 232),) * 3
        index -= 16
        level = [0, 95, 135, 175, 215, 255]
        return (level[index // 36], level[index // 6 % 6], level[index % 6])
    raise ValueError('Unsupported Ratatui color: ' + value)


def render(source, output, font_path, size):
    data = json.loads(source.read_text())
    width, height = data['width'], data['height']
    cells = data['cells']
    assert len(cells) == width * height
    font = ImageFont.truetype(str(font_path), size)
    cell_width = round(font.getlength('M'))
    cell_height = round(size * 1.45)
    padding, heading = 18, 42
    image = Image.new('RGB', (width * cell_width + padding * 2,
                             height * cell_height + padding * 2 + heading), '#181818')
    draw = ImageDraw.Draw(image)
    draw.text((padding, 10), f"Terminal-Test-Render · {data['state']} · {width}×{height}",
              font=font, fill='#cccccc')
    # Draw backgrounds first so wide glyphs aren't erased by the following cell.
    for index, cell in enumerate(cells):
        x, y = padding + index % width * cell_width, heading + index // width * cell_height
        fg, bg = color(cell['fg']), color(cell['bg'], True)
        if 'REVERSED' in cell['modifier']:
            fg, bg = bg, fg
        draw.rectangle((x, y, x + cell_width - 1, y + cell_height - 1), fill=bg)
    for index, cell in enumerate(cells):
        x, y = padding + index % width * cell_width, heading + index // width * cell_height
        fg = color(cell['bg'], True) if 'REVERSED' in cell['modifier'] else color(cell['fg'])
        draw.text((x, y), cell['symbol'], font=font, fill=fg,
                  stroke_width=1 if 'BOLD' in cell['modifier'] else 0)
        if 'UNDERLINED' in cell['modifier']:
            draw.line((x, y + cell_height - 3, x + cell_width, y + cell_height - 3), fill=fg)
    image.save(output)
    print(output)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--font', type=Path, default=Path('/System/Library/Fonts/Menlo.ttc'))
    parser.add_argument('--font-size', type=int, default=16)
    args = parser.parse_args()
    sources = sorted(args.input.glob('*.json')) if args.input.is_dir() else [args.input]
    destination = args.output or (args.input.parent if args.input.is_file() else args.input)
    destination.mkdir(parents=True, exist_ok=True)
    for source in sources:
        render(source, destination / (source.stem + '.png'), args.font, args.font_size)
