#!/usr/bin/env python3
"""Build the README walkthrough from real Ratatui cells and synthetic dialogue.

The presentation labels are outside the terminal. No native app, provider call,
command execution, or product UI modification is involved in this export.
"""
import argparse
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'work/testdeps'))
sys.path.insert(0, str(ROOT / 'tests'))
from PIL import Image, ImageDraw, ImageFont
from render_frames import color

BACKGROUND = '#181818'
TERMINAL = '#111111'
ACCENT = '#ffd7af'
TEXT = '#ededed'
MUTED = '#a7a7a7'
STEPS = ['Open', 'Ask', 'Follow up', 'Use command']


def fonts(font_path, size):
    mono = ImageFont.truetype(str(font_path), size)
    try:
        bold = ImageFont.truetype(str(font_path), size, index=1)
    except OSError:
        bold = mono
    sans_path = Path('/System/Library/Fonts/SFNS.ttf')
    def sans(n):
        return ImageFont.truetype(str(sans_path if sans_path.exists() else font_path), n)
    return mono, bold, sans(26), sans(17), sans(13), sans(11)


def render_frame(data, shot, font_set):
    mono, bold, heading, hint_font, small, disclosure = font_set
    cell_w = round(mono.getlength('M'))
    cell_h = round(mono.size * 1.45)
    width = data['columns'] * cell_w + 64
    terminal_y = 86
    terminal_x = 32
    bottom = terminal_y + data['rows'] * cell_h
    image = Image.new('RGB', (width, bottom + 120), BACKGROUND)
    draw = ImageDraw.Draw(image)
    draw.text((32, 15), 'AMBER SHELL', font=small, fill=ACCENT)
    draw.text((32, 39), shot['title'], font=heading, fill=TEXT)
    number = f"{shot['step'] + 1:02d} / 04"
    draw.text((width - 32, 17), number, font=small, fill=MUTED, anchor='rt')
    draw.rounded_rectangle((20, terminal_y - 12, width - 20, bottom + 12), radius=9,
                           fill=TERMINAL, outline='#383838', width=1)
    # Paint every background before any text so wide terminal glyphs remain intact.
    for i, (symbol, style_id) in enumerate(shot['cells']):
        style = data['styles'][style_id]
        fg, bg = color(style['fg']), color(style['bg'], True)
        if style['reversed']:
            fg, bg = bg, fg
        x = terminal_x + i % data['columns'] * cell_w
        y = terminal_y + i // data['columns'] * cell_h
        draw.rectangle((x, y, x + cell_w - 1, y + cell_h - 1), fill=bg)
    for i, (symbol, style_id) in enumerate(shot['cells']):
        style = data['styles'][style_id]
        fg = color(style['bg'], True) if style['reversed'] else color(style['fg'])
        x = terminal_x + i % data['columns'] * cell_w
        y = terminal_y + i // data['columns'] * cell_h
        # Menlo Bold lacks parts of the terminal box-drawing range; use its
        # regular face for those glyphs.
        line_art = any('\u2500' <= char <= '\u257f' for char in symbol)
        face = bold if style['bold'] and not line_art else mono
        draw.text((x, y), symbol, font=face, fill=fg)
        if style['underlined']:
            draw.line((x, y + cell_h - 3, x + cell_w, y + cell_h - 3), fill=fg)
    if shot['cursor']:
        x, y = shot['cursor']
        x, y = terminal_x + x * cell_w, terminal_y + y * cell_h
        draw.rectangle((x, y + 3, x + 1, y + mono.size + 3), fill=ACCENT)
    footer_y = bottom + 29
    draw.text((32, footer_y), shot['hint'], font=hint_font, fill=TEXT)
    if shot['key']:
        key_w = round(small.getlength(shot['key'])) + 24
        x = width - 32 - key_w
        draw.rounded_rectangle((x, footer_y - 3, width - 32, footer_y + 23), radius=5,
                               fill='#302820', outline='#6d5945')
        draw.text((x + 12, footer_y + 2), shot['key'], font=small, fill=ACCENT)
    step_w = (width - 64) / 4
    for index, label in enumerate(STEPS):
        x = round(32 + index * step_w)
        active = index == shot['step']
        draw.line((x, bottom + 71, round(x + step_w - 12), bottom + 71),
                  fill=ACCENT if active else '#3d3d3d', width=2)
        draw.text((x, bottom + 80), f'{index + 1:02d}  {label}', font=small,
                  fill=ACCENT if active else MUTED)
    note = 'Ratatui render · Simulated replies · Commands are not executed'
    draw.text((32, bottom + 105), note, font=disclosure, fill=MUTED)
    return image


