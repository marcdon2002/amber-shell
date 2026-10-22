#!/usr/bin/env python3
"""Package labeled renderer PNGs into the README demo; no provider requests."""
import argparse
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'work/testdeps'))
from PIL import Image


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, default=ROOT / 'work/readme-frames')
    parser.add_argument('--output', type=Path, default=ROOT / 'assets/amber-demo.gif')
    args = parser.parse_args()
    states = (['start'] + ['waiting', 'waiting-1', 'waiting-2', 'waiting-3'] * 2
              + ['stream', 'stream-1', 'stream-2', 'stream-3'] * 2 + ['final', 'error'])
    frames = []
    for state in states:
        with Image.open(args.input / f'{state}-100x20.png') as source:
            frames.append(source.convert('RGB'))
    if len({frame.size for frame in frames}) != 1:
        raise ValueError('All demo frames must have the same dimensions')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    frames[0].save(args.output, save_all=True, append_images=frames[1:],
                   duration=[900] + [320] * 16 + [1200, 1000], loop=0, optimize=True)
    print(f'{args.output}: {len(frames)} frames, {frames[0].width}x{frames[0].height}')


if __name__ == '__main__':
    main()