def readable_text(data, shot):
    return '\n'.join(''.join(cell[0] for cell in shot['cells'][start:start + data['columns']])
                     for start in range(0, len(shot['cells']), data['columns']))


def keyframes(data):
    frames = data['frames']
    choices = [0]
    for predicate in [
        lambda f: f['step'] == 1 and f['key'] == 'Enter',
        lambda f: f['step'] == 1 and '> ls' in readable_text(data, f),
        lambda f: f['step'] == 2 and '> ls -a' in readable_text(data, f),
        lambda f: f['key'] == 'Ctrl-Y',
        lambda f: f['kind'] == 'shell' and f['command'],
    ]:
        choices.append(next(i for i, f in enumerate(frames) if predicate(f)))
    return choices


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, default=ROOT / 'work/product-demo/timeline.json')
    parser.add_argument('--output', type=Path, default=ROOT / 'assets/amber-demo.gif')
    parser.add_argument('--font', type=Path, default=Path('/System/Library/Fonts/Menlo.ttc'))
    parser.add_argument('--font-size', type=int, default=18)
    parser.add_argument('--preview-dir', type=Path)
    args = parser.parse_args()
    data = json.loads(args.input.read_text())
    if not data['simulated'] or data['executed']:
        raise ValueError('Product demo must contain synthetic, unexecuted commands only')
    if not data['frames'] or data['frames'][-1]['kind'] != 'shell':
        raise ValueError('Demo must finish at the editable shell prompt')
    for frame in data['frames']:
        if len(frame['cells']) != data['columns'] * data['rows']:
            raise ValueError('Incomplete terminal frame')
    font_set = fonts(args.font, args.font_size)
    shots = keyframes(data)
    previews = [render_frame(data, data['frames'][index], font_set) for index in shots]
    # One shared palette avoids color shimmer between otherwise identical frames.
    swatch = Image.new('RGB', (previews[0].width * 3, previews[0].height * 2))
    for i, image in enumerate(previews):
        swatch.paste(image, ((i % 3) * image.width, (i // 3) * image.height))
    palette = swatch.quantize(colors=128, method=Image.Quantize.MEDIANCUT)
    frames = [render_frame(data, shot, font_set).quantize(palette=palette, dither=Image.Dither.NONE)
              for shot in data['frames']]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    frames[0].save(args.output, save_all=True, append_images=frames[1:],
                   duration=[f['duration_ms'] for f in data['frames']],
                   loop=0, optimize=True, disposal=1)
    if args.preview_dir:
        args.preview_dir.mkdir(parents=True, exist_ok=True)
        names = ['open', 'question', 'answer', 'followup', 'transfer', 'prompt']
        for number, (name, image) in enumerate(zip(names, previews), 1):
            image.save(args.preview_dir / f'{number:02d}-{name}.png')
        swatch.resize((1284, round(swatch.height * 1284 / swatch.width)), Image.Resampling.LANCZOS).save(
            args.preview_dir / 'storyboard.png')
    with Image.open(args.output) as saved:
        duration = sum((saved.seek(n), saved.info['duration'])[1] for n in range(saved.n_frames))
        if duration != sum(f['duration_ms'] for f in data['frames']):
            raise ValueError('GIF timing differs from the source timeline')
        print(f'{args.output}: {saved.n_frames} frames, {saved.width}×{saved.height}, '
              f'{duration / 1000:.2f}s, {args.output.stat().st_size / 1024:.0f} KiB')


if __name__ == '__main__':
    main()
